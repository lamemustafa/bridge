use std::collections::BTreeMap;

use bridge_tally_primitives::{ExactDecimal, TallyDate};

use super::{
    AgeingBillCounts, AgeingBuckets, BillReferenceKind, CompleteScan, MoneyValue,
    OutstandingsError, OutstandingsReport, PartyOutstanding,
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
    is_on_account: bool,
    oldest_date: Option<TallyDate>,
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
                let (reference, is_on_account) = match allocation.name.as_deref() {
                    Some(name) => (BillKey::Named(name.to_string()), false),
                    None if matches!(allocation.bill_type, BillReferenceKind::OnAccount) => {
                        // On Account carries no bill identity, so Tally treats
                        // it as a party-scoped aggregate. Keying it per voucher
                        // allocation instead means an advance receipt and a
                        // later on-account adjustment for the same party never
                        // reconcile: the party is reported as BOTH a receivable
                        // and a payable rather than its net balance. Aggregate
                        // by party, matching bridge-tally-core's contract.
                        let _ = (entry_index, allocation_index);
                        (BillKey::OnAccountAggregate, true)
                    }
                    None => {
                        return Err(OutstandingsError::InvalidResponse("bill_reference_missing"))
                    }
                };
                let bill = bills
                    .entry((entry.ledger_name.clone(), reference))
                    .or_insert_with(|| OpenBill {
                        balance: ExactDecimal::zero(),
                        is_on_account,
                        oldest_date: None,
                    });
                let previous_balance = bill.balance.clone();
                let next_balance = bill
                    .balance
                    .checked_add(amount)
                    .map_err(|_| OutstandingsError::ArithmeticOverflow)?;
                if previous_balance.is_zero() {
                    bill.oldest_date = match allocation.bill_type {
                        // TALLY_PROTOCOL_REFERENCE §12a.2 (PR #117): Tally
                        // reported a 1-Jun bill settled to zero and re-opened
                        // by a 1-Jul Agst Ref as due on 1-Jun, 60 days overdue;
                        // zero re-opens age from the original BILLDATE.
                        BillReferenceKind::NewRef | BillReferenceKind::AgstRef => Some(
                            allocation
                                .bill_date
                                .clone()
                                .ok_or(OutstandingsError::InvalidResponse("bill_date_missing"))?,
                        ),
                        BillReferenceKind::Advance => Some(voucher.date.clone()),
                        // TALLY_PROTOCOL_REFERENCE §12a.2: On Account has no
                        // bill reference and is not aged. Retaining a date here
                        // would let it enter the bill-only ageing presentation.
                        BillReferenceKind::OnAccount => None,
                    };
                } else if !next_balance.is_zero()
                    && previous_balance.is_negative() != next_balance.is_negative()
                {
                    // A genuine sign flip makes a new exposure without an
                    // intermediate zero balance. Deliberately use the voucher
                    // date: on an Agst Ref, BILLDATE belongs to the bill being
                    // settled, not the newly exposed balance.
                    bill.oldest_date = (!bill.is_on_account).then(|| voucher.date.clone());
                }
                bill.balance = next_balance;
            }
        }
    }

    let mut receivable_total = ExactDecimal::zero();
    let mut payable_total = ExactDecimal::zero();
    let mut on_account_receivable_total = ExactDecimal::zero();
    let mut on_account_payable_total = ExactDecimal::zero();
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
        if bill.balance.is_negative() {
            receivable_total = add(&receivable_total, &amount)?;
            totals.receivable = Some(add(
                totals.receivable.as_ref().unwrap_or(&ExactDecimal::zero()),
                &amount,
            )?);
            if bill.is_on_account {
                on_account_receivable_total = add(&on_account_receivable_total, &amount)?;
                continue;
            }
            let age = days_between(
                bill.oldest_date
                    .as_ref()
                    .ok_or(OutstandingsError::InvalidResponse("bill_age_missing"))?,
                &as_of,
            )?;
            totals.oldest_bill_age =
                Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
            let (bucket, bill_count) = match age {
                0..=30 => (&mut ageing.days_0_30, &mut ageing_bill_counts.days_0_30),
                31..=60 => (&mut ageing.days_31_60, &mut ageing_bill_counts.days_31_60),
                61..=90 => (&mut ageing.days_61_90, &mut ageing_bill_counts.days_61_90),
                _ => (
                    &mut ageing.days_90_plus,
                    &mut ageing_bill_counts.days_90_plus,
                ),
            };
            *bucket = add(bucket, &amount)?;
            *bill_count = bill_count
                .checked_add(1)
                .ok_or(OutstandingsError::ArithmeticOverflow)?;
        } else {
            payable_total = add(&payable_total, &amount)?;
            totals.payable = Some(add(
                totals.payable.as_ref().unwrap_or(&ExactDecimal::zero()),
                &amount,
            )?);
            if bill.is_on_account {
                on_account_payable_total = add(&on_account_payable_total, &amount)?;
                continue;
            }
            let age = days_between(
                bill.oldest_date
                    .as_ref()
                    .ok_or(OutstandingsError::InvalidResponse("bill_age_missing"))?,
                &as_of,
            )?;
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
        on_account_receivable_total,
        on_account_payable_total,
        ageing,
        open_receivable_bill_count,
        ageing_bill_counts,
        top_parties,
        source_voucher_count: scan.vouchers().len(),
        source_bytes: scan.encoded_bytes(),
    })
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

    use crate::{
        outstandings::{
            BillAllocation, BillReferenceKind, CompleteScan, DateBoundaryProfile, DateWindow,
            LedgerEntry, MoneyValue, PinnedCompany, Voucher, VoucherAlterId,
            VoucherAlterIdHighWater,
        },
        xml_read_profiles::ValidatedCompanyName,
    };

    use super::compute_outstandings;

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

    #[test]
    fn on_account_exposure_is_included_but_has_no_bill_age() {
        let company = PinnedCompany::verified(
            ValidatedCompanyName::new("Synthetic Company").unwrap(),
            "synthetic-guid".to_string(),
        )
        .unwrap();
        let window =
            DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20260101", "20260401").unwrap();
        let mut receivable = voucher(
            "on-account-receivable",
            "20260101",
            "Customer",
            "ignored",
            "On Account",
            "-100.00",
        );
        receivable.ledger_entries[0].bill_allocations[0].name = None;
        let mut payable = voucher(
            "on-account-payable",
            "20260101",
            "Supplier",
            "ignored",
            "On Account",
            "25.00",
        );
        payable.ledger_entries[0].bill_allocations[0].name = None;

        let scan = CompleteScan {
            company,
            reporting_window: window,
            voucher_alter_id_high_water: VoucherAlterIdHighWater::parse("2").unwrap(),
            vouchers: vec![receivable, payable],
            encoded_bytes: 2048,
            empty_partition_witnesses: Vec::new(),
        };
        let report = compute_outstandings(&scan, TallyDate::parse("20260401").unwrap()).unwrap();

        assert_eq!(report.receivable_total.as_str(), "100");
        assert_eq!(report.payable_total.as_str(), "25");
        assert_eq!(report.on_account_receivable_total.as_str(), "100");
        assert_eq!(report.on_account_payable_total.as_str(), "25");
        assert_eq!(report.open_receivable_bill_count, 0);
        assert_eq!(report.ageing.days_0_30.as_str(), "0");
        assert_eq!(report.ageing.days_31_60.as_str(), "0");
        assert_eq!(report.ageing.days_61_90.as_str(), "0");
        assert_eq!(report.ageing.days_90_plus.as_str(), "0");
        assert_eq!(report.top_parties[0].party, "Customer");
        assert_eq!(report.top_parties[0].oldest_bill_age_days, None);
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
                    bill_date: (bill_type == "New Ref").then(|| TallyDate::parse(date).unwrap()),
                    name: Some(reference.to_string()),
                    bill_type: match bill_type {
                        "New Ref" => BillReferenceKind::NewRef,
                        "Agst Ref" => BillReferenceKind::AgstRef,
                        "Advance" => BillReferenceKind::Advance,
                        "On Account" => BillReferenceKind::OnAccount,
                        _ => panic!("synthetic test must use a known kind"),
                    },
                    amount: MoneyValue::Exact(amount),
                }],
            }],
        }
    }
}
