//! Exact summaries of the native amounts already captured by the runtime.
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::{
    native_trial_balance::{NativeTrialBalance, NativeTrialBalanceAmount, NativeTrialBalanceRow},
    PartyLedgerMasterFieldObservation,
};
use serde::Serialize;

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
    use bridge_tally_protocol::native_trial_balance::parse_native_trial_balance;

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
}
