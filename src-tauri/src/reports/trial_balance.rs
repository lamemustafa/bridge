//! Exact summaries of the native amounts already captured by the runtime.
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::native_trial_balance::{NativeTrialBalance, NativeTrialBalanceAmount};
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

/// These are sums of numeric observations. Empty native fields remain counted
/// separately, so a column sum cannot masquerade as fully observed arithmetic.
pub fn observed_totals(
    report: &NativeTrialBalance,
) -> Result<TrialBalanceTotals, bridge_tally_core::TallyError> {
    let mut totals = std::array::from_fn::<_, 4, _>(|_| ObservedAmountTotal {
        sum: ExactDecimal::zero(),
        empty_count: 0,
    });
    for row in &report.rows {
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
}
