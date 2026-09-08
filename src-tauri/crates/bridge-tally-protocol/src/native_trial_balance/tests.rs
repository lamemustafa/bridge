use super::*;
use crate::native_outstandings::NativeLedgerSnapshotPeriod;
use crate::outstandings_shared::DateBoundaryProfile;
use bridge_tally_primitives::{ExactDecimal, TallyDate};

const COMPANY: &str = "eebb9a9f-1679-4468-9e8f-814c729674cb";
const KNOWN_LAB: &str = include_str!("../../tests/fixtures/native/trial_balance_known_lab.xml");
const OPENING_YEAR: &str =
    include_str!("../../tests/fixtures/native/trial_balance_opening_year.xml");

#[test]
fn request_binds_both_admitted_snapshot_boundaries_and_escapes_company() {
    let period = NativeLedgerSnapshotPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20260601").unwrap(),
        TallyDate::parse("20260731").unwrap(),
    )
    .unwrap();
    let request = render_native_trial_balance_request("A & B <Co>", &period);
    assert!(request.contains("<SVCURRENTCOMPANY>A &amp; B &lt;Co&gt;</SVCURRENTCOMPANY>"));
    assert!(request.contains("<SVFROMDATE TYPE=\"Date\">20260601</SVFROMDATE>"));
    assert!(request.contains("<SVTODATE TYPE=\"Date\">20260731</SVTODATE>"));
    assert!(request.contains("TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING"));
    assert!(!request.contains("CLOSINGBALANCE"));
}

#[test]
fn captured_trial_balance_preserves_empty_and_nonzero_openings() {
    let report = parse_native_trial_balance(KNOWN_LAB, COMPANY).unwrap();
    assert_eq!(report.rows.len(), 6);
    assert_eq!(
        report.rows[0].opening,
        NativeTrialBalanceAmount::Present(ExactDecimal::parse("0.00").unwrap())
    );
    assert_eq!(report.rows[0].debit, NativeTrialBalanceAmount::PresentEmpty);
    assert_eq!(
        report.rows[0].credit,
        NativeTrialBalanceAmount::PresentEmpty
    );
    assert_eq!(
        report.rows[0].closing,
        NativeTrialBalanceAmount::PresentEmpty
    );
    assert_eq!(
        report.rows[0].parent.returned_text(),
        Some("Sundry Debtors")
    );
    let escaped_parent = KNOWN_LAB.replacen(
        "<PARENT TYPE=\"String\">Sundry Debtors</PARENT>",
        "<PARENT TYPE=\"String\">A &amp; B</PARENT>",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&escaped_parent, COMPANY)
            .unwrap()
            .rows[0]
            .parent
            .returned_text(),
        Some("A & B")
    );
    let profit_and_loss = report
        .rows
        .iter()
        .find(|row| row.name == "Profit & Loss A/c")
        .unwrap();
    assert_eq!(
        profit_and_loss.closing,
        NativeTrialBalanceAmount::Present(ExactDecimal::parse("7000.00").unwrap())
    );

    let opening =
        parse_native_trial_balance(OPENING_YEAR, "915d42f8-42ae-4b03-8291-55f596e3a2ea").unwrap();
    assert!(opening.rows.iter().any(|row| row.opening
        == NativeTrialBalanceAmount::Present(ExactDecimal::parse("125000.00").unwrap())));
}

#[test]
fn captured_trial_balance_mutations_fail_closed() {
    for (mutation, expected) in [
        (
            KNOWN_LAB.replacen("<TBALCLOSING TYPE=\"Amount\"></TBALCLOSING>", "", 1),
            "trial_balance_closing_missing",
        ),
        (
            KNOWN_LAB.replacen(
                "<DEBITTOTALS TYPE=\"Amount\"></DEBITTOTALS>",
                "<DEBITTOTALS TYPE=\"Amount\">not-money</DEBITTOTALS>",
                1,
            ),
            "invalid_amount",
        ),
        (
            KNOWN_LAB.replacen(
                "eebb9a9f-1679-4468-9e8f-814c729674cb-000000d1",
                "wrong-company-000000d1",
                1,
            ),
            "trial_balance_company_guid_mismatch",
        ),
        (
            KNOWN_LAB.replacen(
                "eebb9a9f-1679-4468-9e8f-814c729674cb-000000d1",
                "eebb9a9f-1679-4468-9e8f-814c729674cb-000000ce",
                1,
            ),
            "trial_balance_duplicate_guid",
        ),
    ] {
        let error = parse_native_trial_balance(&mutation, COMPANY).unwrap_err();
        match (error, expected) {
            (NativeTrialBalanceError::InvalidAmount, "invalid_amount")
            | (
                NativeTrialBalanceError::InvalidResponse("trial_balance_closing_missing"),
                "trial_balance_closing_missing",
            )
            | (
                NativeTrialBalanceError::InvalidResponse("trial_balance_company_guid_mismatch"),
                "trial_balance_company_guid_mismatch",
            )
            | (
                NativeTrialBalanceError::InvalidResponse("trial_balance_duplicate_guid"),
                "trial_balance_duplicate_guid",
            ) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }

    let collection_start = KNOWN_LAB.find("<COLLECTION ").unwrap();
    let rows_start = KNOWN_LAB[collection_start..].find('>').unwrap() + collection_start + 1;
    let rows_end = KNOWN_LAB.find("</COLLECTION>").unwrap();
    let mut empty = KNOWN_LAB.to_owned();
    empty.replace_range(rows_start..rows_end, "");
    assert_eq!(
        parse_native_trial_balance(&empty, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_source_identity_missing"
        ))
    );

    let type_confusion = KNOWN_LAB.replacen(
        "<TBALOPENING TYPE=\"Amount\">0.00</TBALOPENING>",
        "<TBALOPENING TYPE=\"String\">0.00</TBALOPENING>",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&type_confusion, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_amount_type_invalid"
        ))
    );
    let duplicate_type = KNOWN_LAB.replacen(
        "<TBALOPENING TYPE=\"Amount\">0.00</TBALOPENING>",
        "<TBALOPENING TYPE=\"Amount\" TYPE=\"Amount\">0.00</TBALOPENING>",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&duplicate_type, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_attribute_malformed"
        ))
    );
    let duplicate_name = KNOWN_LAB.replacen(
        "<LEDGER NAME=\"Ageing Customer A\"",
        "<LEDGER NAME=\"Ageing Bank\"",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&duplicate_name, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_duplicate_name"
        ))
    );
    let concatenated = format!("{KNOWN_LAB}{KNOWN_LAB}");
    assert_eq!(
        parse_native_trial_balance(&concatenated, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_root_not_envelope"
        ))
    );
    let error = KNOWN_LAB.replacen("<DATA>", "<DATA><LINEERROR>reported failure</LINEERROR>", 1);
    assert_eq!(
        parse_native_trial_balance(&error, COMPANY),
        Err(NativeTrialBalanceError::TallyReportedFailure)
    );
    let nested_scalar = KNOWN_LAB.replacen(
        "<PARENT TYPE=\"String\">Sundry Debtors</PARENT>",
        "<PARENT TYPE=\"String\"><NAME>Sundry Debtors</NAME></PARENT>",
        1,
    );
    assert!(matches!(
        parse_native_trial_balance(&nested_scalar, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_scalar_not_text_only"
        ))
    ));
    let arithmetic_mismatch = KNOWN_LAB.replacen(
        "<TBALCLOSING TYPE=\"Amount\">-7277.00</TBALCLOSING>",
        "<TBALCLOSING TYPE=\"Amount\">-7278.00</TBALCLOSING>",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&arithmetic_mismatch, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_row_equation_mismatch"
        ))
    );
    for malformed_suffix in ["", "zzzzzzzz"] {
        let mutation = KNOWN_LAB.replacen(
            "eebb9a9f-1679-4468-9e8f-814c729674cb-000000d1",
            &format!("{COMPANY}-{malformed_suffix}"),
            1,
        );
        assert_eq!(
            parse_native_trial_balance(&mutation, COMPANY),
            Err(NativeTrialBalanceError::InvalidResponse(
                "trial_balance_guid_suffix_invalid"
            ))
        );
    }
}
