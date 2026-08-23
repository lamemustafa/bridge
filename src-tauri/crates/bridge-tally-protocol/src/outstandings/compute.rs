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
mod tests {
    use bridge_tally_primitives::{ExactDecimal, TallyDate};
    use sha2::{Digest, Sha256};

    use crate::{
        decode_tally_xml_response_bytes_limited,
        outstandings::{
            AlterIdRange, BillAllocation, BillReferenceKind, CompleteScan, CreditPeriod,
            DateBoundaryProfile, DateWindow, LedgerEntry, MoneyValue, PinnedCompany, Voucher,
            VoucherAlterId, VoucherAlterIdHighWater,
        },
        xml_read_profiles::ValidatedCompanyName,
        ExpectedTallyTextEncoding,
    };

    use super::super::parser::parse_segment;
    use super::{add_credit_period, compute_outstandings, compute_outstandings_with_ageing_anchor};

    const AGEING_CORPUS: &[u8] =
        include_bytes!("../../tests/fixtures/vouchers_ageing_corpus_live.utf16le.xml");
    const GST_CREDIT_PERIOD_CORPUS: &[u8] =
        include_bytes!("../../tests/fixtures/vouchers_gst_credit_periods_live.utf16le.xml");
    const AGEING_CORPUS_GUID: &str = "2f65b86f-edf4-471c-99ed-da0de7163836";
    const GST_CREDIT_PERIOD_CORPUS_GUID: &str = "46faa869-1208-4119-8961-f28db4df3b8e";

    #[test]
    fn credit_periods_produce_calendar_due_dates_without_unit_guessing() {
        assert_eq!(
            add_credit_period(
                &TallyDate::parse("20260131").unwrap(),
                &CreditPeriod::Months(1)
            )
            .unwrap()
            .as_str(),
            "20260228"
        );
        assert_eq!(
            add_credit_period(
                &TallyDate::parse("20260101").unwrap(),
                &CreditPeriod::Weeks(3)
            )
            .unwrap()
            .as_str(),
            "20260122"
        );
        assert_eq!(
            add_credit_period(
                &TallyDate::parse("20260101").unwrap(),
                &CreditPeriod::Days(45)
            )
            .unwrap()
            .as_str(),
            "20260215"
        );
    }

    #[test]
    fn captured_ageing_corpus_moves_seven_of_eight_bills_between_anchors() {
        let scan = captured_scan(
            AGEING_CORPUS,
            "BRIDGE CORPUS AGEING",
            AGEING_CORPUS_GUID,
            "20250401",
            "20260331",
            8,
            "497aec1804603b5c79a6ece404554c1f0ee1fb005ce3187b627f3292c51605f6",
        );
        let as_of = TallyDate::parse("20260331").unwrap();
        let bill_date = compute_outstandings_with_ageing_anchor(
            &scan,
            as_of.clone(),
            crate::outstandings::AgeingAnchor::BillDate,
        )
        .expect("captured bill-date ageing computes");
        let due_date = compute_outstandings_with_ageing_anchor(
            &scan,
            as_of,
            crate::outstandings::AgeingAnchor::DueDate,
        )
        .expect("captured due-date ageing computes");

        assert_eq!(bill_date.ageing_bill_counts.days_0_30, 1);
        assert_eq!(bill_date.ageing_bill_counts.days_31_60, 2);
        assert_eq!(bill_date.ageing_bill_counts.days_61_90, 2);
        assert_eq!(bill_date.ageing_bill_counts.days_90_plus, 3);
        assert_eq!(due_date.ageing_bill_counts.days_0_30, 4);
        assert_eq!(due_date.ageing_bill_counts.days_31_60, 3);
        assert_eq!(due_date.ageing_bill_counts.days_61_90, 1);
        assert_eq!(due_date.ageing_bill_counts.days_90_plus, 0);
        assert_ne!(bill_date.ageing, due_date.ageing);
    }

    #[test]
    fn captured_gst_corpus_parses_week_and_month_credit_period_units() {
        let scan = captured_scan(
            GST_CREDIT_PERIOD_CORPUS,
            "BRIDGE CORPUS GST",
            GST_CREDIT_PERIOD_CORPUS_GUID,
            "20250401",
            "20250420",
            40,
            "1e340126eda8e767d2b53cab8bb2086add1ed35f53f7216a16edbc16624b30b8",
        );
        let periods = scan
            .vouchers()
            .iter()
            .flat_map(|voucher| voucher.ledger_entries.iter())
            .flat_map(|entry| entry.bill_allocations.iter())
            .map(|allocation| &allocation.credit_period)
            .collect::<Vec<_>>();

        assert!(periods.contains(&&CreditPeriod::Days(15)));
        assert!(periods.contains(&&CreditPeriod::Days(30)));
        assert!(periods.contains(&&CreditPeriod::Weeks(2)));
        assert!(periods.contains(&&CreditPeriod::Months(1)));
        assert!(periods.contains(&&CreditPeriod::Months(2)));
    }

    #[test]
    fn exact_bill_balances_age_and_split_receivable_from_payable() {
        let company = PinnedCompany::verified(
            ValidatedCompanyName::new("Synthetic Company").unwrap(),
            "synthetic-guid".to_string(),
        )
        .unwrap();
        let window =
            DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260101", "20260401").unwrap();
        let scan = CompleteScan {
            company,
            reporting_window: window,
            voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("5").unwrap(),
            vouchers: vec![
                voucher(
                    "sale",
                    "20260101",
                    "Customer",
                    "Invoice-1",
                    "New Ref",
                    "-100.00",
                ),
                voucher(
                    "receipt",
                    "20260201",
                    "Customer",
                    "Invoice-1",
                    "Agst Ref",
                    "40.00",
                ),
                voucher(
                    "recent",
                    "20260331",
                    "Customer",
                    "Invoice-2",
                    "New Ref",
                    "-10.00",
                ),
                voucher(
                    "purchase",
                    "20260201",
                    "Vendor",
                    "Purchase-1",
                    "New Ref",
                    "50.00",
                ),
                voucher(
                    "customer-payable",
                    "20260331",
                    "Customer",
                    "Customer-Credit-1",
                    "New Ref",
                    "5.00",
                ),
            ],
            encoded_bytes: 4096,
            empty_partition_witnesses: Vec::new(),
        };
        let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();
        assert_eq!(report.receivable_total.as_str(), "70");
        assert_eq!(report.payable_total.as_str(), "55");
        assert!(!report.has_unaged_receivable);
        assert_eq!(report.ageing.days_0_30.as_str(), "10");
        assert_eq!(report.ageing.days_61_90.as_str(), "60");
        assert_eq!(report.open_receivable_bill_count, 2);
        assert_eq!(report.ageing_bill_counts.days_0_30, 1);
        assert_eq!(report.ageing_bill_counts.days_31_60, 0);
        assert_eq!(report.ageing_bill_counts.days_61_90, 1);
        assert_eq!(report.ageing_bill_counts.days_90_plus, 0);
        assert_eq!(report.top_parties[0].party, "Customer");
        assert_eq!(report.top_parties[0].payable.as_str(), "5");
        assert_eq!(report.top_parties[0].outstanding_total.as_str(), "75");
        assert_eq!(report.top_parties[0].oldest_bill_age_days, Some(90));

        let later = compute_outstandings(&scan, TallyDate::parse("20260501").unwrap()).unwrap();
        assert_eq!(later.receivable_total, report.receivable_total);
        assert_eq!(later.payable_total, report.payable_total);
        assert_eq!(
            later.open_receivable_bill_count,
            report.open_receivable_bill_count
        );
        assert_ne!(later.ageing, report.ageing);
        assert_ne!(later.ageing_bill_counts, report.ageing_bill_counts);
        assert_eq!(later.as_of_yyyymmdd, "20260501");
    }

    #[test]
    fn settled_reference_reuse_restarts_age_from_the_new_open_balance() {
        let company = PinnedCompany::verified(
            ValidatedCompanyName::new("Synthetic Company").unwrap(),
            "synthetic-guid".to_string(),
        )
        .unwrap();
        let scan = CompleteScan {
            company,
            reporting_window: DateWindow::parse(
                DateBoundaryProfile::ModeAgnostic,
                "20260101",
                "20260401",
            )
            .unwrap(),
            voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("3").unwrap(),
            // Deliberately not chronological: computation must not inherit the
            // scan's GUID ordering when rebuilding a bill lifecycle.
            vouchers: vec![
                voucher(
                    "new-cycle",
                    "20260331",
                    "Customer",
                    "REUSED-REF",
                    "New Ref",
                    "-50.00",
                ),
                voucher(
                    "old-cycle-settlement",
                    "20260201",
                    "Customer",
                    "REUSED-REF",
                    "Agst Ref",
                    "100.00",
                ),
                voucher(
                    "old-cycle",
                    "20260101",
                    "Customer",
                    "REUSED-REF",
                    "New Ref",
                    "-100.00",
                ),
            ],
            encoded_bytes: 1024,
            empty_partition_witnesses: Vec::new(),
        };
        let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();
        assert_eq!(report.receivable_total.as_str(), "50");
        assert_eq!(report.ageing.days_0_30.as_str(), "50");
        assert_eq!(report.ageing.days_90_plus.as_str(), "0");
        assert_eq!(report.open_receivable_bill_count, 1);
        assert_eq!(report.ageing_bill_counts.days_0_30, 1);
        assert_eq!(report.top_parties[0].oldest_bill_age_days, Some(1));
    }

    #[test]
    fn future_due_open_bill_is_bucketed_without_claiming_an_overdue_age() {
        let company = PinnedCompany::verified(
            ValidatedCompanyName::new("Synthetic Company").unwrap(),
            "synthetic-guid".to_string(),
        )
        .unwrap();
        let mut future_due = voucher(
            "future-due",
            "20260315",
            "Customer",
            "Invoice-future",
            "New Ref",
            "-100.00",
        );
        future_due.ledger_entries[0].bill_allocations[0].credit_period = CreditPeriod::Days(30);
        let scan = CompleteScan {
            company,
            reporting_window: DateWindow::parse(
                DateBoundaryProfile::ModeAgnostic,
                "20260101",
                "20260331",
            )
            .unwrap(),
            voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("1").unwrap(),
            vouchers: vec![future_due],
            encoded_bytes: 1024,
            empty_partition_witnesses: Vec::new(),
        };

        let report = compute_outstandings(&scan, TallyDate::parse("20260331").unwrap())
            .expect("a future-due bill must not fail the complete report");

        assert_eq!(report.receivable_total.as_str(), "100");
        assert_eq!(report.ageing.days_0_30.as_str(), "100");
        assert_eq!(report.open_receivable_bill_count, 1);
        assert_eq!(report.top_parties[0].oldest_bill_age_days, None);
    }

    fn captured_scan(
        bytes: &[u8],
        company_name: &str,
        company_guid: &str,
        from: &str,
        to: &str,
        high_water: u64,
        expected_sha256: &str,
    ) -> CompleteScan {
        let observed_sha256 = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            observed_sha256, expected_sha256,
            "captured wire bytes changed"
        );
        let xml = decode_tally_xml_response_bytes_limited(
            bytes,
            "text/xml; charset=utf-16",
            ExpectedTallyTextEncoding::Utf16Le,
            bytes.len(),
        )
        .expect("captured BOM-less UTF-16LE response decodes")
        .text;
        let company = PinnedCompany::verified(
            ValidatedCompanyName::new(company_name)
                .expect("synthetic capture company name is valid"),
            company_guid.to_string(),
        )
        .expect("captured company identity is pinned");
        let window = DateWindow::parse(DateBoundaryProfile::ModeAgnostic, from, to)
            .expect("captured date window is valid");
        let parsed = parse_segment(
            &xml,
            &company,
            &window,
            AlterIdRange::new(0, high_water).expect("captured AlterID range is valid"),
        )
        .expect("captured response parses");
        assert_eq!(parsed.raw_row_count, parsed.vouchers.len());
        CompleteScan {
            company,
            reporting_window: window,
            voucher_alter_id_high_water: VoucherAlterIdHighWater::parse(&high_water.to_string())
                .unwrap(),
            vouchers: parsed.vouchers,
            encoded_bytes: bytes.len(),
            empty_partition_witnesses: Vec::new(),
        }
    }

    #[test]
    fn optional_vouchers_are_excluded_from_ordinary_book_totals() {
        // Optional vouchers are non-posting in Tally. Tally's own
        // bank-statement import creates vouchers as Optional by default, so a
        // real book can carry them with full bill allocations; counting them
        // would inflate receivables against the customer's actual ledger.
        let company = PinnedCompany::verified(
            ValidatedCompanyName::new("Synthetic Company").unwrap(),
            "synthetic-guid".to_string(),
        )
        .unwrap();
        let window =
            DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260101", "20260401").unwrap();
        let posted = voucher(
            "sale",
            "20260101",
            "Customer",
            "Invoice-1",
            "New Ref",
            "-100.00",
        );
        let mut optional = voucher(
            "optional-sale",
            "20260101",
            "Customer",
            "Invoice-2",
            "New Ref",
            "-250.00",
        );
        optional.optional = true;

        let scan = CompleteScan {
            company,
            reporting_window: window,
            voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("5").unwrap(),
            vouchers: vec![posted, optional],
            encoded_bytes: 2048,
            empty_partition_witnesses: Vec::new(),
        };
        let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();

        // Only the posted 100.00 is receivable; the optional 250.00 is absent
        // from the total AND from the open-bill count.
        assert_eq!(report.receivable_total.as_str(), "100");
        assert_eq!(report.open_receivable_bill_count, 1);
        let customer = report
            .top_parties
            .iter()
            .find(|party| party.party == "Customer")
            .expect("the posted voucher's party is present");
        assert_eq!(
            customer.receivable.as_str(),
            "100",
            "an optional voucher reached top-party exposure"
        );
    }

    fn voucher(
        guid: &str,
        date: &str,
        party: &str,
        reference: &str,
        bill_type: &str,
        amount: &str,
    ) -> Voucher {
        let amount = ExactDecimal::parse(amount).unwrap();
        Voucher {
            guid: guid.to_string(),
            master_id: guid.to_string(),
            alter_id: VoucherAlterId::parse("1").unwrap(),
            date: TallyDate::parse(date).unwrap(),
            voucher_type: "Synthetic".to_string(),
            voucher_number: None,
            party_ledger_name: Some(party.to_string()),
            cancelled: false,
            deleted: false,
            optional: false,
            ledger_entries: vec![LedgerEntry {
                ledger_name: party.to_string(),
                bill_allocations: vec![BillAllocation {
                    bill_date: matches!(bill_type, "New Ref" | "Agst Ref")
                        .then(|| TallyDate::parse(date).unwrap()),
                    name: Some(reference.to_string()),
                    bill_type: match bill_type {
                        "New Ref" => BillReferenceKind::NewRef,
                        "Agst Ref" => BillReferenceKind::AgstRef,
                        "Advance" => BillReferenceKind::Advance,
                        "On Account" => BillReferenceKind::OnAccount,
                        _ => panic!("synthetic test must use a known kind"),
                    },
                    amount: MoneyValue::Exact(amount),
                    credit_period: CreditPeriod::Days(0),
                }],
            }],
        }
    }
}
