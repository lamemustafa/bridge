//! Outstandings for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn outstandings(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, mut result_evidence) = self.verified_company(guid).await?;
        let result: Result<ToolOutcome, ToolFailure> = async {
            let as_of = optional_string(args, "as_of")?
                .as_deref()
                .map(normalized_date)
                .transpose()?
                .unwrap_or_else(tally_host_today);
            let to =
                bridge_tally_core::TallyDate::parse(as_of).map_err(|_| "invalid_as_of".to_string())?;
            let ageing_basis =
                optional_string(args, "ageing_basis")?.unwrap_or_else(|| "due_date".to_string());
            let ageing_anchor = match ageing_basis.as_str() {
                "bill_date" => OutstandingsAgeingAnchor::BillDate,
                "due_date" => OutstandingsAgeingAnchor::DueDate,
                _ => return Err("invalid_ageing_basis".to_string().into()),
            };
            let (currency, currency_evidence) = self
                .runtime
                .detect_base_currency_with_evidence(self.tally_config(), &identity)
                .await
                .map_err(|_| "company_currency_probe_failed".to_string())?;
            result_evidence = combine_evidence(result_evidence.clone(), evidence_from_runtime_read(currency_evidence));
            let assertion = match (currency.currency_count, currency.is_inr) {
                (1, true) => OutstandingsCurrencyAssertion::Inr,
                (0, _) => return Err("company_currency_probe_failed".to_string().into()),
                (1, false) => return Err("company_base_currency_not_inr".to_string().into()),
                _ => return Err("company_base_currency_undetermined".to_string().into()),
            };
            let (load, outstandings_evidence) = self
                .runtime
                .fetch_outstandings_with_evidence(
                    self.tally_config(),
                    &identity,
                    to,
                    assertion,
                    ageing_anchor,
                )
                .await
                .map_err(|_| "native_outstandings_read_failed".to_string())?;
            result_evidence = combine_evidence(result_evidence.clone(), evidence_from_runtime_read(outstandings_evidence));
            let top = arg_positive_usize(args, "top", 25)?.min(self.settings.max_rows);
            let bill_offset = arg_usize(args, "offset", 0)?;
            let bill_limit =
                arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
            let (result, bills_truncated) = match load {
                OutstandingsLoadResult::Complete {
                    report: _,
                    statement_open_bills,
                    statement_unallocated_by_party,
                    ..
                } => {
                    let direction =
                        optional_string(args, "direction")?.unwrap_or_else(|| "both".to_string());
                    if !matches!(direction.as_str(), "receivable" | "payable" | "both") {
                        return Err("invalid_direction".to_string().into());
                    }
                    let all_bills = statement_open_bills
                        .into_iter()
                        .filter(|bill| direction_matches(bill.kind, &direction))
                        .collect::<Vec<_>>();
                    let selected_unallocated = statement_unallocated_by_party
                        .into_iter()
                        .filter(|party| direction_matches(party.direction, &direction))
                        .collect::<Vec<_>>();
                    let parties = ranked_parties_from_exposure(&all_bills, &selected_unallocated, top)?
                        .into_iter()
                        .map(|party| redact_value(party, self.settings.redaction))
                        .collect::<Vec<_>>();
                    let totals = outstanding_totals_from_open_bills(&all_bills)?;
                    let ageing_buckets = ageing_buckets_from_open_bills(&all_bills)?;
                    let (bills, bills_truncated, next_bill_offset) =
                        paginate_open_bills(all_bills, bill_offset, bill_limit);
                    let bills = bills
                        .into_iter()
                        .map(|bill| redact_value(open_bill_json(&bill), self.settings.redaction))
                        .collect::<Vec<_>>();
                    let unallocated_count = selected_unallocated.len();
                    let unallocated_totals = unallocated_totals_from_parties(&selected_unallocated)?;
                    let (unallocated, unallocated_truncated, next_unallocated_offset) =
                        paginate_open_bills(selected_unallocated, bill_offset, bill_limit);
                    let unallocated = unallocated
                        .into_iter()
                        .map(|party| {
                            redact_value(unallocated_party_json(&party), self.settings.redaction)
                        })
                        .collect::<Vec<_>>();
                    (
                        json!({"state":"complete", "totals":totals, "ageing_basis": if matches!(ageing_anchor, OutstandingsAgeingAnchor::BillDate) {"bill_date"} else {"due_date"}, "ageing_buckets": ageing_buckets, "top_parties": parties, "top_parties_ranked_by":"gross_exposure", "open_bills": bills, "offset": bill_offset, "limit": bill_limit, "next_offset": next_bill_offset, "unallocated":{"count": unallocated_count, "totals": unallocated_totals, "parties": unallocated, "truncated": unallocated_truncated, "next_offset": next_unallocated_offset}}),
                        bills_truncated || unallocated_truncated,
                    )
                }
                OutstandingsLoadResult::Partial { reason, .. } => {
                    result_evidence.state = "partial";
                    result_evidence.reason_code = Some(reason.reason_code.clone());
                    (
                        json!({"state":"partial", "partial_reason": reason.reason_code}),
                        false,
                    )
                }
            };
            Ok(ToolOutcome {
                payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
                evidence: result_evidence.clone(),
                company_guid: Some(guid.to_string()),
                truncated: bills_truncated,
            })
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(result_evidence))
    }
}

pub(super) fn direction_matches(kind: ExposureDirection, requested: &str) -> bool {
    requested == "both" || kind.label().eq_ignore_ascii_case(requested)
}

pub(super) fn outstanding_totals_from_open_bills(bills: &[OpenBillRow]) -> Result<Value, String> {
    let mut receivable = "0".to_string();
    let mut payable = "0".to_string();
    for bill in bills {
        match bill.kind {
            ExposureDirection::Receivable => {
                receivable = add_decimal(&receivable, bill.amount.as_str())?
            }
            ExposureDirection::Payable => payable = add_decimal(&payable, bill.amount.as_str())?,
        }
    }
    let gross_billed = add_decimal(&receivable, &payable)?;
    Ok(
        json!({"scope":"open_bills_only", "receivable": receivable, "payable": payable, "gross_billed":gross_billed}),
    )
}

pub(super) fn ageing_buckets_from_open_bills(bills: &[OpenBillRow]) -> Result<Value, String> {
    let mut unaged = "0".to_string();
    let mut days_0_30 = "0".to_string();
    let mut days_31_60 = "0".to_string();
    let mut days_61_90 = "0".to_string();
    let mut days_90_plus = "0".to_string();
    for bill in bills {
        let bucket = match bill.age_days {
            None => &mut unaged,
            Some(age) => match age {
                0..=30 => &mut days_0_30,
                31..=60 => &mut days_31_60,
                61..=90 => &mut days_61_90,
                _ => &mut days_90_plus,
            },
        };
        *bucket = add_decimal(bucket, bill.amount.as_str())?;
    }
    Ok(json!({
        "unaged": unaged,
        "days_0_30": days_0_30,
        "days_31_60": days_31_60,
        "days_61_90": days_61_90,
        "days_90_plus": days_90_plus,
    }))
}

pub(super) fn unallocated_totals_from_parties(
    parties: &[UnallocatedParty],
) -> Result<Value, String> {
    let mut receivable = "0".to_string();
    let mut payable = "0".to_string();
    for party in parties {
        let total = match party.direction {
            ExposureDirection::Receivable => &mut receivable,
            ExposureDirection::Payable => &mut payable,
        };
        *total = add_decimal(total, party.amount.as_str())?;
    }
    let gross_unallocated = add_decimal(&receivable, &payable)?;
    Ok(json!({"receivable":receivable, "payable":payable, "gross_unallocated":gross_unallocated}))
}

struct PartyExposure {
    billed_receivable: String,
    billed_payable: String,
    unallocated_receivable: String,
    unallocated_payable: String,
    oldest_bill_age_days: Option<u32>,
}

impl Default for PartyExposure {
    fn default() -> Self {
        Self {
            billed_receivable: "0".into(),
            billed_payable: "0".into(),
            unallocated_receivable: "0".into(),
            unallocated_payable: "0".into(),
            oldest_bill_age_days: None,
        }
    }
}

pub(super) fn ranked_parties_from_exposure(
    bills: &[OpenBillRow],
    unallocated: &[UnallocatedParty],
    top: usize,
) -> Result<Vec<Value>, String> {
    let mut totals = BTreeMap::<String, PartyExposure>::new();
    for bill in bills {
        let entry = totals.entry(bill.party.clone()).or_default();
        let total = match bill.kind {
            ExposureDirection::Receivable => &mut entry.billed_receivable,
            ExposureDirection::Payable => &mut entry.billed_payable,
        };
        *total = add_decimal(total, bill.amount.as_str())?;
        entry.oldest_bill_age_days = entry.oldest_bill_age_days.max(bill.age_days);
    }
    for party in unallocated {
        let entry = totals.entry(party.party.clone()).or_default();
        let total = match party.direction {
            ExposureDirection::Receivable => &mut entry.unallocated_receivable,
            ExposureDirection::Payable => &mut entry.unallocated_payable,
        };
        *total = add_decimal(total, party.amount.as_str())?;
    }
    let mut ranked = totals
        .into_iter()
        .map(|(party, exposure)| {
            let gross_billed = add_decimal(&exposure.billed_receivable, &exposure.billed_payable)?;
            let gross_unallocated = add_decimal(
                &exposure.unallocated_receivable,
                &exposure.unallocated_payable,
            )?;
            let gross_exposure = add_decimal(&gross_billed, &gross_unallocated)?;
            let magnitude = bridge_tally_core::ExactDecimal::parse(gross_exposure.clone())
                .map_err(|_| "outstandings_amount_invalid".to_string())?;
            let row = json!({
                "party": party_name(party.clone()),
                "billed_receivable": exposure.billed_receivable,
                "billed_payable": exposure.billed_payable,
                "gross_billed": gross_billed,
                "unallocated_receivable": exposure.unallocated_receivable,
                "unallocated_payable": exposure.unallocated_payable,
                "gross_unallocated": gross_unallocated,
                "gross_exposure": gross_exposure,
                "oldest_bill_age_days": exposure.oldest_bill_age_days,
            });
            Ok((party, magnitude, row))
        })
        .collect::<Result<Vec<_>, String>>()?;
    ranked.sort_by(|left, right| {
        right
            .1
            .cmp_magnitude(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(ranked
        .into_iter()
        .take(top)
        .map(|(_, _, row)| row)
        .collect())
}

pub(super) fn paginate_open_bills<T>(
    bills: Vec<T>,
    offset: usize,
    limit: usize,
) -> (Vec<T>, bool, Option<usize>) {
    let total = bills.len();
    let page = bills
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let truncated = page_is_truncated(total, offset, page.len());
    let next_offset = truncated.then_some(offset + page.len());
    (page, truncated, next_offset)
}

#[cfg(test)]
#[path = "agent_outstandings_tests.rs"]
mod tests;
