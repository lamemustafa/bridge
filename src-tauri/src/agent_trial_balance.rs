//! Thin MCP presentation of the shared native report operation.
use super::*;

impl Server {
    pub(super) async fn trial_balance(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        let from =
            bridge_tally_core::TallyDate::parse(from).map_err(|_| "invalid_date".to_string())?;
        let to = bridge_tally_core::TallyDate::parse(to).map_err(|_| "invalid_date".to_string())?;
        let period = crate::tally::runtime::TrialBalancePeriod::new(from.clone(), to.clone())
            .map_err(|_| "invalid_date_range".to_string())?;
        let (company, identity, mut prior) = self.verified_company(guid).await?;
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let snapshot_id = optional_string(args, "snapshot_id")?;
        let kind = ListingKind::TrialBalance { from, to };
        // A first page always reads fresh; a later page is served from its
        // listing's snapshot only while the book's extent, including ALTVCHID,
        // is unchanged (#630).
        let reused = self
            .continued_listing(&identity, &kind, offset, snapshot_id.as_deref(), &mut prior)
            .await
            .map_err(|failure| failure.with_prior_evidence(prior.clone()))?;
        let snapshot = match reused.clone() {
            Some(held) => held,
            None => {
                let (read, extent) = self
                    .runtime
                    .fetch_trial_balance_with_extent(
                        self.tally_config(),
                        &identity,
                        period,
                        crate::tally::runtime::TrialBalanceCurrencyScope::BaseCurrencyLedgersOnly,
                    )
                    .await
                    .map_err(|error| {
                        ToolFailure::from_runtime("trial_balance_read_failed", error)
                            .with_prior_evidence(prior.clone())
                    })?;
                let rows = read
                    .report
                    .rows
                    .into_iter()
                    .map(|row| {
                        json!({
                            "ledger": party_name(row.name), "guid": row.guid,
                            "parent": row.parent, "opening": row.opening,
                            "debit": row.debit, "credit": row.credit, "closing": row.closing,
                        })
                    })
                    .collect::<Vec<_>>();
                let read_evidence = evidence_from_runtime_read(read.evidence);
                let mut frame = json!({
                    "from": read.from, "to": read.to, "currency": read.currency,
                    "read_at": read.read_at, "totals": read.totals,
                    "totals_scope": "all_returned_ledgers_numeric_observations_only",
                });
                if let crate::tally::runtime::TrialBalanceLedgerScope::BaseCurrencyLedgersOnly {
                    base_name,
                    decimal_places,
                    foreign,
                    mixed,
                } = &read.ledger_scope
                {
                    // A several-currency book (bridge#551): only its plain
                    // base-currency ledgers are read. The currency is the base
                    // Tally identified, never the first master read, and the
                    // totals are not expected to balance.
                    frame["currency"] = json!({
                        "base": base_name, "is_inr": true, "decimal_places": decimal_places,
                    });
                    frame["totals_scope"] =
                        json!("base_currency_ledgers_only_numeric_observations_only");
                    frame["ledgers_scope"] = json!("base_currency_ledgers_only");
                    frame["foreign_currency_ledgers_excluded"] = json!({
                        "count": foreign.len(),
                        "ledgers": foreign
                            .iter()
                            .take(EXCLUDED_TRIAL_BALANCE_LEDGERS_NAMED)
                            .map(|ledger| json!({
                                "ledger": party_name(ledger.ledger.clone()),
                                "currency": ledger.currency,
                            }))
                            .collect::<Vec<_>>(),
                    });
                    frame["base_currency_ledgers_mixed_excluded"] = json!({
                        "count": mixed.len(),
                        "reason": "mixed_currency_movement",
                        "ledgers": mixed
                            .iter()
                            .take(EXCLUDED_TRIAL_BALANCE_LEDGERS_NAMED)
                            .map(|ledger| party_name(ledger.clone()))
                            .collect::<Vec<_>>(),
                    });
                    frame["scope_limitation"] = json!(
                        "Totals cover this book's plain base-currency ledgers only. Ledgers kept in another currency, and base-currency ledgers with a balance Tally shows in another currency, are left out and named, so debit and credit totals are expected to differ."
                    );
                }
                self.hold_listing(ListingSnapshot::new(
                    &identity,
                    kind,
                    extent,
                    rows,
                    None,
                    frame,
                    read_evidence,
                ))?
            }
        };
        // A page served from a snapshot records only what it sent: the
        // identity and extent reads, not its first page's read again.
        let evidence = match reused {
            Some(_) => prior,
            None => combine_evidence(prior, snapshot.evidence.clone()),
        };
        let total = snapshot.rows.len();
        let rows = snapshot
            .rows
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let (truncated, next_offset) = page_boundary(offset, rows.len(), total);
        let frame = &snapshot.frame;
        let mut limitations = json!([
            "Not voucher-level reconciliation or an atomic snapshot",
            "Native empty amounts are not numeric zero",
            "Includes ledger masters Tally may hide in its screen"
        ]);
        let mut result = json!({
            "state": "observed", "basis": "tally_native_trial_balance",
            "from": frame["from"], "to": frame["to"], "currency": frame["currency"],
            "read_at": frame["read_at"], "ledgers": rows, "total_ledgers": total,
            "totals": frame["totals"], "totals_scope": frame["totals_scope"],
            "offset": offset, "next_offset": next_offset,
            "verification": "stable_paired_source_with_company_mode_and_extent_guards",
            "snapshot": snapshot.describe(reused.is_some()),
        });
        // A page served from the snapshot reports the same scope as its first.
        for key in [
            "ledgers_scope",
            "foreign_currency_ledgers_excluded",
            "base_currency_ledgers_mixed_excluded",
        ] {
            if !frame[key].is_null() {
                result[key] = frame[key].clone();
            }
        }
        if let (Some(limitation), Some(list)) = (
            frame["scope_limitation"].as_str(),
            limitations.as_array_mut(),
        ) {
            list.push(json!(limitation));
        }
        result["limitations"] = limitations;
        Ok(ToolOutcome {
            payload: json!({
                "company": company_json(&company, std::slice::from_ref(&company)),
                "result": result,
            }),
            evidence,
            company_guid: Some(guid.to_string()),
            truncated,
        })
    }
}

/// At most this many set-aside ledgers are named per list; `count` covers
/// them all. The lists are not paged with the rows, so they are bounded here.
const EXCLUDED_TRIAL_BALANCE_LEDGERS_NAMED: usize = 20;

fn page_boundary(offset: usize, returned: usize, total: usize) -> (bool, Option<usize>) {
    let Some(next_offset) = offset.checked_add(returned) else {
        return (false, None);
    };
    let truncated = next_offset < total;
    (truncated, truncated.then_some(next_offset))
}

#[cfg(test)]
#[path = "agent_trial_balance_tests.rs"]
mod tests;
