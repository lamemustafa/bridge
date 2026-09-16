use super::*;

fn decimal(value: &str) -> ExactDecimal {
    ExactDecimal::parse(value).expect("synthetic exact decimal")
}

fn bill(
    party: &str,
    reference: &str,
    bill_date: &str,
    due_date: &str,
    amount: &str,
    kind: ExposureDirection,
) -> OpenBillRow {
    OpenBillRow {
        party: party.to_string(),
        reference: reference.to_string(),
        bill_date: bill_date.to_string(),
        due_date: due_date.to_string(),
        amount: decimal(amount),
        age_days: None,
        kind,
    }
}

fn source() -> OutstandingsWorkingPaperSource {
    let mut source = OutstandingsWorkingPaperSource {
        company: "Synthetic Books".to_string(),
        company_guid: "synthetic-guid".to_string(),
        as_of_yyyymmdd: "20260825".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        synced_at_unix_ms: 1_777_000_000_000,
        source_bytes: 1_024,
        source_ageing_anchor: OutstandingsAgeingAnchor::DueDate,
        receivable_bill_total: decimal("125.00"),
        payable_bill_total: decimal("40.00"),
        unallocated_total: decimal("17.00"),
        open_bills: vec![
            bill(
                "A Party",
                "A-1",
                "20260501",
                "20260601",
                "125.00",
                ExposureDirection::Receivable,
            ),
            bill(
                "A Party",
                "P-1",
                "20260801",
                "20260901",
                "40.00",
                ExposureDirection::Payable,
            ),
        ],
        unallocated_by_party: vec![
            UnallocatedParty {
                party: "A Party".to_string(),
                amount: decimal("7.00"),
                direction: ExposureDirection::Receivable,
            },
            UnallocatedParty {
                party: "Unallocated Only".to_string(),
                amount: decimal("10.00"),
                direction: ExposureDirection::Payable,
            },
        ],
    };
    source.open_bills[0].age_days = Some(85);
    source
}

#[test]
fn builds_dual_ages_and_keeps_directions_separate() {
    let paper = build_outstandings_working_paper(source()).expect("paper builds");
    let first = paper
        .bills
        .iter()
        .find(|row| row.reference == "A-1")
        .expect("first bill");
    assert_eq!(first.bill_age_days, Some(116));
    assert_eq!(first.due_age_days, Some(85));
    let future_due = paper
        .bills
        .iter()
        .find(|row| row.reference == "P-1")
        .expect("future-due bill");
    assert_eq!(future_due.bill_age_days, Some(24));
    assert_eq!(future_due.due_age_days, None);

    let mixed = paper
        .parties
        .iter()
        .find(|row| row.party == "A Party")
        .expect("mixed party");
    assert_eq!(mixed.receivable_total.as_str(), "132");
    assert_eq!(mixed.payable_total.as_str(), "40");
    assert_eq!(mixed.outstanding_total.as_str(), "172");
    let unallocated_only = paper
        .parties
        .iter()
        .find(|row| row.party == "Unallocated Only")
        .expect("unallocated-only party");
    assert_eq!(unallocated_only.oldest_bill_age_days, None);
    assert_eq!(unallocated_only.oldest_due_age_days, None);
    assert_eq!(
        paper
            .controls
            .bill_date_ageing
            .receivable
            .days_90_plus
            .as_str(),
        "125"
    );
    assert_eq!(
        paper
            .controls
            .due_date_ageing
            .receivable
            .days_61_90
            .as_str(),
        "125"
    );
    assert_eq!(
        paper
            .controls
            .due_date_ageing
            .payable
            .date_not_reached
            .as_str(),
        "40"
    );
    assert_eq!(paper.controls.receivable_total.as_str(), "132");
    assert_eq!(paper.controls.payable_total.as_str(), "50");
    assert_eq!(paper.controls.outstanding_total.as_str(), "182");
}

#[test]
fn rejects_malformed_date_and_control_mismatches() {
    let mut malformed = source();
    malformed.open_bills[0].bill_date = "20260230".to_string();
    assert!(matches!(
        build_outstandings_working_paper(malformed),
        Err(OutstandingsWorkingPaperError::InvalidDate(_))
    ));

    let mut bill_mismatch = source();
    bill_mismatch.receivable_bill_total = decimal("124.99");
    assert_eq!(
        build_outstandings_working_paper(bill_mismatch),
        Err(OutstandingsWorkingPaperError::ControlMismatch(
            "receivable bill"
        ))
    );

    let mut residual_mismatch = source();
    residual_mismatch.unallocated_total = decimal("16.99");
    assert_eq!(
        build_outstandings_working_paper(residual_mismatch),
        Err(OutstandingsWorkingPaperError::ControlMismatch(
            "unallocated"
        ))
    );
}

#[test]
fn rejects_negative_magnitudes() {
    let mut negative = source();
    negative.open_bills[0].amount = decimal("-1");
    assert_eq!(
        build_outstandings_working_paper(negative),
        Err(OutstandingsWorkingPaperError::NegativeAmount)
    );
}

#[test]
fn rejects_zero_value_source_rows() {
    let mut zero = source();
    zero.open_bills[0].amount = ExactDecimal::zero();
    assert_eq!(
        build_outstandings_working_paper(zero),
        Err(OutstandingsWorkingPaperError::ZeroExposureRow)
    );
}

#[test]
fn rejects_exact_arithmetic_overflow() {
    let mut overflow = source();
    let huge = decimal(&"9".repeat(bridge_tally_core::MAX_EXACT_DECIMAL_BYTES));
    overflow.open_bills = vec![
        bill(
            "Overflow Party",
            "BIG",
            "20260801",
            "20260801",
            huge.as_str(),
            ExposureDirection::Receivable,
        ),
        bill(
            "Overflow Party",
            "ONE",
            "20260801",
            "20260801",
            "1",
            ExposureDirection::Receivable,
        ),
    ];
    for row in &mut overflow.open_bills {
        row.age_days = Some(24);
    }
    overflow.payable_bill_total = ExactDecimal::zero();
    overflow.unallocated_total = ExactDecimal::zero();
    overflow.unallocated_by_party.clear();
    assert_eq!(
        build_outstandings_working_paper(overflow),
        Err(OutstandingsWorkingPaperError::ArithmeticOverflow)
    );
}

#[test]
fn rejects_duplicate_residuals_and_selected_age_disagreement() {
    let mut duplicate = source();
    duplicate
        .unallocated_by_party
        .push(duplicate.unallocated_by_party[0].clone());
    duplicate.unallocated_total = decimal("24.00");
    assert_eq!(
        build_outstandings_working_paper(duplicate),
        Err(OutstandingsWorkingPaperError::DuplicateUnallocatedParty)
    );

    let mut wrong_age = source();
    wrong_age.open_bills[0].age_days = Some(84);
    assert_eq!(
        build_outstandings_working_paper(wrong_age),
        Err(OutstandingsWorkingPaperError::SourceAgeMismatch)
    );
}

#[test]
fn bill_date_source_anchor_is_cross_checked_independently() {
    let mut bill_anchor = source();
    bill_anchor.source_ageing_anchor = OutstandingsAgeingAnchor::BillDate;
    bill_anchor.open_bills[0].age_days = Some(116);
    bill_anchor.open_bills[1].age_days = Some(24);
    assert!(build_outstandings_working_paper(bill_anchor).is_ok());
}

#[test]
fn exact_age_boundaries_are_preserved() {
    let as_of = TallyDate::parse("20260825").expect("synthetic as-of");
    for (date, expected) in [
        ("20260726", 30),
        ("20260725", 31),
        ("20260626", 60),
        ("20260625", 61),
        ("20260527", 90),
        ("20260526", 91),
    ] {
        assert_eq!(
            age_on_or_before(&TallyDate::parse(date).expect("synthetic date"), &as_of),
            Ok(Some(expected))
        );
    }
}
