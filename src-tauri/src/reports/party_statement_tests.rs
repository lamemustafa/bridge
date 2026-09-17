use super::*;

fn bill(
    party: &str,
    reference: &str,
    amount: &str,
    age_days: Option<u32>,
    kind: ExposureDirection,
) -> OpenBillRow {
    OpenBillRow {
        party: party.to_string(),
        reference: reference.to_string(),
        bill_date: "20260101".to_string(),
        due_date: "20260201".to_string(),
        amount: ExactDecimal::parse(amount).unwrap(),
        age_days,
        kind,
    }
}

fn unallocated(party: &str, amount: &str) -> UnallocatedParty {
    UnallocatedParty {
        party: party.to_string(),
        amount: ExactDecimal::parse(amount).unwrap(),
        direction: ExposureDirection::Receivable,
    }
}

/// Sums decimal literals through the same `checked_add` accumulation
/// `build_party_statement` uses, so an expectation here lands in
/// whatever canonical form the accumulator itself produces (e.g.
/// trailing fractional zeros dropped) instead of a hand-normalised
/// guess that can drift from `ExactDecimal`'s actual output.
fn exact_sum(values: &[&str]) -> ExactDecimal {
    values.iter().fold(ExactDecimal::zero(), |total, value| {
        total
            .checked_add(&ExactDecimal::parse(*value).unwrap())
            .unwrap()
    })
}

#[test]
fn builds_a_statement_sorted_oldest_first_and_filtered_to_the_party() {
    let bills = vec![
        bill(
            "Aarav Textiles",
            "INV-3",
            "1000.00",
            Some(10),
            ExposureDirection::Receivable,
        ),
        bill(
            "Aarav Textiles",
            "INV-1",
            "2500.50",
            Some(95),
            ExposureDirection::Receivable,
        ),
        bill(
            "Aarav Textiles",
            "INV-2",
            "300.00",
            Some(45),
            ExposureDirection::Receivable,
        ),
        bill(
            "Other Party",
            "INV-9",
            "999.00",
            Some(200),
            ExposureDirection::Receivable,
        ),
    ];
    let unallocated_rows = vec![unallocated("Aarav Textiles", "150.25")];

    let statement = build_party_statement(
        "Lab Co",
        "20260808",
        "Aarav Textiles",
        &bills,
        &unallocated_rows,
    )
    .expect("party has exposure");

    assert_eq!(statement.company, "Lab Co");
    assert_eq!(statement.party, "Aarav Textiles");
    assert_eq!(statement.as_of_yyyymmdd, "20260808");
    assert_eq!(
        statement
            .bills
            .iter()
            .map(|row| row.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["INV-1", "INV-2", "INV-3"],
    );
    assert_eq!(
        statement.bill_total,
        exact_sum(&["1000.00", "2500.50", "300.00"])
    );
    assert_eq!(
        statement.unallocated,
        ExactDecimal::parse("150.25").unwrap()
    );
    assert_eq!(
        statement.grand_total,
        exact_sum(&["1000.00", "2500.50", "300.00", "150.25"]),
    );
}

#[test]
fn aged_bucket_subtotals_sum_to_exactly_the_bill_total() {
    let bills = vec![
        bill(
            "Party",
            "A",
            "10.10",
            Some(5),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "B",
            "20.20",
            Some(30),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "C",
            "30.30",
            Some(31),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "D",
            "40.40",
            Some(60),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "E",
            "50.50",
            Some(61),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "F",
            "60.60",
            Some(90),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "G",
            "70.70",
            Some(91),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "H",
            "80.80",
            Some(500),
            ExposureDirection::Receivable,
        ),
    ];
    let statement = build_party_statement("Lab Co", "20260808", "Party", &bills, &[])
        .expect("party has exposure");

    assert_eq!(
        statement.subtotals.receivable.days_0_30,
        exact_sum(&["10.10", "20.20"])
    );
    assert_eq!(
        statement.subtotals.receivable.days_31_60,
        exact_sum(&["30.30", "40.40"])
    );
    assert_eq!(
        statement.subtotals.receivable.days_61_90,
        exact_sum(&["50.50", "60.60"])
    );
    assert_eq!(
        statement.subtotals.receivable.days_90_plus,
        exact_sum(&["70.70", "80.80"])
    );
    assert!(statement.subtotals.receivable.not_yet_due.is_zero());
    assert!(statement.subtotals.payable.total().unwrap().is_zero());
    assert_eq!(statement.subtotals.total().unwrap(), statement.bill_total);
    assert_eq!(
        statement.bill_total,
        exact_sum(&["10.10", "20.20", "30.30", "40.40", "50.50", "60.60", "70.70", "80.80",]),
    );
    assert_eq!(statement.grand_total, statement.bill_total);
    assert!(statement.unallocated.is_zero());
}

#[test]
fn aged_and_unaged_subtotals_reconcile_to_the_exact_bill_total() {
    let bills = vec![
        bill(
            "Party",
            "AGED",
            "10.10",
            Some(5),
            ExposureDirection::Receivable,
        ),
        bill(
            "Party",
            "UNAGED",
            "20.20",
            None,
            ExposureDirection::Receivable,
        ),
    ];
    let statement = build_party_statement("Lab Co", "20260808", "Party", &bills, &[])
        .expect("party has exposure");

    assert_eq!(
        statement.subtotals.receivable.not_yet_due,
        exact_sum(&["20.20"])
    );
    assert_eq!(statement.subtotals.total().unwrap(), statement.bill_total);
}

#[test]
fn a_party_with_only_unallocated_exposure_and_no_bills_still_builds() {
    let unallocated_rows = vec![unallocated("On Account Only", "42.00")];
    let statement = build_party_statement(
        "Lab Co",
        "20260808",
        "On Account Only",
        &[],
        &unallocated_rows,
    )
    .expect("party has unallocated exposure");
    assert!(statement.bills.is_empty());
    assert!(statement.bill_total.is_zero());
    assert_eq!(statement.unallocated, ExactDecimal::parse("42.00").unwrap());
    assert_eq!(statement.grand_total, exact_sum(&["42.00"]));
}

#[test]
fn an_unknown_party_is_rejected_rather_than_producing_an_empty_statement() {
    let bills = vec![bill(
        "Known Party",
        "INV-1",
        "10.00",
        Some(5),
        ExposureDirection::Receivable,
    )];
    let error =
        build_party_statement("Lab Co", "20260808", "Unknown Party", &bills, &[]).unwrap_err();
    assert_eq!(error, PartyStatementError::PartyNotFound);
}

#[test]
fn a_party_with_a_zero_unallocated_residual_is_treated_as_having_none() {
    // `unallocated_by_party` already drops zero residuals upstream (see
    // `top_unallocated_parties`), but this guards the statement builder
    // itself against ever surfacing a zero as if it were real exposure.
    let bills = vec![bill(
        "Party",
        "INV-1",
        "10.00",
        Some(5),
        ExposureDirection::Receivable,
    )];
    let unallocated_rows = vec![unallocated("Party", "0")];
    let statement =
        build_party_statement("Lab Co", "20260808", "Party", &bills, &unallocated_rows).unwrap();
    assert!(statement.unallocated.is_zero());
    assert_eq!(statement.grand_total, statement.bill_total);
}
