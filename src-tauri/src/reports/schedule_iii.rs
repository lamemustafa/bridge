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

        match classify(row, &groups) {
            Ok(Some(line)) => line_rows.entry(line).or_default().push(index),
            Ok(None) => exclusions.push(ScheduleIIIExclusion {
                row_index: index,
                reason: "Group hierarchy does not determine a Schedule III head; client mapping decision required.".to_string(),
            }),
            Err(reason) => exclusions.push(ScheduleIIIExclusion { row_index: index, reason }),
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
    groups: &BTreeMap<String, Vec<&PartyLedgerMasterGroup>>,
) -> Result<Option<(&'static str, &'static str)>, String> {
    let Some(parent) = row.parent.as_deref() else {
        return Err(
            "Ledger has no parent group; its Schedule III head is not determined.".to_string(),
        );
    };
    let mut current = normalize(parent);
    let mut visited = BTreeSet::new();
    for _ in 0..=groups.len() {
        if current.is_empty() || current == "primary" {
            return Ok(None);
        }
        if !visited.insert(current.clone()) {
            return Err("Group hierarchy contains a cycle; classification withheld.".to_string());
        }
        let Some(matches) = groups.get(&current) else {
            return Err("Ledger parent is absent from the captured group hierarchy; classification withheld.".to_string());
        };
        let [group] = matches.as_slice() else {
            return Err(
                "Captured group hierarchy repeated a group name; classification withheld."
                    .to_string(),
            );
        };
        let Some(reserved_name) = group.reserved_name.as_deref() else {
            return Err("Group omitted Tally RESERVEDNAME; immutable classification evidence is unavailable.".to_string());
        };
        if reserved_name.is_empty() {
            current = group.parent.as_deref().map(normalize).unwrap_or_default();
            continue;
        }
        match normalize(reserved_name).as_str() {
            "sundry debtors" => {
                if !row
                    .closing_balance
                    .as_ref()
                    .is_some_and(ExactDecimal::is_negative)
                    && !row
                        .closing_balance
                        .as_ref()
                        .is_some_and(ExactDecimal::is_zero)
                {
                    return Err("A credit-balance Sundry Debtors ledger is a customer advance; its Schedule III head is not determined by the group and was excluded.".to_string());
                }
                return Ok(Some((
                    "Assets",
                    "Trade receivables (maturity split not determined)",
                )));
            }
            "sundry creditors" => {
                if row
                    .closing_balance
                    .as_ref()
                    .is_some_and(ExactDecimal::is_negative)
                {
                    return Err("A debit-balance Sundry Creditors ledger is a supplier advance; its Schedule III head is not determined by the group and was excluded.".to_string());
                }
                return Ok(Some((
                    "Liabilities",
                    "Trade payables (maturity split not determined)",
                )));
            }
            "cash-in-hand" | "bank accounts" => {
                return Ok(Some((
                    "Assets",
                    "Cash and bank balances (Schedule III split not determined)",
                )))
            }
            _ => current = group.parent.as_deref().map(normalize).unwrap_or_default(),
        }
    }
    Err("Group hierarchy exceeded its captured length; classification withheld.".to_string())
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

    use super::*;

    fn row(name: &str, parent: &str, balance: &str) -> PartyLedgerMasterRow {
        PartyLedgerMasterRow {
            name: name.to_string(),
            parent: Some(parent.to_string()),
            party_gstin: None,
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
    fn contra_signed_party_ledgers_are_excluded_not_net_against_their_group_head() {
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
            from: TallyDate::parse("20260401").unwrap(),
            to: TallyDate::parse("20260731").unwrap(),
            rows: vec![
                row("Customer advance", "Sundry Debtors", "100"),
                row("Supplier advance", "Sundry Creditors", "-200"),
                row("Receivable", "Sundry Debtors", "-300"),
            ],
            master_response_sha256: "a".repeat(64),
            balance_response_sha256: "b".repeat(64),
            group_response_sha256: "c".repeat(64),
            master_response_bytes: 1,
            balance_response_bytes: 1,
            group_response_bytes: 1,
            groups: vec![
                PartyLedgerMasterGroup {
                    name: "Sundry Debtors".to_string(),
                    parent: Some("Primary".to_string()),
                    reserved_name: Some("Sundry Debtors".to_string()),
                },
                PartyLedgerMasterGroup {
                    name: "Sundry Creditors".to_string(),
                    parent: Some("Primary".to_string()),
                    reserved_name: Some("Sundry Creditors".to_string()),
                },
            ],
        };

        let view = build_schedule_iii_view(&source).unwrap();
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].row_indices, vec![2]);
        assert_eq!(view.lines[0].total.as_str(), "-300");
        assert_eq!(view.exclusions.len(), 2);
        assert!(view
            .exclusions
            .iter()
            .all(|entry| entry.reason.contains("advance")));
    }

    #[test]
    fn empty_closing_balance_is_excluded_not_manufactured_as_zero() {
        let mut missing = row("Unestablished", "Sundry Debtors", "-1");
        missing.closing_balance = None;
        let source = PartyLedgerMasterSource {
            company: "Synthetic Books".to_string(),
            company_guid: "company-guid".to_string(),
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
