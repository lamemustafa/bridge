//! Thin MCP presentation of the shared statement read (#692).
use super::*;
use crate::reports::statements::{Established, TieOut};
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
                    "gross_result": established_json(&derived.gross_result),
                    "net_result": established_json(&derived.net_result),
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
                        "carried": established_json(&derived.balance_sheet_profit_and_loss),
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
                    "balance_sheet_gate": tie_json(&derived.balance_sheet_tie),
                    "tie_out": derived.profit_and_loss_tie.as_ref().map(tie_json),
                    "verification": "stable_paired_sources_with_company_mode_and_extent_guards",
                    "limitations": [
                        "Each line sums the Trial Balance amounts Tally returned under one reserved primary group, and counts the empty amounts it left out",
                        "A ledger under a user-created primary group, or with an incomplete group chain, is listed in unclassified; while any carries an amount, no result is established",
                        "Closing stock is not derived: with a Stock-in-Hand balance no result is established",
                        "Every result is established only if Tally's own Balance Sheet for the window ties line for line (balance_sheet_gate); gross and net also need Tally's own Profit and Loss to tie (tie_out), with one heading, Cost of Sales, allowed while it equals the derived cost of sales; a book with stock items is expected to refuse, and no inventory book has been measured",
                        "A book with more than one currency master is refused before the Trial Balance is read",
                        "A Tally line the derivation has no counterpart for, such as a heading with an amount or a difference in opening balances, refuses the results rather than being guessed at",
                        "Tally's own statements carry no company identity; they are bound only by the company, mode and book-extent checks around the read",
                        "The Balance Sheet gate has been measured over one full year on one book and one month on another; a window spanning more than one financial year is unmeasured",
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

/// A result, with any line names that failed the gate masked as party names:
/// a Tally line can name a ledger (its Profit & Loss A/c line does).
fn established_json(result: &Established) -> Value {
    match result {
        Established::Established { value } => json!({"state": "established", "value": value}),
        Established::NotEstablished { reason, lines } => json!({
            "state": "not_established", "reason": reason,
            "lines": lines.iter().cloned().map(party_name).collect::<Vec<_>>(),
        }),
    }
}

/// A tie-out with every line name masked as a party name, for the same reason.
fn tie_json(tie: &TieOut) -> Value {
    json!({
        "lines": tie.lines.iter().map(|line| {
            let mut entry = json!({
                "name": party_name(line.name.clone()),
                "tally_sub": line.tally_sub, "tally_main": line.tally_main,
            });
            // Flattened as in `TieLine`: `status`, and `reason` or `derived`.
            if let (Some(entry), Ok(Value::Object(status))) =
                (entry.as_object_mut(), serde_json::to_value(&line.status))
            {
                entry.extend(status);
            }
            entry
        }).collect::<Vec<_>>(),
        "derived_only": tie.derived_only.iter().cloned().map(party_name).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
#[path = "agent_statements_tests.rs"]
mod tests;
