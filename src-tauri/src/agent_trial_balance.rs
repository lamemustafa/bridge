//! Thin MCP presentation of the shared native report operation.
use super::*;

impl Server {
    pub(super) async fn trial_balance(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        if from > to {
            return Err("invalid_date_range".to_string().into());
        }
        let from =
            bridge_tally_core::TallyDate::parse(from).map_err(|_| "invalid_date".to_string())?;
        let to = bridge_tally_core::TallyDate::parse(to).map_err(|_| "invalid_date".to_string())?;
        let (company, identity, prior) = self.verified_company(guid).await?;
        let read = self
            .runtime
            .fetch_trial_balance(self.tally_config(), &identity, from, to)
            .await
            .map_err(|error| {
                ToolFailure::from_runtime("trial_balance_read_failed", error)
                    .with_prior_evidence(prior.clone())
            })?;
        let mut evidence = combine_evidence(prior, evidence_from_runtime_read(read.evidence));
        evidence.read_at = Some(read.read_at.clone());
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let total = read.report.rows.len();
        let rows = read
            .report
            .rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| {
                json!({
                    "ledger": party_name(row.name), "guid": row.guid,
                    "parent": row.parent, "opening": row.opening,
                    "debit": row.debit, "credit": row.credit, "closing": row.closing,
                })
            })
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(rows.len()) < total;
        Ok(ToolOutcome {
            payload: json!({
                "company": company_json(&company, std::slice::from_ref(&company)),
                "result": {
                    "state": "observed", "basis": "tally_native_trial_balance",
                    "from": read.from, "to": read.to, "currency": read.currency,
                    "read_at": read.read_at, "ledgers": rows, "total_ledgers": total,
                    "totals": read.totals, "totals_scope": "all_returned_ledgers_numeric_observations_only",
                    "offset": offset, "next_offset": truncated.then_some(offset + limit),
                    "verification": "stable_paired_source_with_company_mode_and_extent_guards",
                    "limitations": ["Not voucher-level reconciliation or an atomic snapshot", "Native empty amounts are not numeric zero", "Includes ledger masters Tally may hide in its screen"],
                },
            }),
            evidence,
            company_guid: Some(guid.to_string()),
            truncated,
        })
    }
}
