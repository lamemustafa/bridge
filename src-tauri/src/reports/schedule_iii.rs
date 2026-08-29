//! A deliberately partial, traceable Schedule III view over an already read
//! party/ledger master source. No Tally I/O belongs in this module.

use std::collections::{BTreeMap, BTreeSet};

use bridge_tally_core::ExactDecimal;

use super::party_ledger_master::{
    PartyLedgerMasterGroup, PartyLedgerMasterRow, PartyLedgerMasterSource,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleIIIView {
    pub(crate) lines: Vec<ScheduleIIILine>,
    pub(crate) exclusions: Vec<ScheduleIIIExclusion>,
    pub(crate) debit_total: ExactDecimal,
    pub(crate) credit_total: ExactDecimal,
    pub(crate) difference: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleIIILine {
    pub(crate) section: &'static str,
    pub(crate) label: &'static str,
    pub(crate) total: ExactDecimal,
    /// Indices into `PartyLedgerMasterSource::rows`, so each subtotal keeps a
    /// direct link to the original named ledger row and exact source balance.
    pub(crate) row_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduleIIIExclusion {
    pub(crate) row_index: usize,
    pub(crate) reason: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ScheduleIIIError {
    #[error("Schedule III totals exceeded the exact-decimal range")]
    Arithmetic,
    #[error("an unestablished closing balance reached a Schedule III total")]
    UnestablishedBalance,
}

/// Returns only classifications determined by a Tally built-in group identity.
/// A total is intentionally labelled as a *group-derived subtotal* whenever
/// Schedule III requires a client decision (for example, a current/non-current
/// maturity split). Every other ledger is listed loudly as an exclusion.
pub(crate) fn build_schedule_iii_view(
    source: &PartyLedgerMasterSource,
) -> Result<ScheduleIIIView, ScheduleIIIError> {
    let groups = group_index(&source.groups);
    let mut line_rows = BTreeMap::<(&'static str, &'static str), Vec<usize>>::new();
    let mut exclusions = Vec::new();
    let mut debit_total = ExactDecimal::zero();
    let mut credit_total = ExactDecimal::zero();

    for (index, row) in source.rows.iter().enumerate() {
        let Some(closing_balance) = row.closing_balance.as_ref() else {
            exclusions.push(ScheduleIIIExclusion {
                row_index: index,
                reason: "Tally returned an empty CLOSINGBALANCE; the balance is not established and was excluded from Schedule III totals.".to_string(),
            });
            continue;
        };
        if closing_balance.is_negative() {
            debit_total = debit_total
                .checked_add(
                    &closing_balance
                        .abs()
                        .map_err(|_| ScheduleIIIError::Arithmetic)?,
                )
                .map_err(|_| ScheduleIIIError::Arithmetic)?;
        } else {
            credit_total = credit_total
                .checked_add(closing_balance)
                .map_err(|_| ScheduleIIIError::Arithmetic)?;
        }

        match classify(row, closing_balance, &groups) {
            ScheduleIIIClassification::Head(head) => line_rows
                .entry((head.section(), head.label()))
                .or_default()
                .push(index),
            ScheduleIIIClassification::Excluded(reason) => exclusions.push(ScheduleIIIExclusion {
                row_index: index,
                reason,
            }),
        }
    }

    let mut lines = Vec::with_capacity(line_rows.len());
    for ((section, label), row_indices) in line_rows {
        let total = sum_rows(source, &row_indices)?;
        lines.push(ScheduleIIILine {
            section,
            label,
            total,
            row_indices,
        });
    }
    let difference = credit_total
        .checked_subtract(&debit_total)
        .map_err(|_| ScheduleIIIError::Arithmetic)?;
    Ok(ScheduleIIIView {
        lines,
        exclusions,
        debit_total,
        credit_total,
        difference,
    })
}

fn group_index(
    groups: &[PartyLedgerMasterGroup],
) -> BTreeMap<String, Vec<&PartyLedgerMasterGroup>> {
    let mut result = BTreeMap::new();
    for group in groups {
        let key = normalize(&group.name);
        if !key.is_empty() {
            result.entry(key).or_insert_with(Vec::new).push(group);
        }
    }
    result
}

fn classify(
    row: &PartyLedgerMasterRow,
    closing_balance: &ExactDecimal,
    groups: &BTreeMap<String, Vec<&PartyLedgerMasterGroup>>,
) -> ScheduleIIIClassification {
    let Some(parent) = row.parent.as_deref() else {
        return ScheduleIIIClassification::excluded(
            "Ledger has no parent group; its Schedule III head is not determined.",
        );
    };
    let mut current = normalize(parent);
    let mut visited = BTreeSet::new();
    for _ in 0..=groups.len() {
        if current.is_empty() || current == "primary" {
            return ScheduleIIIClassification::excluded(
                "Group hierarchy does not determine a Schedule III head; client mapping decision required.",
            );
        }
        if !visited.insert(current.clone()) {
            return ScheduleIIIClassification::excluded(
                "Group hierarchy contains a cycle; classification withheld.",
            );
        }
        let Some(matches) = groups.get(&current) else {
            return ScheduleIIIClassification::excluded(
                "Ledger parent is absent from the captured group hierarchy; classification withheld.",
            );
        };
        let [group] = matches.as_slice() else {
            return ScheduleIIIClassification::excluded(
                "Captured group hierarchy repeated a group name; classification withheld.",
            );
        };
        let Some(reserved_name) = group.reserved_name.as_deref() else {
            return ScheduleIIIClassification::excluded(
                "Group omitted Tally RESERVEDNAME; immutable classification evidence is unavailable.",
            );
        };
        if reserved_name.is_empty() {
            current = group.parent.as_deref().map(normalize).unwrap_or_default();
            continue;
        }
        match normalize(reserved_name).as_str() {
            "sundry debtors" => return admit_head(
                head::Candidate::debit(
                    "Assets",
                    "Trade receivables (maturity split not determined)",
                    "A credit-balance Sundry Debtors ledger is a customer advance; its Schedule III head is not determined by the group and was excluded.",
                ),
                closing_balance,
            ),
            "sundry creditors" => return admit_head(
                head::Candidate::credit(
                    "Liabilities",
                    "Trade payables (maturity split not determined)",
                    "A debit-balance Sundry Creditors ledger is a supplier advance; its Schedule III head is not determined by the group and was excluded.",
                ),
                closing_balance,
            ),
            "cash-in-hand" | "bank accounts" => return admit_head(
                head::Candidate::debit(
                    "Assets",
                    "Cash and bank balances (Schedule III split not determined)",
                    "A credit-balance Cash-in-Hand or Bank Accounts ledger may be an overdraft; its Schedule III head is not determined by the group and was excluded.",
                ),
                closing_balance,
            ),
            _ => current = group.parent.as_deref().map(normalize).unwrap_or_default(),
        }
    }
    ScheduleIIIClassification::excluded(
        "Group hierarchy exceeded its captured length; classification withheld.",
    )
}

/// A classifier result is always either an admitted Schedule III head or an
/// explicit exclusion. `head::ScheduleIIIHead` cannot be constructed outside
/// the polarity gate, so adding another group branch requires selecting and
/// evaluating its expected balance polarity before it can emit a head.
enum ScheduleIIIClassification {
    Head(head::ScheduleIIIHead),
    Excluded(String),
}

impl ScheduleIIIClassification {
    fn excluded(reason: impl Into<String>) -> Self {
        Self::Excluded(reason.into())
    }
}

fn admit_head(
    candidate: head::Candidate,
    closing_balance: &ExactDecimal,
) -> ScheduleIIIClassification {
    match candidate.admit(closing_balance) {
        Ok(head) => ScheduleIIIClassification::Head(head),
        Err(reason) => ScheduleIIIClassification::Excluded(reason.to_string()),
    }
}

/// The private head type can only be obtained through `Candidate::admit`.
/// Keeping that constructor in a child module makes a group branch unable to
/// create a reportable head directly.
mod head {
    use bridge_tally_core::ExactDecimal;

    #[derive(Clone, Copy)]
    enum RequiredPolarity {
        Debit,
        Credit,
    }

    pub(super) struct Candidate {
        section: &'static str,
        label: &'static str,
        required_polarity: RequiredPolarity,
        contra_reason: &'static str,
    }

    impl Candidate {
        pub(super) fn debit(
            section: &'static str,
            label: &'static str,
            contra_reason: &'static str,
        ) -> Self {
            Self {
                section,
                label,
                required_polarity: RequiredPolarity::Debit,
                contra_reason,
            }
        }

        pub(super) fn credit(
            section: &'static str,
            label: &'static str,
            contra_reason: &'static str,
        ) -> Self {
            Self {
                section,
                label,
                required_polarity: RequiredPolarity::Credit,
                contra_reason,
            }
        }

        pub(super) fn admit(
            self,
            closing_balance: &ExactDecimal,
        ) -> Result<ScheduleIIIHead, &'static str> {
            let admitted = closing_balance.is_zero()
                || match self.required_polarity {
                    RequiredPolarity::Debit => closing_balance.is_negative(),
                    RequiredPolarity::Credit => !closing_balance.is_negative(),
                };
            admitted
                .then_some(ScheduleIIIHead {
                    section: self.section,
                    label: self.label,
                })
                .ok_or(self.contra_reason)
        }
    }

    pub(super) struct ScheduleIIIHead {
        section: &'static str,
        label: &'static str,
    }

    impl ScheduleIIIHead {
        pub(super) fn section(&self) -> &'static str {
            self.section
        }

        pub(super) fn label(&self) -> &'static str {
            self.label
        }
    }
}

fn sum_rows(
    source: &PartyLedgerMasterSource,
    indices: &[usize],
) -> Result<ExactDecimal, ScheduleIIIError> {
    indices
        .iter()
        .try_fold(ExactDecimal::zero(), |total, index| {
            total
                .checked_add(
                    source.rows[*index]
                        .closing_balance
                        .as_ref()
                        .ok_or(ScheduleIIIError::UnestablishedBalance)?,
                )
                .map_err(|_| ScheduleIIIError::Arithmetic)
        })
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use bridge_tally_core::{ExactDecimal, TallyDate};
    use bridge_tally_protocol::PartyLedgerMasterFields;

    use super::*;
    use crate::tally::OutstandingsCurrencyAssertion;

    fn row(name: &str, parent: &str, balance: &str) -> PartyLedgerMasterRow {
        PartyLedgerMasterRow {
            name: name.to_string(),
            parent: Some(parent.to_string()),
            party_gstin: bridge_tally_protocol::PartyLedgerMasterFieldObservation::NotObserved,
            fields: PartyLedgerMasterFields::default(),
            guid: format!("guid-{name}"),
            master_id: format!("id-{name}"),
            alter_id: "1".to_string(),
            opening_balance: ExactDecimal::zero(),
            closing_balance: Some(ExactDecimal::parse(balance).unwrap()),
        }
    }

    #[test]
    fn maps_only_immutable_group_evidence_and_lists_everything_else() {
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260331").unwrap(),
            rows: vec![
                row("Customer", "Regional customers", "-100"),
                row("Unknown", "Custom", "100"),
            ],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 1,
            balance_response_bytes: 1,
            group_response_bytes: 1,
            groups: vec![
                PartyLedgerMasterGroup {
                    name: "Regional customers".to_string(),
                    parent: Some("Renamed debtor root".to_string()),
                    reserved_name: Some("".to_string()),
                },
                PartyLedgerMasterGroup {
                    name: "Renamed debtor root".to_string(),
                    parent: Some("Primary".to_string()),
                    reserved_name: Some("Sundry Debtors".to_string()),
                },
                PartyLedgerMasterGroup {
                    name: "Custom".to_string(),
                    parent: Some("Primary".to_string()),
                    reserved_name: Some("".to_string()),
                },
            ],
        };
        let view = build_schedule_iii_view(&source).unwrap();
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].row_indices, vec![0]);
        assert_eq!(view.exclusions.len(), 1);
        assert!(view.exclusions[0].reason.contains("mapping decision"));
        assert!(view.difference.is_zero());
    }

    #[test]
    fn contra_signed_sundry_debtor_is_excluded_not_netted_against_trade_receivables() {
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![
                row("Customer advance", "Sundry Debtors", "100"),
                row("Receivable", "Sundry Debtors", "-300"),
            ],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 1,
            balance_response_bytes: 1,
            group_response_bytes: 1,
            groups: vec![PartyLedgerMasterGroup {
                name: "Sundry Debtors".to_string(),
                parent: Some("Primary".to_string()),
                reserved_name: Some("Sundry Debtors".to_string()),
            }],
        };

        let view = build_schedule_iii_view(&source).unwrap();
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].row_indices, vec![1]);
        assert_eq!(view.lines[0].total.as_str(), "-300");
        assert_eq!(view.exclusions.len(), 1);
        assert!(view.exclusions[0]
            .reason
            .contains("credit-balance Sundry Debtors"));
    }

    #[test]
    fn contra_signed_sundry_creditor_is_excluded_not_netted_against_trade_payables() {
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![
                row("Supplier advance", "Sundry Creditors", "-200"),
                row("Payable", "Sundry Creditors", "300"),
            ],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 1,
            balance_response_bytes: 1,
            group_response_bytes: 1,
            groups: vec![PartyLedgerMasterGroup {
                name: "Sundry Creditors".to_string(),
                parent: Some("Primary".to_string()),
                reserved_name: Some("Sundry Creditors".to_string()),
            }],
        };

        let view = build_schedule_iii_view(&source).unwrap();
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].row_indices, vec![1]);
        assert_eq!(view.lines[0].total.as_str(), "300");
        assert_eq!(view.exclusions.len(), 1);
        assert!(view.exclusions[0]
            .reason
            .contains("debit-balance Sundry Creditors"));
    }

    #[test]
    fn contra_signed_bank_account_is_excluded_not_netted_against_cash_and_bank_balances() {
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![
                row("Overdraft", "Bank Accounts", "200"),
                row("Petty cash", "Cash-in-Hand", "-300"),
            ],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 1,
            balance_response_bytes: 1,
            group_response_bytes: 1,
            groups: vec![
                PartyLedgerMasterGroup {
                    name: "Bank Accounts".to_string(),
                    parent: Some("Primary".to_string()),
                    reserved_name: Some("Bank Accounts".to_string()),
                },
                PartyLedgerMasterGroup {
                    name: "Cash-in-Hand".to_string(),
                    parent: Some("Primary".to_string()),
                    reserved_name: Some("Cash-in-Hand".to_string()),
                },
            ],
        };

        let view = build_schedule_iii_view(&source).unwrap();
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].row_indices, vec![1]);
        assert_eq!(view.lines[0].total.as_str(), "-300");
        assert_eq!(view.exclusions.len(), 1);
        assert!(view.exclusions[0]
            .reason
            .contains("credit-balance Cash-in-Hand or Bank Accounts"));
    }

    #[test]
    fn empty_closing_balance_is_excluded_not_manufactured_as_zero() {
        let mut missing = row("Unestablished", "Sundry Debtors", "-1");
        missing.closing_balance = None;
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            currency_assertion: OutstandingsCurrencyAssertion::Inr,
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![missing],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 1,
            balance_response_bytes: 1,
            group_response_bytes: 1,
            groups: vec![],
        };

        let view = build_schedule_iii_view(&source).unwrap();
        assert!(view.lines.is_empty());
        assert!(view.debit_total.is_zero());
        assert!(view.credit_total.is_zero());
        assert_eq!(view.exclusions.len(), 1);
        assert!(view.exclusions[0].reason.contains("not established"));
    }
}
