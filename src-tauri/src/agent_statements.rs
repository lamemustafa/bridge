//! Thin MCP presentation of the shared statement read (#692).
use super::*;
use bridge_tally_protocol::native_statement_reports::NativeStatementKind;

/// Unclassified ledgers returned in full up to this many; the count is always
/// complete.
const MAX_UNCLASSIFIED_RETURNED: usize = 100;

impl Server {
    pub(super) async fn profit_and_loss(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        self.statement(args, NativeStatementKind::ProfitAndLoss, "profit_and_loss")
            .await
    }

    pub(super) async fn balance_sheet(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        self.statement(args, NativeStatementKind::BalanceSheet, "balance_sheet")
            .await
    }

    async fn statement(
        &self,
        args: &Value,
        kind: NativeStatementKind,
        tool: &str,
    ) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        let from =
            bridge_tally_core::TallyDate::parse(from).map_err(|_| "invalid_date".to_string())?;
        let to = bridge_tally_core::TallyDate::parse(to).map_err(|_| "invalid_date".to_string())?;
        let period = crate::tally::runtime::TrialBalancePeriod::new(from, to)
            .map_err(|_| "invalid_date_range".to_string())?;
        let (company, identity, prior) = self.verified_company(guid).await?;
        let read = self
            .runtime
            .fetch_statements(self.tally_config(), &identity, period, kind)
            .await
            .map_err(|error| {
                ToolFailure::from_runtime(&format!("{tool}_read_failed"), error)
                    .with_prior_evidence(prior.clone())
            })?;
        let trial_balance = read.trial_balance;
        let evidence = combine_evidence(prior, evidence_from_runtime_read(trial_balance.evidence));
        let derived = read.derived;
        let (lines, headline) = match kind {
            NativeStatementKind::ProfitAndLoss => (
                &derived.profit_and_loss,
                json!({
                    "gross_result": derived.gross_result,
                    "net_result": derived.net_result,
                }),
            ),
            NativeStatementKind::BalanceSheet => (
                &derived.balance_sheet,
                json!({
                    "profit_and_loss": {
                        "ledger": derived.profit_and_loss_ledger.as_ref().map(|ledger| json!({
                            "name": party_name(ledger.name.clone()),
                            "closing": ledger.closing,
                        })),
                        "carried": derived.balance_sheet_profit_and_loss,
                    },
                }),
            ),
        };
        let unclassified_total = derived.unclassified.len();
        let unclassified = derived
            .unclassified
            .into_iter()
            .take(MAX_UNCLASSIFIED_RETURNED)
            .map(|ledger| {
                json!({
                    "ledger": party_name(ledger.name), "guid": ledger.guid,
                    "reason": ledger.reason, "debit": ledger.debit,
                    "credit": ledger.credit, "closing": ledger.closing,
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolOutcome {
            payload: json!({
                "company": company_json(&company, std::slice::from_ref(&company)),
                "result": {
                    "state": "observed",
                    "basis": "tally_native_trial_balance_classified_by_reserved_primary_group",
                    "from": trial_balance.from, "to": trial_balance.to,
                    "currency": trial_balance.currency, "read_at": trial_balance.read_at,
                    "lines": lines,
                    "result": headline,
                    "unclassified": unclassified,
                    "unclassified_total": unclassified_total,
                    "stock_ledger_count": derived.stock_ledger_count,
                    "tie_out": read.tie_out,
                    "verification": "stable_paired_sources_with_company_mode_and_extent_guards",
                    "limitations": [
                        "Each line sums Trial Balance amounts under one reserved primary group; empty amounts are excluded and counted, never read as zero",
                        "A ledger under a user-created primary group, or with an incomplete group chain, is listed in unclassified; while any carries an amount, no result is established",
                        "Closing stock is not derived: with a Stock-in-Hand balance, gross and net results are not established",
                        "tie_out compares Tally's own statement lines by display name and enforces nothing; Tally's carried Profit & Loss line has been compared only over a full year",
                        "Not voucher-level reconciliation or an atomic snapshot",
                    ],
                },
            }),
            evidence,
            company_guid: Some(guid.to_string()),
            truncated: unclassified_total > MAX_UNCLASSIFIED_RETURNED,
        })
    }
}
