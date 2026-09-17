//! Exact summaries of the native amounts already captured by the runtime.
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::{
    native_trial_balance::{NativeTrialBalance, NativeTrialBalanceAmount, NativeTrialBalanceRow},
    PartyLedgerMasterFieldObservation,
};
use std::borrow::Cow;

use serde::Serialize;

const MAX_PARENT_OPTIONS: usize = 100;
const PARENT_OPTION_OVERFLOW: usize = 1;
const MAX_RETAINED_PARENT_CANDIDATES: usize = MAX_PARENT_OPTIONS + PARENT_OPTION_OVERFLOW;
// TrialBalanceExportStore admits no retained cell text beyond this budget. A
// returned parent can therefore have this many bytes; its explicit selector
// label adds the longest display prefix below.
const MAX_RETAINED_PARENT_BYTES: usize = 4_096;
const GROUP_LABEL_PREFIX: &str = "Group: ";
const MAX_PARENT_DISPLAY_BYTES: usize = MAX_RETAINED_PARENT_BYTES + GROUP_LABEL_PREFIX.len();
const NOT_RETURNED_PARENT_LABEL: &str = "Missing field: Parent not returned";
const RETURNED_EMPTY_PARENT_LABEL: &str = "Empty field: Parent returned empty";

#[derive(Debug, Clone, Serialize)]
pub struct ObservedAmountTotal {
    pub sum: ExactDecimal,
    pub empty_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialBalanceTotals {
    pub opening: ObservedAmountTotal,
    pub debit: ObservedAmountTotal,
    pub credit: ObservedAmountTotal,
    pub closing: ObservedAmountTotal,
}

/// A case-preserving follow-up over one already retained Trial Balance capture.
/// It is a row subset, not a qualified financial group balance.
#[derive(Debug, Clone, Serialize)]
pub struct TrialBalanceParentQuery {
    pub parent: PartyLedgerMasterFieldObservation,
    pub selected_rows: Vec<NativeTrialBalanceRow>,
    pub totals: TrialBalanceTotals,
    pub source_row_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialBalanceParentQueryError {
    ParentNotInCapture,
    TotalsUnavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialBalanceCaptureParentOption {
    pub parent: PartyLedgerMasterFieldObservation,
    pub row_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrialBalanceCaptureParentOptions {
    pub options: Vec<TrialBalanceCaptureParentOption>,
    pub has_more: bool,
    pub source_row_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialBalanceCaptureParentOptionsError {
    SearchInvalid,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ParentMatchRank {
    Exact = 0,
    FoldedExact = 1,
    FoldedSubstring = 2,
}

struct ParentSearch {
    raw: String,
    lowercase: String,
}

impl ParentSearch {
    fn new(raw: String) -> Result<Self, TrialBalanceCaptureParentOptionsError> {
        if raw.len() > MAX_PARENT_DISPLAY_BYTES {
            return Err(TrialBalanceCaptureParentOptionsError::SearchInvalid);
        }
        Ok(Self {
            lowercase: raw.to_lowercase(),
            raw,
        })
    }

    fn rank(&self, parent: &PartyLedgerMasterFieldObservation) -> Option<ParentMatch> {
        // Initial discovery preserves distinct-observation source order and
        // avoids display/lowercase allocations for every retained row.
        if self.raw.is_empty() {
            return Some(ParentMatch::source_order());
        }
        let display = parent_display_label(parent);
        if parent.returned_text() == Some(self.raw.as_str()) || display == self.raw {
            let display_owned_len = match display {
                Cow::Borrowed(_) => 0,
                Cow::Owned(ref value) => value.len(),
            };
            return Some(ParentMatch::exact(display_owned_len));
        }
        let raw_lowercase = parent.returned_text().map(str::to_lowercase);
        let display_lowercase = display.to_lowercase();
        #[cfg(test)]
        let temporary_text_bytes = match &display {
            Cow::Borrowed(_) => 0,
            Cow::Owned(value) => value.len(),
        } + raw_lowercase.as_ref().map_or(0, String::len)
            + display_lowercase.len();
        if raw_lowercase.as_deref() == Some(self.lowercase.as_str())
            || display_lowercase == self.lowercase
        {
            return Some(ParentMatch {
                rank: ParentMatchRank::FoldedExact,
                #[cfg(test)]
                temporary_text_bytes,
            });
        }
        (raw_lowercase
            .as_deref()
            .is_some_and(|raw| raw.contains(&self.lowercase))
            || display_lowercase.contains(&self.lowercase))
        .then_some(ParentMatch {
            rank: ParentMatchRank::FoldedSubstring,
            #[cfg(test)]
            temporary_text_bytes,
        })
    }
}

struct ParentMatch {
    rank: ParentMatchRank,
    #[cfg(test)]
    temporary_text_bytes: usize,
}

impl ParentMatch {
    fn source_order() -> Self {
        Self {
            rank: ParentMatchRank::FoldedSubstring,
            #[cfg(test)]
            temporary_text_bytes: 0,
        }
    }

    fn exact(_display_owned_len: usize) -> Self {
        Self {
            rank: ParentMatchRank::Exact,
            #[cfg(test)]
            temporary_text_bytes: _display_owned_len,
        }
    }
}

fn parent_display_label(parent: &PartyLedgerMasterFieldObservation) -> Cow<'static, str> {
    match parent {
        PartyLedgerMasterFieldObservation::NotObserved => Cow::Borrowed(NOT_RETURNED_PARENT_LABEL),
        PartyLedgerMasterFieldObservation::Returned(value) if value.is_empty() => {
            Cow::Borrowed(RETURNED_EMPTY_PARENT_LABEL)
        }
        PartyLedgerMasterFieldObservation::Returned(value) => {
            Cow::Owned(format!("{GROUP_LABEL_PREFIX}{value}"))
        }
    }
}

struct ParentCandidate<'a> {
    parent: &'a PartyLedgerMasterFieldObservation,
    rank: ParentMatchRank,
    first_index: usize,
    row_count: usize,
}

impl ParentCandidate<'_> {
    fn is_better_than(&self, other: &Self) -> bool {
        (self.rank, self.first_index) < (other.rank, other.first_index)
    }
}

struct ParentOptionScan<'a> {
    candidates: Vec<ParentCandidate<'a>>,
    has_more: bool,
    #[cfg(test)]
    max_retained_candidates: usize,
    #[cfg(test)]
    max_retained_parent_text_bytes: usize,
    #[cfg(test)]
    max_temporary_text_bytes: usize,
}

impl<'a> ParentOptionScan<'a> {
    fn new() -> Self {
        Self {
            candidates: Vec::with_capacity(MAX_RETAINED_PARENT_CANDIDATES),
            has_more: false,
            #[cfg(test)]
            max_retained_candidates: 0,
            #[cfg(test)]
            max_retained_parent_text_bytes: 0,
            #[cfg(test)]
            max_temporary_text_bytes: 0,
        }
    }

    fn retain(
        &mut self,
        parent: &'a PartyLedgerMasterFieldObservation,
        parent_match: ParentMatch,
        index: usize,
    ) {
        #[cfg(test)]
        {
            self.max_temporary_text_bytes = self
                .max_temporary_text_bytes
                .max(parent_match.temporary_text_bytes);
        }
        if let Some(existing) = self
            .candidates
            .iter_mut()
            .find(|candidate| candidate.parent == parent)
        {
            existing.row_count += 1;
            return;
        }
        let candidate = ParentCandidate {
            parent,
            rank: parent_match.rank,
            first_index: index,
            row_count: 1,
        };
        if self.candidates.len() < MAX_RETAINED_PARENT_CANDIDATES {
            self.candidates.push(candidate);
        } else {
            let worst = self
                .candidates
                .iter()
                .enumerate()
                .max_by_key(|(_, candidate)| (candidate.rank, candidate.first_index))
                .expect("nonempty bounded candidate set")
                .0;
            if candidate.is_better_than(&self.candidates[worst]) {
                self.candidates[worst] = candidate;
            }
            self.has_more = true;
        }
        #[cfg(test)]
        {
            self.max_retained_candidates = self.max_retained_candidates.max(self.candidates.len());
            self.max_retained_parent_text_bytes = self.max_retained_parent_text_bytes.max(
                self.candidates
                    .iter()
                    .map(|candidate| candidate.parent.workbook_text().len())
                    .sum(),
            );
        }
    }

    fn finish(mut self, source_row_count: usize) -> TrialBalanceCaptureParentOptions {
        self.candidates
            .sort_by_key(|candidate| (candidate.rank, candidate.first_index));
        let has_more = self.has_more || self.candidates.len() > MAX_PARENT_OPTIONS;
        TrialBalanceCaptureParentOptions {
            options: self
                .candidates
                .into_iter()
                .take(MAX_PARENT_OPTIONS)
                .map(|candidate| TrialBalanceCaptureParentOption {
                    parent: candidate.parent.clone(),
                    row_count: candidate.row_count,
                })
                .collect(),
            has_more,
            source_row_count,
        }
    }
}

/// Lists at most 100 distinct observed parents from one retained capture. The
/// scan borrows retained row text and keeps at most one overflow candidate;
/// it never builds a parent index or acquires a new Tally response.
pub fn list_observed_capture_parents(
    report: &NativeTrialBalance,
    search: String,
) -> Result<TrialBalanceCaptureParentOptions, TrialBalanceCaptureParentOptionsError> {
    let search = ParentSearch::new(search)?;
    Ok(scan_observed_capture_parents(report, &search).finish(report.rows.len()))
}

fn scan_observed_capture_parents<'a>(
    report: &'a NativeTrialBalance,
    search: &ParentSearch,
) -> ParentOptionScan<'a> {
    let mut scan = ParentOptionScan::new();
    for (index, row) in report.rows.iter().enumerate() {
        if let Some(parent_match) = search.rank(&row.parent) {
            scan.retain(&row.parent, parent_match, index);
        }
    }
    scan
}

/// These are sums of numeric observations. Empty native fields remain counted
/// separately, so a column sum cannot masquerade as fully observed arithmetic.
pub fn observed_totals(
    report: &NativeTrialBalance,
) -> Result<TrialBalanceTotals, bridge_tally_core::TallyError> {
    observed_totals_for_rows(report.rows.iter())
}

fn observed_totals_for_rows<'a>(
    rows: impl IntoIterator<Item = &'a NativeTrialBalanceRow>,
) -> Result<TrialBalanceTotals, bridge_tally_core::TallyError> {
    let mut totals = std::array::from_fn::<_, 4, _>(|_| ObservedAmountTotal {
        sum: ExactDecimal::zero(),
        empty_count: 0,
    });
    for row in rows {
        for (total, amount) in
            totals
                .iter_mut()
                .zip([&row.opening, &row.debit, &row.credit, &row.closing])
        {
            match amount {
                NativeTrialBalanceAmount::Present(value) => {
                    total.sum = total.sum.checked_add(value)?
                }
                NativeTrialBalanceAmount::PresentEmpty => total.empty_count += 1,
            }
        }
    }
    let [opening, debit, credit, closing] = totals;
    Ok(TrialBalanceTotals {
        opening,
        debit,
        credit,
        closing,
    })
}

/// Selects only rows whose source parent observation exactly matches `parent`.
/// `NotObserved` and a returned empty string intentionally remain distinct.
/// See `docs/tally/TALLY_PROTOCOL_REFERENCE.md` §5.6 for the capture contract.
pub fn query_observed_parent(
    report: &NativeTrialBalance,
    parent: &PartyLedgerMasterFieldObservation,
) -> Result<TrialBalanceParentQuery, TrialBalanceParentQueryError> {
    let selected_rows = report
        .rows
        .iter()
        .filter(|row| row.parent == *parent)
        .cloned()
        .collect::<Vec<_>>();
    if selected_rows.is_empty() {
        return Err(TrialBalanceParentQueryError::ParentNotInCapture);
    }
    let totals = observed_totals_for_rows(selected_rows.iter())
        .map_err(|_| TrialBalanceParentQueryError::TotalsUnavailable)?;
    Ok(TrialBalanceParentQuery {
        parent: parent.clone(),
        selected_rows,
        totals,
        source_row_count: report.rows.len(),
    })
}

#[cfg(test)]
#[path = "trial_balance_tests.rs"]
mod tests;
