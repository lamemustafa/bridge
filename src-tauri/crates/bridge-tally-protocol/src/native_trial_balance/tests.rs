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

/// The plain request is byte-for-byte the request sent before the currency
/// variant existed, so a single-currency book's Trial Balance is unchanged
/// (bridge#551). The variant differs only by `CURRENCYNAME` in its `FETCH`.
#[test]
fn the_currency_variant_differs_from_the_unchanged_request_only_by_currencyname() {
    let period = NativeLedgerSnapshotPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20260601").unwrap(),
        TallyDate::parse("20260731").unwrap(),
    )
    .unwrap();
    let plain = render_native_trial_balance_request("A & B <Co>", &period);
    assert_eq!(
        plain,
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>A &amp; B &lt;Co&gt;</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">20260601</SVFROMDATE><SVTODATE TYPE="Date">20260731</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>NAME, GUID, PARENT, TBALOPENING, DEBITTOTALS, CREDITTOTALS, TBALCLOSING</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#
    );
    let with_currency = render_native_trial_balance_request_with_currency("A & B <Co>", &period);
    assert_eq!(
        with_currency,
        plain.replace("TBALCLOSING</FETCH>", "TBALCLOSING, CURRENCYNAME</FETCH>")
    );
    assert_ne!(with_currency, plain);
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
    // Synthetic adversarial transformation, not an observed Tally response:
    // the empty credit bypasses row-equation validation, so polarity remains a
    // parser-boundary admission rather than a UI magnitude assumption.
    let wrong_sign_debit_with_empty_credit = KNOWN_LAB.replacen(
        "<DEBITTOTALS TYPE=\"Amount\">-4777.00</DEBITTOTALS>",
        "<DEBITTOTALS TYPE=\"Amount\">4777.00</DEBITTOTALS>",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&wrong_sign_debit_with_empty_credit, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_debit_polarity_invalid"
        ))
    );
    let wrong_sign_credit_with_empty_debit = KNOWN_LAB.replacen(
        "<CREDITTOTALS TYPE=\"Amount\">1200.00</CREDITTOTALS>",
        "<CREDITTOTALS TYPE=\"Amount\">-1200.00</CREDITTOTALS>",
        1,
    );
    assert_eq!(
        parse_native_trial_balance(&wrong_sign_credit_with_empty_debit, COMPANY),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_credit_polarity_invalid"
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

// -- bridge#551: a several-currency book's Trial Balance ----------------------

const FOREX_COMPANY: &str = "b14e9b2d-8a63-4779-804d-25d59eb787eb";

fn forex_capture(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn forex_with_currency() -> String {
    forex_capture(include_bytes!(
        "../../tests/fixtures/trial_balance_currency_forex_live.utf16le.xml"
    ))
}

fn forex_base() -> crate::native_outstandings::BaseCurrencyName {
    crate::native_outstandings::BaseCurrencyName::among_several_for_tests("I\u{20b9}")
}

/// Every value the captured response holds in a composite, read from the bytes.
fn captured_composites(capture: &str) -> Vec<String> {
    ["TBALOPENING", "DEBITTOTALS", "CREDITTOTALS", "TBALCLOSING"]
        .iter()
        .flat_map(|tag| {
            capture
                .split(&format!("<{tag} "))
                .skip(1)
                .filter_map(|tail| {
                    let text = &tail[tail.find('>')? + 1..tail.find(&format!("</{tag}>"))?];
                    text.contains(" @ ").then(|| text.to_string())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The captured several-currency Trial Balance: foreign ledgers and rupee
/// ledgers with a composite value are set aside by name, and only the plain
/// rupee rows are read.
#[test]
fn a_several_currency_trial_balance_reads_only_its_plain_base_rows() {
    let scoped = parse_native_trial_balance_with_currency(
        &forex_with_currency(),
        FOREX_COMPANY,
        &forex_base(),
    )
    .unwrap();
    let foreign = scoped
        .foreign_currency_ledgers
        .iter()
        .map(|ledger| (ledger.ledger.as_str(), ledger.currency.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        foreign,
        [
            ("BRIDGE FX DEBTOR A", "$"),
            ("FX USD Debtor 01", "$"),
            ("FX USD Debtor 02", "$"),
        ]
    );
    // The empty-rate closing (`$ 0.00 @ I₹ /$  = I₹ 0.00`) is set aside too.
    assert_eq!(
        scoped.mixed_currency_ledgers,
        ["FX Party 01", "FX Sales", "Profit & Loss A/c"]
    );
    let read = scoped
        .report
        .rows
        .iter()
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        read,
        ["BRIDGE INR DEBTOR A", "Cash", "FX Party 02", "FX Party 03"]
    );
}

/// No value is ever read from a composite: every row that is read holds only
/// plain decimals or empty amounts, and every composite in the capture sits on
/// a row set aside.
#[test]
fn no_trial_balance_value_is_read_from_a_composite() {
    let capture = forex_with_currency();
    let composites = captured_composites(&capture);
    assert_eq!(composites.len(), 11, "{composites:?}");
    for composite in &composites {
        assert!(
            super::scalar::is_currency_composite(composite),
            "{composite}"
        );
    }
    let scoped =
        parse_native_trial_balance_with_currency(&capture, FOREX_COMPANY, &forex_base()).unwrap();
    let composite_rows = capture
        .split("<LEDGER NAME=\"")
        .skip(1)
        .filter(|row| {
            let body = &row[..row.find("</LEDGER>").unwrap()];
            composites.iter().any(|value| body.contains(value.as_str()))
        })
        .map(|row| row[..row.find('"').unwrap()].replace("&amp;", "&"))
        .collect::<Vec<_>>();
    assert_eq!(composite_rows.len(), 5, "{composite_rows:?}");
    for row in &scoped.report.rows {
        assert!(!composite_rows.contains(&row.name), "{row:?}");
    }
    // Control: the single-currency parser still refuses this response.
    assert_eq!(
        parse_native_trial_balance(&capture, FOREX_COMPANY),
        Err(NativeTrialBalanceError::InvalidAmount)
    );
}

/// The classifier admits only the composite shape. A captured composite cut
/// short, doubled or with a non-ASCII digit is not one, so a base row holding
/// it is refused as an invalid amount rather than set aside.
#[test]
fn a_damaged_composite_is_refused_not_set_aside() {
    use super::scalar::is_currency_composite;
    let capture = forex_with_currency();
    let empty_rate = captured_composites(&capture)
        .into_iter()
        .find(|value| value.contains(" /$"))
        .expect("the captured empty-rate composite");
    assert!(is_currency_composite(&empty_rate));
    let cut = &empty_rate[..empty_rate.find(" = ").unwrap()];
    for damaged in [
        cut.to_string(),
        format!("{empty_rate} @ $ 1/$"),
        empty_rate.replace("0.00", "\u{0660}.00"),
        empty_rate.replacen("$ ", "", 1),
    ] {
        assert!(!is_currency_composite(&damaged), "{damaged}");
    }
    // Through the parser: the rupee P&L row with its closing cut short.
    let damaged = capture.replacen(&empty_rate, cut, 1);
    assert_eq!(
        parse_native_trial_balance_with_currency(&damaged, FOREX_COMPANY, &forex_base()),
        Err(NativeTrialBalanceError::InvalidAmount)
    );
}

/// Every row of a several-currency read must name its currency.
#[test]
fn a_row_without_its_currency_is_refused() {
    let capture = forex_with_currency();
    let named = "<CURRENCYNAME TYPE=\"String\">I\u{20b9}</CURRENCYNAME>";
    assert!(capture.contains(named), "captured CURRENCYNAME shape");
    let missing = capture.replacen(named, "", 1);
    assert_eq!(
        parse_native_trial_balance_with_currency(&missing, FOREX_COMPANY, &forex_base()),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_currency_missing"
        ))
    );
}

/// A row set aside still binds to the selected company: a foreign row whose
/// GUID names another company refuses the whole read.
#[test]
fn a_set_aside_row_still_binds_to_the_company() {
    let capture = forex_with_currency();
    let foreign_row = capture.find("<LEDGER NAME=\"BRIDGE FX DEBTOR A\"").unwrap();
    let guid_at = foreign_row + capture[foreign_row..].find(FOREX_COMPANY).unwrap();
    let mut other = capture.clone();
    other.replace_range(guid_at..guid_at + 8, "00000000");
    assert_eq!(
        parse_native_trial_balance_with_currency(&other, FOREX_COMPANY, &forex_base()),
        Err(NativeTrialBalanceError::InvalidResponse(
            "trial_balance_company_guid_mismatch"
        ))
    );
}
