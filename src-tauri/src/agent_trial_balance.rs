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
                    .fetch_trial_balance_with_extent(self.tally_config(), &identity, period)
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
                let frame = json!({
                    "from": read.from, "to": read.to, "currency": read.currency,
                    "read_at": read.read_at, "totals": read.totals,
                });
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
        Ok(ToolOutcome {
            payload: json!({
                "company": company_json(&company, std::slice::from_ref(&company)),
                "result": {
                    "state": "observed", "basis": "tally_native_trial_balance",
                    "from": frame["from"], "to": frame["to"], "currency": frame["currency"],
                    "read_at": frame["read_at"], "ledgers": rows, "total_ledgers": total,
                    "totals": frame["totals"], "totals_scope": "all_returned_ledgers_numeric_observations_only",
                    "offset": offset, "next_offset": next_offset,
                    "verification": "stable_paired_source_with_company_mode_and_extent_guards",
                    "limitations": ["Not voucher-level reconciliation or an atomic snapshot", "Native empty amounts are not numeric zero", "Includes ledger masters Tally may hide in its screen"],
                    "snapshot": snapshot.describe(reused.is_some()),
                },
            }),
            evidence,
            company_guid: Some(guid.to_string()),
            truncated,
        })
    }
}

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
