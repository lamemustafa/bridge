use std::collections::BTreeMap;

use bridge_tally_primitives::{ExactDecimal, TallyDate};

use crate::outstandings_shared::{
    AgeingBillCounts, AgeingBuckets, OutstandingsReport, PartyOutstanding,
};

use super::model::{
    AgeingAnchor, LedgerSnapshotEntry, NativeBillRow, NativeOutstandingsError,
    NativeOutstandingsResult, PartyResidual,
};

#[derive(Default)]
struct PartyAccumulator {
    receivable: Option<ExactDecimal>,
    payable: Option<ExactDecimal>,
    oldest_bill_age: Option<u32>,
}

/// Computes a drop-in [`OutstandingsReport`] plus on-account residual
/// evidence from the native Bills Receivable/Payable rows and the ledger
/// snapshot, per TALLY_PROTOCOL_REFERENCE ground truth captured 2026-08-07.
///
/// `source_bytes` is the caller's real encoded byte count for the responses
/// consumed; this path reads no vouchers, so `source_voucher_count` is
/// always `0`.
pub fn compute_native_outstandings(
    company_name: &str,
    receivable_rows: &[NativeBillRow],
    payable_rows: &[NativeBillRow],
    ledgers: &[LedgerSnapshotEntry],
    anchor: AgeingAnchor,
    as_of: &TallyDate,
    source_bytes: usize,
) -> Result<NativeOutstandingsResult, NativeOutstandingsError> {
    let mut receivable_total = ExactDecimal::zero();
    let mut payable_total = ExactDecimal::zero();
    let mut ageing = AgeingBuckets {
        days_0_30: ExactDecimal::zero(),
        days_31_60: ExactDecimal::zero(),
        days_61_90: ExactDecimal::zero(),
        days_90_plus: ExactDecimal::zero(),
    };
    let mut ageing_bill_counts = AgeingBillCounts {
        days_0_30: 0,
        days_31_60: 0,
        days_61_90: 0,
        days_90_plus: 0,
    };
    let mut overdue_crosscheck_mismatches = 0_usize;
    let mut parties = BTreeMap::<String, PartyAccumulator>::new();

    for row in receivable_rows
        .iter()
        .filter(|row| !row.closing_balance.is_zero())
    {
        let amount = row
            .closing_balance
            .abs()
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
        receivable_total = add(&receivable_total, &amount)?;

        let age = overdue_days(bill_anchor_date(row, anchor), as_of)?;
        if let Some(tally_overdue) = row.tally_overdue_days {
            let age_from_due = overdue_days(&row.due_date, as_of)?.unwrap_or(0);
            if i64::from(age_from_due) != tally_overdue {
                overdue_crosscheck_mismatches += 1;
            }
        }

        let totals = parties.entry(row.party.clone()).or_default();
        totals.receivable = Some(add(
            totals.receivable.as_ref().unwrap_or(&ExactDecimal::zero()),
            &amount,
        )?);
        // Tally keeps a future-due open bill in its first ageing bucket even
        // though BILLOVERDUE is empty and no overdue age can truthfully be
        // claimed. Bucket membership and bill age are therefore distinct:
        // count the bill and its amount, but retain `None` for oldest age.
        let (bucket, count) = match age {
            None | Some(0..=30) => (&mut ageing.days_0_30, &mut ageing_bill_counts.days_0_30),
            Some(31..=60) => (&mut ageing.days_31_60, &mut ageing_bill_counts.days_31_60),
            Some(61..=90) => (&mut ageing.days_61_90, &mut ageing_bill_counts.days_61_90),
            Some(_) => (
                &mut ageing.days_90_plus,
                &mut ageing_bill_counts.days_90_plus,
            ),
        };
        *bucket = add(bucket, &amount)?;
        *count = count
            .checked_add(1)
            .ok_or(NativeOutstandingsError::ArithmeticOverflow)?;
        if let Some(age) = age {
            totals.oldest_bill_age =
                Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
        }
    }

    for row in payable_rows
        .iter()
        .filter(|row| !row.closing_balance.is_zero())
    {
        let amount = row
            .closing_balance
            .abs()
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
        payable_total = add(&payable_total, &amount)?;

        let age = overdue_days(bill_anchor_date(row, anchor), as_of)?;

        let totals = parties.entry(row.party.clone()).or_default();
        totals.payable = Some(add(
            totals.payable.as_ref().unwrap_or(&ExactDecimal::zero()),
            &amount,
        )?);
        if let Some(age) = age {
            totals.oldest_bill_age =
                Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
        }
    }

    let mut top_parties = parties
        .into_iter()
        .map(|(party, totals)| {
            let receivable = totals.receivable.unwrap_or_else(ExactDecimal::zero);
            let payable = totals.payable.unwrap_or_else(ExactDecimal::zero);
            let outstanding_total = add(&receivable, &payable)?;
            Ok(PartyOutstanding {
                party,
                receivable,
                payable,
                outstanding_total,
                oldest_bill_age_days: totals.oldest_bill_age,
            })
        })
        .collect::<Result<Vec<_>, NativeOutstandingsError>>()?;
    top_parties.sort_by(|left, right| {
        right
            .outstanding_total
            .cmp_magnitude(&left.outstanding_total)
            .then_with(|| left.party.cmp(&right.party))
    });
    top_parties.truncate(10);

    let open_receivable_bill_count = ageing_bill_counts
        .days_0_30
        .checked_add(ageing_bill_counts.days_31_60)
        .and_then(|value| value.checked_add(ageing_bill_counts.days_61_90))
        .and_then(|value| value.checked_add(ageing_bill_counts.days_90_plus))
        .ok_or(NativeOutstandingsError::ArithmeticOverflow)?;

    let (residuals, residual_total, has_unaged_receivable) =
        compute_residuals(receivable_rows, payable_rows, ledgers)?;

    let report = OutstandingsReport {
        company_name: company_name.to_string(),
        as_of_yyyymmdd: as_of.as_str().to_string(),
        receivable_total,
        payable_total,
        has_unaged_receivable,
        ageing,
        open_receivable_bill_count,
        ageing_bill_counts,
        top_parties,
        source_voucher_count: 0,
        source_bytes,
    };

    Ok(NativeOutstandingsResult {
        report,
        residuals,
        residual_total,
        overdue_crosscheck_mismatches,
    })
}

/// Per-party residual: `ledger CLOSINGBALANCE - sum(receivable BILLCL) -
/// sum(payable BILLCL)`. The native Bills Receivable/Payable reports only
/// ever list NAMED bills, so any non-zero residual on a party ledger is
/// exactly that party's on-account exposure — present in the ledger balance
/// but invisible to (and therefore unaged by) the bill-level reports. A
/// Sundry Debtor/Creditor with bill-wise tracking disabled has no bill rows
/// by construction, so its entire balance is such a residual.
fn compute_residuals(
    receivable_rows: &[NativeBillRow],
    payable_rows: &[NativeBillRow],
    ledgers: &[LedgerSnapshotEntry],
) -> Result<(Vec<PartyResidual>, ExactDecimal, bool), NativeOutstandingsError> {
    let mut receivable_sums = BTreeMap::<&str, ExactDecimal>::new();
    for row in receivable_rows {
        let entry = receivable_sums
            .entry(row.party.as_str())
            .or_insert_with(ExactDecimal::zero);
        *entry = entry
            .checked_add(&row.closing_balance)
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
    }
    let mut payable_sums = BTreeMap::<&str, ExactDecimal>::new();
    for row in payable_rows {
        let entry = payable_sums
            .entry(row.party.as_str())
            .or_insert_with(ExactDecimal::zero);
        *entry = entry
            .checked_add(&row.closing_balance)
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
    }

    let mut residuals = Vec::new();
    let mut residual_total = ExactDecimal::zero();
    let mut has_unaged_receivable = false;
    for ledger in ledgers.iter().filter(|ledger| is_party_ledger(ledger)) {
        let zero = ExactDecimal::zero();
        let receivable_sum = receivable_sums.get(ledger.name.as_str()).unwrap_or(&zero);
        let payable_sum = payable_sums.get(ledger.name.as_str()).unwrap_or(&zero);
        let residual = ledger
            .closing_balance
            .checked_subtract(receivable_sum)
            .and_then(|value| value.checked_subtract(payable_sum))
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
        if !residual.is_zero() {
            let magnitude = residual
                .abs()
                .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
            residual_total = add(&residual_total, &magnitude)?;
            // A receivable-side (debtor) ledger reports a negative closing
            // balance in this data; a non-zero residual there is exposure
            // the Bills Receivable report cannot see and therefore cannot
            // age.
            has_unaged_receivable |= ledger.closing_balance.is_negative();
        }
        residuals.push(PartyResidual {
            party: ledger.name.clone(),
            amount: residual,
        });
    }
    Ok((residuals, residual_total, has_unaged_receivable))
}

fn is_party_ledger(ledger: &LedgerSnapshotEntry) -> bool {
    ledger.bill_wise_on
        || ledger
            .parent
            .as_deref()
            .is_some_and(|parent| matches!(parent.trim(), "Sundry Debtors" | "Sundry Creditors"))
}

fn bill_anchor_date(row: &NativeBillRow, anchor: AgeingAnchor) -> &TallyDate {
    match anchor {
        AgeingAnchor::BillDate => &row.bill_date,
        AgeingAnchor::DueDate => &row.due_date,
    }
}

fn add(left: &ExactDecimal, right: &ExactDecimal) -> Result<ExactDecimal, NativeOutstandingsError> {
    left.checked_add(right)
        .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)
}

/// Public for tests and callers that want to cross-check or display a raw
/// bill age without going through the full report computation.
pub fn age_in_days(from: &TallyDate, to: &TallyDate) -> Result<u32, NativeOutstandingsError> {
    days_between(from, to)
}

/// A bill whose due date has not arrived has zero overdue days in Tally's
/// `BILLOVERDUE` column, but no bill age to place into the ageing buckets.
/// Keep that state distinct from a bill due today: the latter is aged zero,
/// while the former is absent from ageing and from `oldest_bill_age_days`.
fn overdue_days(from: &TallyDate, to: &TallyDate) -> Result<Option<u32>, NativeOutstandingsError> {
    if from > to {
        return Ok(None);
    }
    days_between(from, to).map(Some)
}

fn days_between(from: &TallyDate, to: &TallyDate) -> Result<u32, NativeOutstandingsError> {
    let from = civil_day(from)?;
    let to = civil_day(to)?;
    u32::try_from(to - from)
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_after_as_of"))
}

/// Days-since-epoch via Howard Hinnant's `days_from_civil` algorithm — the
/// same computation `outstandings::compute` uses, duplicated here because
/// that module's helper is private to its own subtree.
fn civil_day(date: &TallyDate) -> Result<i64, NativeOutstandingsError> {
    let value = date.as_str();
    let year = value[0..4]
        .parse::<i64>()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_malformed"))?;
    let month = value[4..6]
        .parse::<i64>()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_malformed"))?;
    let day = value[6..8]
        .parse::<i64>()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_malformed"))?;
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Ok(era * 146_097 + day_of_era)
}
