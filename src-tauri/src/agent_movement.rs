//! Movement for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn ledger_movement(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        if from > to {
            return Err("invalid_date_range".to_string().into());
        }
        let (company, identity, mut evidence) = self.verified_company(guid).await?;
        let result: Result<ToolOutcome, ToolFailure> = async {
            let books_from = normalized_date(
                company
                    .books_from
                    .as_deref()
                    .ok_or_else(|| "company_identity_incomplete".to_string())?,
            )?;
            ensure_movement_window_within_books(&from, &books_from)?;
            let opening_date = bridge_tally_core::TallyDate::parse(from.clone())
                .map_err(|_| "invalid_date".to_string())?;
            let (ledgers, ledger_evidence) = self
                .read_movement_ledgers(&identity, opening_date.clone())
                .await?;
            evidence = combine_evidence(evidence.clone(), ledger_evidence);
            let (page, read_evidence) = self
                .read_movement_vouchers(&identity, &company.name, from.clone(), to.clone())
                .await?;
            let MovementPage {
                rows: vouchers,
                observed_rows: voucher_rows_observed,
            } = page;
            let voucher_snapshot = read_evidence.response_sha256.clone();
            evidence = combine_evidence(evidence.clone(), read_evidence);
            let (corroborating_ledgers, corroboration_evidence) =
                self.read_movement_ledgers(&identity, opening_date).await?;
            evidence = combine_evidence(evidence.clone(), corroboration_evidence);
            // An in-window posting need not change the period opening. Repeat
            // the voucher source across the final ledger read as well; an
            // AlterID high-water alone cannot establish deletion stability.
            let (_, closing_voucher_evidence) = self
                .read_movement_vouchers(&identity, &company.name, from, to)
                .await?;
            let closing_snapshot = closing_voucher_evidence.response_sha256.clone();
            evidence = combine_evidence(evidence.clone(), closing_voucher_evidence);
            if voucher_snapshot != closing_snapshot {
                return Err("voucher_snapshot_drifted".to_string().into());
            }
            validate_movement_snapshot(&ledgers, &corroborating_ledgers, &vouchers)?;
            let selected = optional_string(args, "ledger")?
                .map(|name| {
                    resolve_ledger_name(ledgers.iter().map(|ledger| ledger.name.as_str()), &name)
                })
                .transpose()?;
            let mut movement =
                BTreeMap::<String, (Option<String>, Option<String>, String, String, usize)>::new();
            for ledger in ledgers {
                if selected.as_deref().is_none_or(|name| name == ledger.name) {
                    movement.insert(
                        ledger.name,
                        (
                            ledger.parent.returned_text().map(str::to_string),
                            ledger.opening_balance,
                            "0".into(),
                            "0".into(),
                            0,
                        ),
                    );
                }
            }
            for voucher in &vouchers {
                let mut touched = std::collections::BTreeSet::new();
                for entry in &voucher.ledger_entries {
                    let Some(record) = movement.get_mut(&entry.ledger_name) else {
                        // The full catalogue was corroborated before selection;
                        // this entry belongs to a known, unselected ledger.
                        continue;
                    };
                    let amount = bridge_tally_core::ExactDecimal::parse(entry.amount.clone())
                        .map_err(|_| "voucher_amount_invalid".to_string())?;
                    let magnitude = amount
                        .abs()
                        .map_err(|_| "voucher_amount_invalid".to_string())?
                        .as_str()
                        .to_string();
                    if entry.is_deemed_positive {
                        record.2 = add_decimal(&record.2, &format!("-{magnitude}"))?;
                    } else {
                        record.3 = add_decimal(&record.3, &magnitude)?;
                    }
                    touched.insert(entry.ledger_name.clone());
                }
                for ledger in touched {
                    if let Some(record) = movement.get_mut(&ledger) {
                        record.4 += 1;
                    }
                }
            }
            let (rows, opening_unobserved) = movement
                .into_iter()
                .map(
                    |(name, (parent, opening, debit, credit, vouchers_touching))| {
                        ledger_movement_row(
                            LedgerMovementRow {
                                name,
                                parent,
                                opening,
                                debit,
                                credit,
                                vouchers_touching,
                            },
                            self.settings.redaction,
                        )
                    },
                )
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .fold(
                    (Vec::new(), false),
                    |(mut rows, any_partial), (row, partial)| {
                        rows.push(row);
                        (rows, any_partial || partial)
                    },
                );
            if opening_unobserved {
                evidence.state = "partial";
                evidence.reason_code = Some("opening_balance_not_observed".to_string());
            }
            let offset = arg_usize(args, "offset", 0)?;
            let limit =
                arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
            let total = rows.len();
            let rows = rows
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>();
            let truncated = offset.saturating_add(rows.len()) < total;
            let next_offset = truncated.then_some(offset + rows.len());
            Ok(ToolOutcome {
                payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"state": if opening_unobserved {"partial"} else {"complete"}, "partial_reason": opening_unobserved.then_some("opening_balance_not_observed"), "ledgers": rows, "offset": offset, "next_offset": next_offset, "voucher_rows_observed": voucher_rows_observed, "balance_basis": "tally_period_opening_plus_direct_voucher_movement", "evidence_method": "runtime_ledger_opening_at_from_plus_literal_window_entries"}}),
                evidence: evidence.clone(),
                company_guid: Some(guid.to_string()),
                truncated,
            })
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(evidence))
    }

    pub(super) async fn read_movement_ledgers(
        &self,
        identity: &VerifiedCompanyIdentity,
        from: bridge_tally_core::TallyDate,
    ) -> Result<(Vec<TallyLedger>, Evidence), ToolFailure> {
        let (ledgers, evidence) = self
            .runtime
            .fetch_ledger_opening_at_with_evidence(self.tally_config(), identity, from)
            .await
            .map_err(|error| ToolFailure::from_runtime("ledger_movement_read_failed", error))?;
        Ok((ledgers, evidence_from_runtime_read(evidence)))
    }

    async fn read_movement_vouchers(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        from: String,
        to: String,
    ) -> Result<(MovementPage, Evidence), ToolFailure> {
        let company = ValidatedCompanyName::new(company.to_string())
            .map_err(|_| "company_name_invalid".to_string())?;
        let range =
            ValidatedDateRange::new(from, to).map_err(|_| "invalid_date_range".to_string())?;
        let (xml, mut evidence) = self
            .post_read(
                identity,
                render_agent_vouchers(
                    company.as_str(),
                    range.from_yyyymmdd(),
                    range.to_yyyymmdd(),
                    None,
                )?,
            )
            .await?;
        let result: Result<(MovementPage, Evidence), ToolFailure> = async {
            let page = parse_movement_rows(
                parse_agent_changed_rows(&xml, identity.company_guid())?,
                range.from_yyyymmdd(),
                range.to_yyyymmdd(),
            )?;
            if page.observed_rows == 0 {
                let (corroboration, partial, reason) = self
                    .corroborate_empty_voucher_read(
                        identity,
                        company.as_str(),
                        range.from_yyyymmdd(),
                        range.to_yyyymmdd(),
                        None,
                    )
                    .await?;
                evidence = combine_evidence(evidence.clone(), corroboration);
                if partial {
                    return Err(reason.unwrap_or("empty_uncorroborated").to_string().into());
                }
            }
            Ok((page, evidence.clone()))
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(evidence))
    }
}

fn validate_movement_snapshot(
    opening: &[TallyLedger],
    corroboration: &[TallyLedger],
    vouchers: &[MovementVoucher],
) -> Result<(), String> {
    let initial = opening
        .iter()
        .map(|ledger| (ledger.name.as_str(), ledger))
        .collect::<BTreeMap<_, _>>();
    let repeated = corroboration
        .iter()
        .map(|ledger| (ledger.name.as_str(), ledger))
        .collect::<BTreeMap<_, _>>();
    if initial.len() != opening.len()
        || repeated.len() != corroboration.len()
        || initial != repeated
        || vouchers
            .iter()
            .flat_map(|voucher| &voucher.ledger_entries)
            .any(|entry| !initial.contains_key(entry.ledger_name.as_str()))
    {
        return Err("ledger_snapshot_drifted".to_string());
    }
    Ok(())
}

/// Native collection rows are parsed through the same boundary as the change
/// feed. Bridge's report-format parser expects a different XML vocabulary.
#[derive(serde::Deserialize)]
pub(super) struct MovementVoucher {
    date: String,
    cancelled: bool,
    optional: bool,
    #[serde(rename = "amounts")]
    pub(super) ledger_entries: Vec<MovementEntry>,
}

#[derive(serde::Deserialize)]
pub(super) struct MovementEntry {
    #[serde(rename = "ledger")]
    ledger_name: String,
    amount: String,
    #[serde(deserialize_with = "parse_movement_polarity")]
    is_deemed_positive: bool,
}

fn parse_movement_polarity<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    required_tally_bool(Some(&value)).map_err(serde::de::Error::custom)
}

#[cfg(test)]
pub(super) fn parse_movement_vouchers(
    xml: &str,
    from: &str,
    to: &str,
    company_guid: &str,
) -> Result<Vec<MovementVoucher>, String> {
    Ok(parse_movement_rows(parse_agent_changed_rows(xml, company_guid)?, from, to)?.rows)
}

struct MovementPage {
    rows: Vec<MovementVoucher>,
    observed_rows: usize,
}

fn parse_movement_rows(rows: Vec<Value>, from: &str, to: &str) -> Result<MovementPage, String> {
    let observed_rows = rows.len();
    let vouchers = rows
        .into_iter()
        .map(|row| {
            serde_json::from_value::<MovementVoucher>(row)
                .map_err(|_| "ledger_movement_read_failed".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    // Check the whole window before excluding cancelled/optional records.
    if vouchers
        .iter()
        .any(|voucher| voucher.date.as_str() < from || voucher.date.as_str() > to)
    {
        return Err("window_not_honoured".to_string());
    }
    for voucher in vouchers
        .iter()
        .filter(|voucher| !voucher.cancelled && !voucher.optional)
    {
        if voucher.ledger_entries.is_empty() {
            return Err("ledger_movement_entries_not_observed".to_string());
        }
        let total = voucher.ledger_entries.iter().try_fold(
            bridge_tally_core::ExactDecimal::zero(),
            |total, entry| {
                bridge_tally_core::ExactDecimal::parse(entry.amount.clone())
                    .and_then(|amount| total.checked_add(&amount))
                    .map_err(|_| "voucher_amount_invalid".to_string())
            },
        )?;
        if !total.is_zero() {
            return Err("voucher_entries_unbalanced".to_string());
        }
    }
    Ok(MovementPage {
        rows: vouchers
            .into_iter()
            .filter(|voucher| !voucher.cancelled && !voucher.optional)
            .collect(),
        observed_rows,
    })
}

#[cfg(test)]
#[path = "agent_movement_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_movement_snapshot_tests.rs"]
mod snapshot_tests;
