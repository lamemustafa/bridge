use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::exact_arithmetic::{is_negative_nonzero, numeric_equal, ExactDecimalAccumulator};
use crate::{CoreAccountingBatch, LedgerEntryPolarity, LedgerEntryRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passed,
    Mismatch,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CoreAccountingChecks {
    pub reference_integrity: CheckState,
    pub voucher_entry_balance: CheckState,
    pub voucher_entry_polarity: CheckState,
    pub voucher_entry_applicability: CheckState,
    pub voucher_header_entry_total: CheckState,
}

/// Raw source IDs are intentionally kept in a non-serializable intermediate.
/// The application must replace them with local-only aliases before any
/// operator-visible persistence or export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountingIssue {
    pub safe_reason_code: &'static str,
    pub source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreAccountingAssessment {
    pub checks: CoreAccountingChecks,
    pub issues: Vec<AccountingIssue>,
}

pub fn assess_core_accounting(core: &CoreAccountingBatch) -> CoreAccountingAssessment {
    let voucher_types = core
        .voucher_types
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let vouchers = core
        .vouchers
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let ledgers = core
        .ledgers
        .iter()
        .map(|record| record.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut issues = Vec::new();

    push_issue(
        &mut issues,
        "voucher_type_reference_missing",
        core.vouchers
            .iter()
            .filter(|record| !voucher_types.contains(record.voucher_type_source_id.as_str()))
            .map(|record| record.source_id.clone())
            .collect(),
    );
    push_issue(
        &mut issues,
        "voucher_reference_missing",
        core.ledger_entries
            .iter()
            .filter(|record| !vouchers.contains(record.voucher_source_id.as_str()))
            .map(|record| record.source_id.clone())
            .collect(),
    );
    push_issue(
        &mut issues,
        "ledger_reference_missing",
        core.ledger_entries
            .iter()
            .filter(|record| !ledgers.contains(record.ledger_source_id.as_str()))
            .map(|record| record.source_id.clone())
            .collect(),
    );
    let reference_integrity = state_for_codes(
        &issues,
        &[
            "voucher_type_reference_missing",
            "voucher_reference_missing",
            "ledger_reference_missing",
        ],
    );

    let excluded_from_books = core
        .vouchers
        .iter()
        .filter(|voucher| voucher.cancelled || voucher.optional)
        .map(|voucher| voucher.source_id.as_str())
        .collect::<BTreeSet<_>>();

    push_issue(
        &mut issues,
        "voucher_entry_polarity_mismatch",
        inverted_voucher_entries(core, &excluded_from_books),
    );
    let voucher_entry_polarity =
        if state_for_codes(&issues, &["voucher_entry_polarity_mismatch"]) == CheckState::Mismatch {
            CheckState::Mismatch
        } else if core
            .ledger_entries
            .iter()
            .filter(|entry| !excluded_from_books.contains(entry.voucher_source_id.as_str()))
            .any(entry_amount_is_zero)
        {
            CheckState::Unavailable
        } else {
            CheckState::Passed
        };

    let mut totals: BTreeMap<&str, ExactDecimalAccumulator> = BTreeMap::new();
    for entry in &core.ledger_entries {
        if !excluded_from_books.contains(entry.voucher_source_id.as_str()) {
            totals
                .entry(entry.voucher_source_id.as_str())
                .or_default()
                .add(entry.amount.as_str());
        }
    }
    push_issue(
        &mut issues,
        "voucher_entries_unbalanced",
        core.vouchers
            .iter()
            .filter(|voucher| !voucher.cancelled && !voucher.optional)
            .filter(|voucher| {
                totals
                    .get(voucher.source_id.as_str())
                    .is_some_and(|total| !total.is_zero())
            })
            .map(|voucher| voucher.source_id.clone())
            .collect(),
    );
    let voucher_entry_balance = state_for_codes(&issues, &["voucher_entries_unbalanced"]);
    let voucher_entry_applicability = if core
        .vouchers
        .iter()
        .filter(|voucher| !voucher.cancelled && !voucher.optional)
        .any(|voucher| !totals.contains_key(voucher.source_id.as_str()))
    {
        CheckState::Unavailable
    } else {
        CheckState::Passed
    };

    CoreAccountingAssessment {
        checks: CoreAccountingChecks {
            reference_integrity,
            voucher_entry_balance,
            voucher_entry_polarity,
            voucher_entry_applicability,
            // The Core v2 model has no independently observed voucher header
            // total. Absence is a gap, never an inferred pass.
            voucher_header_entry_total: CheckState::Unavailable,
        },
        issues,
    }
}

fn push_issue(
    issues: &mut Vec<AccountingIssue>,
    safe_reason_code: &'static str,
    mut source_ids: Vec<String>,
) {
    if source_ids.is_empty() {
        return;
    }
    source_ids.sort();
    source_ids.dedup();
    issues.push(AccountingIssue {
        safe_reason_code,
        source_ids,
    });
}

fn state_for_codes(issues: &[AccountingIssue], codes: &[&str]) -> CheckState {
    if issues
        .iter()
        .any(|issue| codes.contains(&issue.safe_reason_code))
    {
        CheckState::Mismatch
    } else {
        CheckState::Passed
    }
}

/// The entries of every voucher whose polarity is inverted *as a whole*.
///
/// A single entry disagreeing with its amount sign is not a defect. Tally's
/// `ISDEEMEDPOSITIVE` records the column an entry was made in, not its
/// arithmetic sign, and the two legitimately diverge — this document's own
/// protocol reference already records that bill-allocation polarity is
/// "contextual, not an amount-sign invariant", measured in both directions,
/// and outstandings deliberately does not validate one against the other.
///
/// Measured on twelve monthly windows of a real trading book, by two
/// independent parsers: **on the order of 110 of ~6,900 entries** disagree
/// (111 and 104 respectively -- the ~5% gap is unresolved and is most likely
/// XML-parsing edge cases, so treat the count as approximate). Both passes
/// agree exactly on the shape that matters: the disagreement is **always
/// exactly one entry** of a voucher four to six wide, in both directions, and
/// **never** the whole voucher. Flagging each was ~110 reconciliation
/// mismatches on an ordinary book, which is how a check teaches its reader to
/// ignore it.
///
/// What the check is actually for survives: a voucher can sum to zero while
/// every entry's polarity is inverted, which the balance check cannot see
/// (`numeric_zero_sum_cannot_hide_contradictory_tally_polarity`). That shape is
/// systematic and cannot be explained by contextual polarity, so it is still
/// reported. On the same real book it occurs **zero** times.
///
/// `polarity` itself is unchanged and still comes from the flag. It is read
/// here and in `transport_qualification.rs`, which projects it into a transport
/// parity fingerprint; no accounting total is derived from it.
fn inverted_voucher_entries(
    core: &CoreAccountingBatch,
    excluded_from_books: &BTreeSet<&str>,
) -> Vec<String> {
    // Judge only the entries that carry a sign. A zero amount agrees with
    // either polarity by definition -- `entry_polarity_matches_amount` returns
    // true for every one of them -- so folding them in would let a single zero
    // leg prove the voucher "not wholly inverted" and silently disable this
    // check for that voucher. Zero legs are ordinary: 227 of 6,854 entries in
    // the captured year. They are counted in `judged` nowhere, so a voucher of
    // nothing but zero amounts reports nothing rather than everything.
    struct Tally {
        judged: usize,
        inverted: usize,
        entries: Vec<String>,
    }
    let mut by_voucher: BTreeMap<&str, Tally> = BTreeMap::new();
    for entry in core
        .ledger_entries
        .iter()
        .filter(|entry| !excluded_from_books.contains(entry.voucher_source_id.as_str()))
    {
        let slot = by_voucher
            .entry(entry.voucher_source_id.as_str())
            .or_insert(Tally {
                judged: 0,
                inverted: 0,
                entries: Vec::new(),
            });
        if !entry_amount_is_zero(entry) {
            slot.judged += 1;
            if !entry_polarity_matches_amount(entry) {
                slot.inverted += 1;
            }
        }
        slot.entries.push(entry.source_id.clone());
    }
    by_voucher
        .into_values()
        .filter(|tally| tally.judged > 0 && tally.inverted == tally.judged)
        .flat_map(|tally| tally.entries)
        .collect()
}

fn entry_polarity_matches_amount(entry: &LedgerEntryRecord) -> bool {
    let zero = numeric_equal(entry.amount.as_str(), "0");
    let negative = is_negative_nonzero(entry.amount.as_str());
    zero || matches!(
        (entry.polarity, negative),
        (LedgerEntryPolarity::Debit, true) | (LedgerEntryPolarity::Credit, false)
    )
}

fn entry_amount_is_zero(entry: &LedgerEntryRecord) -> bool {
    numeric_equal(entry.amount.as_str(), "0")
}

#[cfg(test)]
#[path = "reconciliation_tests.rs"]
mod tests;
