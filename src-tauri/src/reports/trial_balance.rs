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
        let temporary_text_bytes = display_owned_len(&display)
            + raw_lowercase.as_ref().map_or(0, String::len)
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

#[cfg(test)]
fn display_owned_len(display: &Cow<'static, str>) -> usize {
    match display {
        Cow::Borrowed(_) => 0,
        Cow::Owned(value) => value.len(),
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
mod tests {
    use super::*;
    use bridge_tally_protocol::native_trial_balance::{
        parse_native_trial_balance, NativeTrialBalance, NativeTrialBalanceAmount,
        NativeTrialBalanceRow,
    };

    fn empty_parent_row(parent: PartyLedgerMasterFieldObservation) -> NativeTrialBalanceRow {
        NativeTrialBalanceRow {
            name: String::new(),
            guid: String::new(),
            parent,
            opening: NativeTrialBalanceAmount::PresentEmpty,
            debit: NativeTrialBalanceAmount::PresentEmpty,
            credit: NativeTrialBalanceAmount::PresentEmpty,
            closing: NativeTrialBalanceAmount::PresentEmpty,
        }
    }

    #[test]
    fn captured_opening_difference_is_retained_without_a_balancing_row() {
        let xml = include_str!("../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_opening_year.xml");
        let report =
            parse_native_trial_balance(xml, "915d42f8-42ae-4b03-8291-55f596e3a2ea").unwrap();
        let totals = observed_totals(&report).unwrap();
        assert_eq!(report.rows.len(), 8);
        assert!(totals
            .opening
            .sum
            .numeric_eq(&ExactDecimal::parse("-49833.50").unwrap()));
        assert_eq!(totals.opening.empty_count, 0);
        assert!(totals.debit.empty_count > 0);
        assert!(totals
            .debit
            .sum
            .checked_add(&totals.credit.sum)
            .unwrap()
            .numeric_eq(&ExactDecimal::zero()));
    }

    #[test]
    fn parent_query_keeps_exact_text_and_distinguishes_empty_from_not_observed() {
        let mut report = parse_native_trial_balance(
            include_str!("../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"),
            "eebb9a9f-1679-4468-9e8f-814c729674cb",
        )
        .unwrap();
        let sundry_debtors = query_observed_parent(
            &report,
            &PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".into()),
        )
        .unwrap();
        assert_eq!(sundry_debtors.source_row_count, 6);
        assert_eq!(sundry_debtors.selected_rows.len(), 3);
        assert!(sundry_debtors
            .totals
            .opening
            .sum
            .numeric_eq(&ExactDecimal::parse("-4400.00").unwrap()));
        assert_eq!(sundry_debtors.totals.opening.empty_count, 0);
        assert!(sundry_debtors
            .totals
            .debit
            .sum
            .numeric_eq(&ExactDecimal::parse("-4777.00").unwrap()));
        assert_eq!(sundry_debtors.totals.debit.empty_count, 2);
        assert!(sundry_debtors
            .totals
            .credit
            .sum
            .numeric_eq(&ExactDecimal::parse("5700.00").unwrap()));
        assert_eq!(sundry_debtors.totals.credit.empty_count, 1);
        assert!(sundry_debtors
            .totals
            .closing
            .sum
            .numeric_eq(&ExactDecimal::parse("-3477.00").unwrap()));
        assert_eq!(sundry_debtors.totals.closing.empty_count, 1);

        let unobserved_guid = report.rows[0].guid.clone();
        let empty_guid = report.rows[1].guid.clone();
        report.rows[0].parent = PartyLedgerMasterFieldObservation::NotObserved;
        report.rows[1].parent = PartyLedgerMasterFieldObservation::Returned(String::new());

        let unobserved =
            query_observed_parent(&report, &PartyLedgerMasterFieldObservation::NotObserved)
                .unwrap();
        assert_eq!(unobserved.selected_rows.len(), 1);
        assert_eq!(unobserved.selected_rows[0].guid, unobserved_guid);

        let empty = query_observed_parent(
            &report,
            &PartyLedgerMasterFieldObservation::Returned(String::new()),
        )
        .unwrap();
        assert_eq!(empty.selected_rows.len(), 1);
        assert_eq!(empty.selected_rows[0].guid, empty_guid);
        assert_ne!(empty.parent, unobserved.parent);

        let marked_parent = report
            .rows
            .iter()
            .find(|row| row.name == "Profit & Loss A/c")
            .unwrap()
            .parent
            .clone();
        assert!(matches!(
            &marked_parent,
            PartyLedgerMasterFieldObservation::Returned(value) if value.starts_with('\u{fffd}')
        ));
        let marked = query_observed_parent(&report, &marked_parent).unwrap();
        assert_eq!(marked.selected_rows.len(), 1);
        assert_eq!(marked.selected_rows[0].parent, marked_parent);
        assert_ne!(
            marked.parent,
            PartyLedgerMasterFieldObservation::Returned("Primary".into())
        );

        assert_eq!(
            query_observed_parent(
                &report,
                &PartyLedgerMasterFieldObservation::Returned("sundry debtors".into()),
            )
            .unwrap_err(),
            TrialBalanceParentQueryError::ParentNotInCapture,
        );
    }

    #[test]
    fn captured_parent_options_rank_without_normalizing_observed_parent_text() {
        let mut report = parse_native_trial_balance(
            include_str!("../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"),
            "eebb9a9f-1679-4468-9e8f-814c729674cb",
        )
        .unwrap();
        let captured = list_observed_capture_parents(&report, "sundry debtors".into()).unwrap();
        assert_eq!(captured.source_row_count, 6);
        assert!(!captured.has_more);
        assert_eq!(captured.options.len(), 1);
        assert_eq!(
            captured.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".into())
        );
        assert_eq!(captured.options[0].row_count, 3);

        report.rows[0].parent = PartyLedgerMasterFieldObservation::Returned("alpha".into());
        report.rows[1].parent = PartyLedgerMasterFieldObservation::Returned("alpha".into());
        report.rows[2].parent = PartyLedgerMasterFieldObservation::Returned("ALPHA".into());
        report.rows[3].parent =
            PartyLedgerMasterFieldObservation::Returned("prefix alpha suffix".into());
        report.rows[4].parent = PartyLedgerMasterFieldObservation::NotObserved;
        report.rows[5].parent = PartyLedgerMasterFieldObservation::Returned(String::new());

        let ranked = list_observed_capture_parents(&report, "alpha".into()).unwrap();
        assert_eq!(ranked.options.len(), 3);
        assert_eq!(ranked.options[0].row_count, 2);
        assert_eq!(
            ranked.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("alpha".into())
        );
        assert_eq!(
            ranked.options[1].parent,
            PartyLedgerMasterFieldObservation::Returned("ALPHA".into())
        );
        assert_eq!(
            ranked.options[2].parent,
            PartyLedgerMasterFieldObservation::Returned("prefix alpha suffix".into())
        );

        let blank = list_observed_capture_parents(&report, String::new()).unwrap();
        assert_eq!(
            blank.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("alpha".into())
        );
        assert!(blank
            .options
            .iter()
            .any(|option| option.parent == PartyLedgerMasterFieldObservation::NotObserved));
        let group = list_observed_capture_parents(&report, "Group: alpha".into()).unwrap();
        assert_eq!(group.options.len(), 2);
        assert_eq!(
            group.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("alpha".into())
        );
        assert_eq!(
            group.options[1].parent,
            PartyLedgerMasterFieldObservation::Returned("ALPHA".into())
        );

        report.rows[2].parent =
            PartyLedgerMasterFieldObservation::Returned(RETURNED_EMPTY_PARENT_LABEL.into());
        report.rows[3].parent =
            PartyLedgerMasterFieldObservation::Returned(NOT_RETURNED_PARENT_LABEL.into());
        let missing =
            list_observed_capture_parents(&report, NOT_RETURNED_PARENT_LABEL.into()).unwrap();
        assert_eq!(missing.options.len(), 2);
        assert_eq!(
            missing.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned(NOT_RETURNED_PARENT_LABEL.into())
        );
        assert_eq!(
            missing.options[1].parent,
            PartyLedgerMasterFieldObservation::NotObserved
        );
        let empty =
            list_observed_capture_parents(&report, RETURNED_EMPTY_PARENT_LABEL.into()).unwrap();
        assert_eq!(empty.options.len(), 2);
        assert_eq!(
            empty.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned(RETURNED_EMPTY_PARENT_LABEL.into())
        );
        assert_eq!(
            empty.options[1].parent,
            PartyLedgerMasterFieldObservation::Returned(String::new())
        );
        let literal_group = list_observed_capture_parents(
            &report,
            format!("{GROUP_LABEL_PREFIX}{NOT_RETURNED_PARENT_LABEL}"),
        )
        .unwrap();
        assert_eq!(literal_group.options.len(), 1);
        assert_eq!(
            literal_group.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned(NOT_RETURNED_PARENT_LABEL.into())
        );

        report.rows[4].parent =
            PartyLedgerMasterFieldObservation::Returned("\u{fffd}Primary".into());
        let marker = list_observed_capture_parents(&report, "\u{fffd}Primary".into()).unwrap();
        assert_eq!(
            marker.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("\u{fffd}Primary".into())
        );
    }

    #[test]
    fn blank_parent_search_keeps_interleaved_field_states_in_source_order() {
        let report = NativeTrialBalance {
            rows: vec![
                empty_parent_row(PartyLedgerMasterFieldObservation::NotObserved),
                empty_parent_row(PartyLedgerMasterFieldObservation::Returned("first".into())),
                empty_parent_row(PartyLedgerMasterFieldObservation::Returned(String::new())),
                empty_parent_row(PartyLedgerMasterFieldObservation::Returned("second".into())),
            ],
        };

        let listed = list_observed_capture_parents(&report, String::new()).unwrap();
        assert_eq!(
            listed
                .options
                .into_iter()
                .map(|option| option.parent)
                .collect::<Vec<_>>(),
            vec![
                PartyLedgerMasterFieldObservation::NotObserved,
                PartyLedgerMasterFieldObservation::Returned("first".into()),
                PartyLedgerMasterFieldObservation::Returned(String::new()),
                PartyLedgerMasterFieldObservation::Returned("second".into()),
            ]
        );
    }

    #[test]
    fn parent_option_scan_keeps_only_101_borrowed_candidates_for_200k_rows() {
        const SYNTHETIC_ROWS: usize = 200_000;
        let report = NativeTrialBalance {
            rows: (0..SYNTHETIC_ROWS)
                .map(|index| NativeTrialBalanceRow {
                    name: String::new(),
                    guid: String::new(),
                    parent: PartyLedgerMasterFieldObservation::Returned(format!(
                        "Synthetic parent {index:06}"
                    )),
                    opening: NativeTrialBalanceAmount::PresentEmpty,
                    debit: NativeTrialBalanceAmount::PresentEmpty,
                    credit: NativeTrialBalanceAmount::PresentEmpty,
                    closing: NativeTrialBalanceAmount::PresentEmpty,
                })
                .collect(),
        };
        let search = ParentSearch::new(String::new()).unwrap();
        let scan = scan_observed_capture_parents(&report, &search);
        assert_eq!(scan.max_retained_candidates, MAX_RETAINED_PARENT_CANDIDATES);
        assert!(
            scan.max_retained_parent_text_bytes
                <= MAX_RETAINED_PARENT_CANDIDATES * "Synthetic parent 000000".len()
        );
        let display_len = GROUP_LABEL_PREFIX.len() + "Synthetic parent 000000".len();
        assert!(
            scan.max_temporary_text_bytes <= "Synthetic parent 000000".len() + (2 * display_len)
        );

        let options = scan.finish(report.rows.len());
        assert_eq!(options.source_row_count, SYNTHETIC_ROWS);
        assert_eq!(options.options.len(), MAX_PARENT_OPTIONS);
        assert!(options.has_more);
        assert_eq!(options.options[0].row_count, 1);
        assert_eq!(
            options.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("Synthetic parent 000000".into())
        );
    }

    #[test]
    fn late_exact_parent_replaces_a_bounded_substring_candidate_and_counts_duplicates() {
        let mut rows = (0..MAX_RETAINED_PARENT_CANDIDATES)
            .map(|index| NativeTrialBalanceRow {
                name: String::new(),
                guid: String::new(),
                parent: PartyLedgerMasterFieldObservation::Returned(format!(
                    "candidate needle {index:03}"
                )),
                opening: NativeTrialBalanceAmount::PresentEmpty,
                debit: NativeTrialBalanceAmount::PresentEmpty,
                credit: NativeTrialBalanceAmount::PresentEmpty,
                closing: NativeTrialBalanceAmount::PresentEmpty,
            })
            .collect::<Vec<_>>();
        rows.extend((0..2).map(|_| NativeTrialBalanceRow {
            name: String::new(),
            guid: String::new(),
            parent: PartyLedgerMasterFieldObservation::Returned("needle".into()),
            opening: NativeTrialBalanceAmount::PresentEmpty,
            debit: NativeTrialBalanceAmount::PresentEmpty,
            credit: NativeTrialBalanceAmount::PresentEmpty,
            closing: NativeTrialBalanceAmount::PresentEmpty,
        }));
        let report = NativeTrialBalance { rows };

        let listed = list_observed_capture_parents(&report, "needle".into()).unwrap();
        assert_eq!(listed.options.len(), MAX_PARENT_OPTIONS);
        assert!(listed.has_more);
        assert_eq!(
            listed.options[0].parent,
            PartyLedgerMasterFieldObservation::Returned("needle".into())
        );
        assert_eq!(listed.options[0].row_count, 2);
    }

    #[test]
    fn parent_option_search_accepts_supported_display_and_control_text_but_refuses_oversize() {
        let unicode_parent = "\u{0130}".repeat(2_048);
        let report = NativeTrialBalance {
            rows: vec![
                empty_parent_row(PartyLedgerMasterFieldObservation::Returned(
                    unicode_parent.clone(),
                )),
                empty_parent_row(PartyLedgerMasterFieldObservation::Returned(
                    "parent\0".into(),
                )),
            ],
        };
        let supported_display = format!("{GROUP_LABEL_PREFIX}{unicode_parent}");
        assert_eq!(supported_display.len(), MAX_PARENT_DISPLAY_BYTES);
        assert_eq!(
            list_observed_capture_parents(&report, supported_display)
                .unwrap()
                .options[0]
                .parent,
            PartyLedgerMasterFieldObservation::Returned(unicode_parent)
        );
        assert_eq!(
            list_observed_capture_parents(&report, "parent\0".into())
                .unwrap()
                .options[0]
                .parent,
            PartyLedgerMasterFieldObservation::Returned("parent\0".into())
        );
        assert_eq!(
            list_observed_capture_parents(&report, "x".repeat(MAX_PARENT_DISPLAY_BYTES + 1))
                .unwrap_err(),
            TrialBalanceCaptureParentOptionsError::SearchInvalid
        );
    }
}
