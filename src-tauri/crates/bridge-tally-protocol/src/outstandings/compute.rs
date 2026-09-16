use std::collections::BTreeMap;

use bridge_tally_primitives::{ExactDecimal, TallyDate};

use super::{
    AgeingAnchor, AgeingBillCounts, AgeingBuckets, BillReferenceKind, CompleteScan, CreditPeriod,
    MoneyValue, OutstandingsError, OutstandingsReport, PartyOutstanding,
};

/// How a bill is identified within one ledger.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum BillKey {
    /// A bill Tally named via `New Ref` / `Agst Ref`.
    Named(String),
    /// The party-scoped `On Account` aggregate, which carries no bill identity.
    OnAccountAggregate,
}

struct OpenBill {
    balance: ExactDecimal,
    kind: OpenBillKind,
}

/// A named bill always carries an ageing date; On Account never does.
/// TALLY_PROTOCOL_REFERENCE.md §12a.2 records that Tally does not age On
/// Account and leaves its overdue value blank.
///
/// Keeping these states distinct prevents an On Account aggregate from
/// accidentally entering bill ageing when the calculation changes.
enum OpenBillKind {
    Named { oldest_date: TallyDate },
    OnAccount,
}

#[derive(Default)]
struct PartyTotals {
    receivable: Option<ExactDecimal>,
    payable: Option<ExactDecimal>,
    oldest_bill_age: Option<u32>,
}

pub fn compute_outstandings(
    scan: &CompleteScan,
    as_of: TallyDate,
) -> Result<OutstandingsReport, OutstandingsError> {
    compute_outstandings_with_ageing_anchor(scan, as_of, AgeingAnchor::DueDate)
}

/// Computes aged outstandings using the caller-selected bill or due-date
/// basis. The default entry point uses `DueDate`, which matches Tally's
/// native overdue report where credit periods exist.
pub fn compute_outstandings_with_ageing_anchor(
    scan: &CompleteScan,
    as_of: TallyDate,
    ageing_anchor: AgeingAnchor,
) -> Result<OutstandingsReport, OutstandingsError> {
    if &as_of < scan.window().to() {
        return Err(OutstandingsError::InvalidDateWindow);
    }
    // A bill is identified by its ledger plus either a NAMED reference or the
    // party-scoped On Account aggregate. These must be distinct key variants: a
    // magic string would collide with an ordinary bill whose reference happens
    // to be that literal, letting the two reconcile against each other and hide
    // an open balance. Tally bill names are free user text, so no sentinel
    // string is safe.
    let mut bills = BTreeMap::<(String, BillKey), OpenBill>::new();
    let mut vouchers = scan
        .vouchers()
        .iter()
        // Optional vouchers are non-posting in Tally: they are excluded from
        // ordinary books, so including them would inflate receivable/payable
        // totals. Tally's own bank-statement import creates vouchers as
        // Optional by default, so this is a live production shape, not an edge
        // case.
        .filter(|voucher| !voucher.cancelled && !voucher.deleted && !voucher.optional)
        .collect::<Vec<_>>();
    vouchers.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.guid.cmp(&right.guid))
    });
    for voucher in vouchers {
        for (entry_index, entry) in voucher.ledger_entries.iter().enumerate() {
            for (allocation_index, allocation) in entry.bill_allocations.iter().enumerate() {
                let amount = exact(&allocation.amount)?;
                if amount.is_zero() {
                    continue;
                }
                let (reference, initial_kind) = match allocation.name.as_deref() {
                    Some(name) => (
                        BillKey::Named(name.to_string()),
                        OpenBillKind::Named {
                            oldest_date: bill_age_date(allocation, voucher, ageing_anchor)?,
                        },
                    ),
                    None if matches!(allocation.bill_type, BillReferenceKind::OnAccount) => {
                        // On Account carries no bill identity, so Tally treats
                        // it as a party-scoped aggregate. Keying it per voucher
                        // allocation instead means an advance receipt and a
                        // later on-account adjustment for the same party never
                        // reconcile: the party is reported as BOTH a receivable
                        // and a payable rather than its net balance. Aggregate
                        // by party, matching bridge-tally-core's contract.
                        let _ = (entry_index, allocation_index);
                        (BillKey::OnAccountAggregate, OpenBillKind::OnAccount)
                    }
                    None => {
                        return Err(OutstandingsError::InvalidResponse("bill_reference_missing"))
                    }
                };
                let bill = bills
                    .entry((entry.ledger_name.clone(), reference))
                    .or_insert_with(|| OpenBill {
                        balance: ExactDecimal::zero(),
                        kind: initial_kind,
                    });
                let previous_balance = bill.balance.clone();
                let next_balance = bill
                    .balance
                    .checked_add(amount)
                    .map_err(|_| OutstandingsError::ArithmeticOverflow)?;
                if previous_balance.is_zero() {
                    if let OpenBillKind::Named { oldest_date } = &mut bill.kind {
                        *oldest_date = bill_age_date(allocation, voucher, ageing_anchor)?;
                    }
                } else if !next_balance.is_zero()
                    && previous_balance.is_negative() != next_balance.is_negative()
                {
                    // A genuine sign flip makes a new exposure without an
                    // intermediate zero balance. Deliberately use the voucher
                    // date: on an Agst Ref, BILLDATE belongs to the bill being
                    // settled, not the newly exposed balance.
                    if let OpenBillKind::Named { oldest_date } = &mut bill.kind {
                        *oldest_date = voucher.date.clone();
                    }
                }
                bill.balance = next_balance;
            }
        }
    }

    let mut receivable_total = ExactDecimal::zero();
    let mut payable_total = ExactDecimal::zero();
    let mut has_unaged_receivable = false;
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
    let mut parties = BTreeMap::<String, PartyTotals>::new();
    for ((party, _), bill) in bills
        .into_iter()
        .filter(|(_, bill)| !bill.balance.is_zero())
    {
        let amount = bill
            .balance
            .abs()
            .map_err(|_| OutstandingsError::ArithmeticOverflow)?;
        let totals = parties.entry(party).or_default();
        let bill_age = match &bill.kind {
            OpenBillKind::Named { oldest_date } => overdue_days(oldest_date, &as_of)?,
            OpenBillKind::OnAccount => None,
        };
        if bill.balance.is_negative() {
            receivable_total = add(&receivable_total, &amount)?;
            has_unaged_receivable |= matches!(&bill.kind, OpenBillKind::OnAccount);
            totals.receivable = Some(add(
                totals.receivable.as_ref().unwrap_or(&ExactDecimal::zero()),
                &amount,
            )?);
            if matches!(&bill.kind, OpenBillKind::Named { .. }) {
                // Match the native report: a future-due named bill remains in
                // the first bucket, but has no truthful overdue age and cannot
                // become a party's oldest aged bill. On Account remains a
                // separate unaged aggregate and never enters bill buckets.
                let (bucket, bill_count) = match bill_age {
                    None | Some(0..=30) => {
                        (&mut ageing.days_0_30, &mut ageing_bill_counts.days_0_30)
                    }
                    Some(31..=60) => (&mut ageing.days_31_60, &mut ageing_bill_counts.days_31_60),
                    Some(61..=90) => (&mut ageing.days_61_90, &mut ageing_bill_counts.days_61_90),
                    Some(_) => (
                        &mut ageing.days_90_plus,
                        &mut ageing_bill_counts.days_90_plus,
                    ),
                };
                *bucket = add(bucket, &amount)?;
                *bill_count = bill_count
                    .checked_add(1)
                    .ok_or(OutstandingsError::ArithmeticOverflow)?;
                if let Some(age) = bill_age {
                    totals.oldest_bill_age =
                        Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
                }
            }
        } else {
            payable_total = add(&payable_total, &amount)?;
            totals.payable = Some(add(
                totals.payable.as_ref().unwrap_or(&ExactDecimal::zero()),
                &amount,
            )?);
            if let Some(age) = bill_age {
                totals.oldest_bill_age =
                    Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
            }
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
        .collect::<Result<Vec<_>, OutstandingsError>>()?;
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
        .ok_or(OutstandingsError::ArithmeticOverflow)?;

    Ok(OutstandingsReport {
        company_name: scan.company().name().to_string(),
        as_of_yyyymmdd: as_of.as_str().to_string(),
        receivable_total,
        payable_total,
        has_unaged_receivable,
        ageing,
        open_receivable_bill_count,
        ageing_bill_counts,
        top_parties,
        source_voucher_count: scan.vouchers().len(),
        source_bytes: scan.encoded_bytes(),
    })
}

fn bill_age_date(
    allocation: &super::BillAllocation,
    voucher: &super::Voucher,
    ageing_anchor: AgeingAnchor,
) -> Result<TallyDate, OutstandingsError> {
    let bill_date = match allocation.bill_type {
        // TALLY_PROTOCOL_REFERENCE §12a.2 (PR #117): Tally reported a 1-Jun
        // bill settled to zero and re-opened by a 1-Jul Agst Ref as due on
        // 1-Jun, 60 days overdue; zero re-opens age from the original
        // BILLDATE.
        BillReferenceKind::NewRef | BillReferenceKind::AgstRef => allocation
            .bill_date
            .clone()
            .ok_or(OutstandingsError::InvalidResponse("bill_date_missing")),
        BillReferenceKind::Advance => Ok(voucher.date.clone()),
        BillReferenceKind::OnAccount => Err(OutstandingsError::InvalidResponse(
            "bill_reference_forbidden",
        )),
    }?;
    match ageing_anchor {
        AgeingAnchor::BillDate => Ok(bill_date),
        AgeingAnchor::DueDate => add_credit_period(&bill_date, &allocation.credit_period),
    }
}

fn add_credit_period(
    date: &TallyDate,
    period: &CreditPeriod,
) -> Result<TallyDate, OutstandingsError> {
    match period {
        CreditPeriod::Days(days) => add_days(date, *days),
        CreditPeriod::Weeks(weeks) => add_days(
            date,
            weeks
                .checked_mul(7)
                .ok_or(OutstandingsError::InvalidResponse(
                    "bill_credit_period_invalid",
                ))?,
        ),
        CreditPeriod::Months(months) => date
            .add_months_clamped(*months)
            .map_err(|_| OutstandingsError::InvalidDateWindow),
    }
}

fn add_days(date: &TallyDate, days: u32) -> Result<TallyDate, OutstandingsError> {
    date.add_days(days)
        .map_err(|_| OutstandingsError::InvalidDateWindow)
}

fn exact(value: &MoneyValue) -> Result<&ExactDecimal, OutstandingsError> {
    match value {
        MoneyValue::Exact(value) => Ok(value),
        MoneyValue::Absent => Err(OutstandingsError::InvalidAmount),
    }
}

fn add(left: &ExactDecimal, right: &ExactDecimal) -> Result<ExactDecimal, OutstandingsError> {
    left.checked_add(right)
        .map_err(|_| OutstandingsError::ArithmeticOverflow)
}

fn days_between(from: &TallyDate, to: &TallyDate) -> Result<u32, OutstandingsError> {
    let from = civil_day(from)?;
    let to = civil_day(to)?;
    u32::try_from(to - from).map_err(|_| OutstandingsError::InvalidDateWindow)
}

/// Future-due open bills are open exposure, not a negative-aged error. They
/// remain in the first bucket, while their party has no `oldest_bill_age_days`
/// until the selected ageing date arrives.
fn overdue_days(from: &TallyDate, to: &TallyDate) -> Result<Option<u32>, OutstandingsError> {
    if from > to {
        return Ok(None);
    }
    days_between(from, to).map(Some)
}

fn civil_day(date: &TallyDate) -> Result<i64, OutstandingsError> {
    let value = date.as_str();
    let year = value[0..4]
        .parse::<i64>()
        .map_err(|_| OutstandingsError::InvalidDateWindow)?;
    let month = value[4..6]
        .parse::<i64>()
        .map_err(|_| OutstandingsError::InvalidDateWindow)?;
    let day = value[6..8]
        .parse::<i64>()
        .map_err(|_| OutstandingsError::InvalidDateWindow)?;
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Ok(era * 146_097 + day_of_era)
}

#[cfg(test)]
#[path = "compute_tests.rs"]
mod tests;
