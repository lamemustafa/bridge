//! A deliberately partial, traceable Schedule III view over an already read
//! party/ledger master source. No Tally I/O belongs in this module.

use std::collections::BTreeMap;

use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::group_ancestry::{AncestryGap, GroupIndex};
#[cfg(test)]
use bridge_tally_protocol::TallyNamedMaster;

use super::party_ledger_master::{PartyLedgerMasterRow, PartyLedgerMasterSource};

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

/// Returns only evidence-backed Tally group subtotals. The available reads
/// establish built-in group identity and balance polarity, but not the
/// voucher-level commercial purpose or liquidity facts needed for a Schedule
/// III head. Every other ledger is listed loudly as an exclusion.
pub(crate) fn build_schedule_iii_view(
    source: &PartyLedgerMasterSource,
) -> Result<ScheduleIIIView, ScheduleIIIError> {
    let groups = GroupIndex::build(source.groups.iter().cloned());
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
            ScheduleIIIClassification::GroupSubtotal(subtotal) => line_rows
                .entry((subtotal.section(), subtotal.label()))
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

/// Decides a Schedule III head from a ledger's predefined group ancestry.
///
/// The climb itself is shared with the bank-voucher classifier, which needs
/// the same traversal for a different verdict — see
/// [`bridge_tally_protocol::group_ancestry`]. What is Schedule III's own is
/// which reserved identities map to a head, the polarity gate below, and how
/// each refusal reads to someone holding a trial balance.
fn classify(
    row: &PartyLedgerMasterRow,
    closing_balance: &ExactDecimal,
    groups: &GroupIndex,
) -> ScheduleIIIClassification {
    let reserved = match groups.reserved_ancestor(row.parent.nonempty_returned_text()) {
        Ok(reserved) => reserved,
        Err(gap) => return ScheduleIIIClassification::excluded(exclusion(gap)),
    };
    match normalize(reserved).as_str() {
        "sundry debtors" => admit_group_subtotal(
            subtotal::Candidate::debit(
                "Debit-balance group subtotals",
                "Sundry Debtors group subtotal",
                "A credit-balance Sundry Debtors ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            ),
            closing_balance,
        ),
        "sundry creditors" => admit_group_subtotal(
            subtotal::Candidate::credit(
                "Credit-balance group subtotals",
                "Sundry Creditors group subtotal",
                "A debit-balance Sundry Creditors ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            ),
            closing_balance,
        ),
        "cash-in-hand" => admit_group_subtotal(
            subtotal::Candidate::debit(
                "Debit-balance group subtotals",
                "Cash-in-Hand group subtotal",
                "A credit-balance Cash-in-Hand ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            ),
            closing_balance,
        ),
        "bank accounts" => admit_group_subtotal(
            subtotal::Candidate::debit(
                "Debit-balance group subtotals",
                "Bank Accounts group subtotal",
                "A credit-balance Bank Accounts ledger has the opposite polarity; its Schedule III head is not determined by the group and was excluded.",
            ),
            closing_balance,
        ),
        // A predefined identity Schedule III does not map. The ancestry is
        // established; it simply does not name a head this view can fill.
        _ => ScheduleIIIClassification::excluded(
            "Group hierarchy does not determine a Schedule III head; client mapping decision required.",
        ),
    }
}

/// What each shared refusal means to someone reading a Schedule III view.
fn exclusion(gap: AncestryGap) -> &'static str {
    match gap {
        AncestryGap::NoParent => {
            "Ledger has no parent group; its Schedule III head is not determined."
        }
        // Reaching the account root without a predefined identity is the same
        // outcome as a predefined group this view does not map: nothing here
        // determines a head, and a client decides.
        AncestryGap::ReachedRoot => {
            "Group hierarchy does not determine a Schedule III head; client mapping decision required."
        }
        AncestryGap::GroupAbsent => {
            "Ledger parent is absent from the captured group hierarchy; classification withheld."
        }
        AncestryGap::GroupNameRepeated => {
            "Captured group hierarchy repeated a group name; classification withheld."
        }
        AncestryGap::ReservedNameMissing => {
            "Group omitted Tally RESERVEDNAME; immutable classification evidence is unavailable."
        }
        AncestryGap::Cycle => "Group hierarchy contains a cycle; classification withheld.",
        AncestryGap::Exhausted => {
            "Group hierarchy exceeded its captured length; classification withheld."
        }
    }
}

/// A classifier result is always either an evidence-backed group subtotal or
/// an explicit exclusion. `subtotal::GroupSubtotal` cannot be constructed outside
/// the polarity gate, so adding another group branch requires selecting and
/// evaluating its expected balance polarity before it can emit a subtotal.
enum ScheduleIIIClassification {
    GroupSubtotal(subtotal::GroupSubtotal),
    Excluded(String),
}

impl ScheduleIIIClassification {
    fn excluded(reason: impl Into<String>) -> Self {
        Self::Excluded(reason.into())
    }
}

fn admit_group_subtotal(
    candidate: subtotal::Candidate,
    closing_balance: &ExactDecimal,
) -> ScheduleIIIClassification {
    match candidate.admit(closing_balance) {
        Ok(subtotal) => ScheduleIIIClassification::GroupSubtotal(subtotal),
        Err(reason) => ScheduleIIIClassification::Excluded(reason.to_string()),
    }
}

/// The private head type can only be obtained through `Candidate::admit`.
/// Keeping that constructor in a child module makes a group branch unable to
/// create a reportable head directly.
mod subtotal {
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
        ) -> Result<GroupSubtotal, &'static str> {
            let admitted = closing_balance.is_zero()
                || match self.required_polarity {
                    RequiredPolarity::Debit => closing_balance.is_negative(),
                    RequiredPolarity::Credit => !closing_balance.is_negative(),
                };
            admitted
                .then_some(GroupSubtotal {
                    section: self.section,
                    label: self.label,
                })
                .ok_or(self.contra_reason)
        }
    }

    pub(super) struct GroupSubtotal {
        section: &'static str,
        label: &'static str,
    }

    impl GroupSubtotal {
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
#[path = "schedule_iii_tests.rs"]
mod tests;
