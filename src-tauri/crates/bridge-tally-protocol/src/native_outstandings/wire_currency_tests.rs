use super::*;

const MODERN_LIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/currency_inr_modern_live.utf16le.xml"
));
const LEGACY_LIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/currency_inr_legacy_live.utf16le.xml"
));
const MULTI_LIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/currency_multi_live.utf16le.xml"
));
const FOREX_COMPOSITE_LIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ledgers_forex_composite_live.utf16le.xml"
));

fn decode_utf16le(bytes: &[u8]) -> String {
    let (units, remainder) = bytes.as_chunks::<2>();
    assert!(
        remainder.is_empty(),
        "captured UTF-16LE must have whole units"
    );
    String::from_utf16(
        &units
            .iter()
            .map(|unit| u16::from_le_bytes(*unit))
            .collect::<Vec<_>>(),
    )
    .expect("captured UTF-16LE must decode")
}

#[test]
fn a_duplicated_attribute_on_a_currency_row_is_refused() {
    // The currency row is the third element in this file read through the
    // attribute helpers, and it is guarded for the same reason the group
    // and ledger rows are: `.flatten()` drops the `Err` quick-xml raises
    // for a repeated attribute, leaving whichever value came first
    // silently in effect. No finding pointed here — the guard closes the
    // class rather than the one instance that was reported.
    let xml = concat!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>"#,
        r#"<CURRENCY NAME="I₹" NAME="$"><MAILINGNAME>INR</MAILINGNAME>"#,
        r#"<DECIMALPLACES>2</DECIMALPLACES></CURRENCY>"#,
        r#"</COLLECTION></DATA></BODY></ENVELOPE>"#,
    );
    assert_eq!(
        parse_company_currency(xml),
        Err(NativeOutstandingsError::InvalidResponse(
            "currency_row_malformed_attributes"
        ))
    );
}

#[test]
fn captured_currency_collections_recognize_both_indian_spellings_without_guessing() {
    for (bytes, sha256, symbol, mailing_name, count, decimal_places, is_inr) in [
        (
            MODERN_LIVE,
            "0dc84aa287cab1e1922db7e99a01f9f2b0bacd0d777fdd0b080adedc6622ed22",
            "I₹",
            "INR",
            1,
            2,
            true,
        ),
        (
            LEGACY_LIVE,
            "dcc3539205080c4272b42d333b693e6c90e1cdd6b9e9e080d4ea6b8ae2abb06e",
            "Rs.",
            "Indian Rupees",
            1,
            2,
            true,
        ),
        // Two masters: this read alone does not name the base currency, so no
        // row is reported as the base (before bridge#551 the first row, `$`,
        // was).
        (
            MULTI_LIVE,
            "b64c0d5feb528fa02f81de576de5c766a95e1da1000975b1e2932868ae34118b",
            "",
            "",
            2,
            0,
            false,
        ),
    ] {
        assert_eq!(sha256_hex(bytes), sha256, "captured wire bytes changed");
        let currency = parse_company_currency(&decode_utf16le(bytes)).expect("parses");
        assert_eq!(currency.symbol, symbol);
        assert_eq!(currency.mailing_name, mailing_name);
        assert_eq!(currency.currency_count, count, "CMPINFO is not a row");
        assert_eq!(currency.decimal_places, decimal_places);
        assert_eq!(currency.is_inr, is_inr);
    }
}

#[test]
fn several_currencies_cannot_name_the_base_currency() {
    let xml = decode_utf16le(LEGACY_LIVE).replace(
        "</COLLECTION>",
        r#"<CURRENCY NAME="$" RESERVEDNAME=""><MAILINGNAME TYPE="String">US Dollars</MAILINGNAME><DECIMALPLACES TYPE="Number">2</DECIMALPLACES></CURRENCY></COLLECTION>"#,
    );
    let currency = parse_company_currency(&xml).expect("parses");
    assert_eq!(currency.currency_count, 2);
    assert!(!currency.is_inr, "must fall back to asking, never guess");
}

#[test]
fn a_non_indian_single_currency_is_not_inr() {
    // Constructed: no captured company has this single-currency shape.
    let xml = decode_utf16le(LEGACY_LIVE)
        .replace("Indian Rupees", "US Dollars")
        .replace(r#"NAME="Rs.""#, r#"NAME="$""#);
    let currency = parse_company_currency(&xml).expect("parses");
    assert!(!currency.is_inr);
}

#[test]
fn failed_or_structurally_incomplete_currency_collections_fail_closed() {
    for xml in [
        "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>failed</LINEERROR></DATA></BODY></ENVELOPE>",
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA/></BODY></ENVELOPE>",
        "<not-xml",
    ] {
        assert!(parse_company_currency(xml).is_err(), "{xml}");
    }
}

#[test]
fn currency_precision_is_required_and_typed_at_the_wire_boundary() {
    let missing = decode_utf16le(LEGACY_LIVE)
        .replace(r#"<DECIMALPLACES TYPE="Number"> 2</DECIMALPLACES>"#, "");
    assert_eq!(
        parse_company_currency(&missing),
        Err(NativeOutstandingsError::InvalidResponse(
            "currency_decimal_places_missing"
        ))
    );

    let invalid = decode_utf16le(LEGACY_LIVE).replace(
        r#"<DECIMALPLACES TYPE="Number"> 2</DECIMALPLACES>"#,
        r#"<DECIMALPLACES TYPE="Number">fractional</DECIMALPLACES>"#,
    );
    assert_eq!(
        parse_company_currency(&invalid),
        Err(NativeOutstandingsError::InvalidResponse(
            "currency_decimal_places_invalid"
        ))
    );
}

#[test]
fn common_rs_symbol_does_not_prove_indian_rupees() {
    let xml = decode_utf16le(LEGACY_LIVE).replace("Indian Rupees", "Pakistani Rupees");
    let currency = parse_company_currency(&xml).expect("shaped collection parses");
    assert!(!currency.is_inr);
}

#[test]
fn captured_forex_composite_closing_balance_names_the_ledger_without_parsing_it() {
    assert_eq!(
        sha256_hex(FOREX_COMPOSITE_LIVE),
        "4941f30826ec51da9ab1c834abb1abcd711ffec22464044d5c669b77aaa313f8",
        "captured wire bytes changed"
    );
    let xml = decode_utf16le(FOREX_COMPOSITE_LIVE);
    assert_eq!(xml.matches("<LEDGER NAME=").count(), 8);
    assert_eq!(xml.matches(" @ ").count(), 1);
    assert_eq!(
        parse_native_ledger_snapshot(&xml),
        Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance {
            ledger_name: "FX USD Debtor 02".to_string(),
        })
    );
}

#[test]
fn ledger_snapshot_retains_an_empty_closing_balance_distinct_from_zero() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
            <LEDGER NAME=\"Empty\"><PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE></CLOSINGBALANCE>\
            <OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
            <LEDGER NAME=\"Zero\"><PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>0</CLOSINGBALANCE>\
            <OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
            </COLLECTION></DATA></BODY></ENVELOPE>";
    let rows = parse_native_ledger_snapshot(xml).expect("the observed empty element is valid XML");
    assert_eq!(rows[0].closing_balance, None);
    assert_eq!(rows[1].closing_balance, Some(ExactDecimal::zero()));
}

#[test]
fn party_ledger_master_balance_response_with_only_foreign_company_guids_is_withheld() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
            <LEDGER NAME=\"Same ledger name\"><GUID>22222222-2222-2222-2222-222222222222-00000001</GUID>\
            <BRIDGECOMPANYGUID>22222222-2222-2222-2222-222222222222</BRIDGECOMPANYGUID>\
            <PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>-100.00</CLOSINGBALANCE>\
            <OPENINGBALANCE>-100.00</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
            </COLLECTION></DATA></BODY></ENVELOPE>";

    assert_eq!(
        parse_native_ledger_snapshot_for_company(xml, "11111111-1111-1111-1111-111111111111"),
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_response_company_guid_mismatch"
        ))
    );
    assert_eq!(
        parse_native_ledger_snapshot(xml).unwrap().len(),
        1,
        "ordinary snapshot consumers retain their documented, identity-neutral parser"
    );
    assert_eq!(
        parse_native_ledger_snapshot_for_company(xml, "22222222-2222-2222-2222-222222222222")
            .unwrap()
            .len(),
        1,
        "a matching row GUID binds the export response before it is joined"
    );
}

#[test]
fn party_ledger_master_balance_response_requires_its_own_company_identity() {
    const EXPECTED: &str = "11111111-1111-1111-1111-111111111111";
    let response = |company_guid: Option<&str>| {
        let company_guid = company_guid
            .map(|guid| format!("<BRIDGECOMPANYGUID>{guid}</BRIDGECOMPANYGUID>"))
            .unwrap_or_default();
        format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
                <LEDGER NAME=\"Imported selected ledger\"><GUID>{EXPECTED}-00000001</GUID>{company_guid}\
                <PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>-100.00</CLOSINGBALANCE>\
                <OPENINGBALANCE>-100.00</OPENINGBALANCE><ISBILLWISEON>Yes</ISBILLWISEON></LEDGER>\
                </COLLECTION></DATA></BODY></ENVELOPE>"
        )
    };

    assert_eq!(
        parse_native_ledger_snapshot_for_company(
            &response(Some("22222222-2222-2222-2222-222222222222")),
            EXPECTED,
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_response_company_guid_mismatch"
        )),
        "a selected-prefix imported ledger cannot prove which company answered"
    );
    assert_eq!(
        parse_native_ledger_snapshot_for_company(&response(None), EXPECTED),
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_response_company_guid_missing"
        )),
        "a response without Tally's computed company GUID is withheld"
    );
    assert_eq!(
        parse_native_ledger_snapshot_for_company(&response(Some(EXPECTED)), EXPECTED)
            .unwrap()
            .len(),
        1,
        "the response-bound company GUID, not a row GUID prefix, admits the snapshot"
    );
}

#[test]
fn constructed_forex_composite_boundaries_remain_fail_closed() {
    let xml = decode_utf16le(FOREX_COMPOSITE_LIVE);
    // Constructed: the capture contains only the measured negative composite balance.
    let positive = xml.replacen(
        "-$ 2000.00 @ I₹ 84/$  = -I₹ 168000.00",
        "$ 2000.00 @ I₹ 84/$  = I₹ 168000.00",
        1,
    );
    assert!(matches!(
        parse_native_ledger_snapshot(&positive),
        Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance { ledger_name })
            if ledger_name == "FX USD Debtor 02"
    ));

    // Constructed: a near-miss without a base-currency tail is invalid, not foreign currency.
    let malformed = xml.replacen(" @ I₹ 84/$  = ", " @ I₹ 84/$ ", 1);
    assert_eq!(
        parse_native_ledger_snapshot(&malformed),
        Err(NativeOutstandingsError::InvalidAmount)
    );
}

/// The captured two-currency book (an INR base with `$` added), as the currency
/// read now returns it: each master also carries its `ORIGINALNAME`. The
/// values are the ones measured on 22 Sep 2026 (bridge#551): master `I₹` has
/// `ORIGINALNAME` `₹`, and master `$` has `$`.
fn multi_with_original_names() -> String {
    let xml = decode_utf16le(MULTI_LIVE);
    let with_names = xml
        .replacen(
            "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
            "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME><ORIGINALNAME TYPE=\"String\">$</ORIGINALNAME>",
            1,
        )
        .replacen(
            "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
            "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME><ORIGINALNAME TYPE=\"String\">₹</ORIGINALNAME>",
            1,
        );
    assert_eq!(
        with_names.matches("<ORIGINALNAME").count(),
        2,
        "both rows gain a name"
    );
    with_names
}

#[test]
fn the_base_among_several_masters_is_the_one_whose_original_name_is_the_company_currency() {
    let masters = parse_company_currency(&multi_with_original_names()).expect("parses");
    assert_eq!(masters.currency_count, 2);
    assert!(!masters.base_determined);
    assert_eq!(
        masters.inr_admission(),
        Err("company_base_currency_undetermined")
    );

    // An INR-based book with a second currency: its company CURRENCYNAME is
    // the INR master's ORIGINALNAME.
    let inr = masters.clone().with_company_currency_name("₹");
    assert!(inr.base_determined && inr.is_inr);
    assert_eq!(
        (inr.symbol.as_str(), inr.mailing_name.as_str()),
        ("I₹", "INR")
    );
    assert_eq!(inr.inr_admission(), Ok(()));

    // A USD-based book: the base is found, and it is not INR.
    let usd = masters.clone().with_company_currency_name("$");
    assert!(usd.base_determined && !usd.is_inr);
    assert_eq!(usd.symbol, "$");
    assert_eq!(usd.inr_admission(), Err("company_base_currency_not_inr"));

    // The master's NAME is not the match: `I₹` names a master, but no
    // master's ORIGINALNAME is `I₹`.
    for unmatched in ["I₹", "Rs.", "", "₹ "] {
        let currency = masters.clone().with_company_currency_name(unmatched);
        assert!(!currency.base_determined, "{unmatched:?}");
        assert_eq!(
            currency.inr_admission(),
            Err("company_base_currency_undetermined"),
            "{unmatched:?}"
        );
    }
}

#[test]
fn two_masters_with_the_same_original_name_leave_the_base_undetermined() {
    let xml = multi_with_original_names().replacen(
        "<ORIGINALNAME TYPE=\"String\">$</ORIGINALNAME>",
        "<ORIGINALNAME TYPE=\"String\">₹</ORIGINALNAME>",
        1,
    );
    let currency = parse_company_currency(&xml)
        .expect("parses")
        .with_company_currency_name("₹");
    assert!(!currency.base_determined);
    assert_eq!(
        currency.inr_admission(),
        Err("company_base_currency_undetermined")
    );
}

#[test]
fn masters_read_without_original_names_stay_undetermined() {
    let currency = parse_company_currency(&decode_utf16le(MULTI_LIVE))
        .expect("parses")
        .with_company_currency_name("₹");
    assert!(!currency.base_determined);
}

#[test]
fn a_single_master_is_the_base_and_ignores_the_company_currency_name() {
    for bytes in [MODERN_LIVE, LEGACY_LIVE] {
        let currency = parse_company_currency(&decode_utf16le(bytes)).expect("parses");
        assert!(currency.base_determined && currency.is_inr);
        assert_eq!(currency.inr_admission(), Ok(()));
        assert_eq!(currency.clone().with_company_currency_name("$"), currency);
    }
    let empty = CompanyCurrency::from_masters(Vec::new());
    assert_eq!(empty.inr_admission(), Err("company_currency_probe_failed"));
}

#[test]
fn a_repeated_original_name_on_one_master_is_refused() {
    let xml = multi_with_original_names().replacen(
        "<ORIGINALNAME TYPE=\"String\">$</ORIGINALNAME>",
        "<ORIGINALNAME TYPE=\"String\">$</ORIGINALNAME><ORIGINALNAME TYPE=\"String\">₹</ORIGINALNAME>",
        1,
    );
    assert_eq!(
        parse_company_currency(&xml),
        Err(NativeOutstandingsError::InvalidResponse(
            "currency_duplicate_original_name"
        ))
    );
}

/// A synthetic Company collection in the shape measured on 22 Sep 2026: every
/// loaded company is listed, each with its GUID and CURRENCYNAME. The names
/// and GUIDs are invented.
fn company_collection(rows: &[(&str, &str, Option<&str>)]) -> String {
    let body: String = rows
        .iter()
        .map(|(name, guid, currency)| {
            let currency = currency.map_or(String::new(), |value| {
                format!("<CURRENCYNAME TYPE=\"String\">{value}</CURRENCYNAME>")
            });
            format!(
                "<COMPANY NAME=\"{name}\" RESERVEDNAME=\"\"><GUID TYPE=\"String\">{guid}</GUID>{currency}<NUMCURRENCIES TYPE=\"Number\">2</NUMCURRENCIES></COMPANY>"
            )
        })
        .collect();
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO><COMPANY>0</COMPANY></CMPINFO></DESC><DATA><COLLECTION>{body}</COLLECTION></DATA></BODY></ENVELOPE>"
    )
}

#[test]
fn the_company_currency_name_is_taken_from_the_row_with_the_company_guid() {
    let xml = company_collection(&[
        (
            "Synthetic Other Book",
            "11111111-aaaa-4000-8000-000000000001",
            Some("Rs."),
        ),
        (
            "Synthetic Forex Book",
            "22222222-BBBB-4000-8000-000000000002",
            Some("₹"),
        ),
        (
            "Synthetic Usd Book",
            "33333333-cccc-4000-8000-000000000003",
            Some("$"),
        ),
    ]);
    assert_eq!(
        parse_company_currency_name(&xml, "22222222-bbbb-4000-8000-000000000002"),
        Ok("₹".to_string())
    );
    assert_eq!(
        parse_company_currency_name(&xml, "33333333-cccc-4000-8000-000000000003"),
        Ok("$".to_string())
    );
    for (xml, guid, code) in [
        (
            xml.clone(),
            "44444444-dddd-4000-8000-000000000004",
            "company_currency_row_missing",
        ),
        (
            company_collection(&[
                ("A", "55555555-eeee-4000-8000-000000000005", Some("₹")),
                ("B", "55555555-EEEE-4000-8000-000000000005", Some("$")),
            ]),
            "55555555-eeee-4000-8000-000000000005",
            "company_currency_row_ambiguous",
        ),
        (
            company_collection(&[("A", "66666666-ffff-4000-8000-000000000006", None)]),
            "66666666-ffff-4000-8000-000000000006",
            "company_currency_name_missing",
        ),
        (
            company_collection(&[("A", "66666666-ffff-4000-8000-000000000006", Some(""))]),
            "66666666-ffff-4000-8000-000000000006",
            "company_currency_name_missing",
        ),
    ] {
        assert_eq!(
            parse_company_currency_name(&xml, guid),
            Err(NativeOutstandingsError::InvalidResponse(code)),
            "{code}"
        );
    }
    let failed = company_collection(&[]).replace("<STATUS>1</STATUS>", "<STATUS>0</STATUS>");
    assert_eq!(
        parse_company_currency_name(&failed, "x"),
        Err(NativeOutstandingsError::TallyReportedFailure)
    );
}

#[test]
fn a_company_row_repeating_its_guid_or_currency_name_or_a_response_without_status_is_refused() {
    let guid = "77777777-aaaa-4000-8000-000000000007";
    let row = company_collection(&[("A", guid, Some("₹"))]);
    let repeated_guid = row.replace(
        "</GUID>",
        "</GUID><GUID TYPE=\"String\">88888888-bbbb-4000-8000-000000000008</GUID>",
    );
    let repeated_name = row.replace(
        "</CURRENCYNAME>",
        "</CURRENCYNAME><CURRENCYNAME TYPE=\"String\">$</CURRENCYNAME>",
    );
    let no_status = row.replace("<STATUS>1</STATUS>", "");
    assert_eq!(
        parse_company_currency_name(&repeated_guid, guid),
        Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_duplicate_guid"
        ))
    );
    assert_eq!(
        parse_company_currency_name(&repeated_name, guid),
        Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_duplicate_name"
        ))
    );
    assert_eq!(
        parse_company_currency_name(&no_status, guid),
        Err(NativeOutstandingsError::TallyReportedFailure)
    );
    // A nested element under the row is skipped, not read as the row's field.
    let nested = row.replace(
        "</COMPANY>",
        "<ADDRESS.LIST><CURRENCYNAME TYPE=\"String\">$</CURRENCYNAME></ADDRESS.LIST></COMPANY>",
    );
    assert_eq!(
        parse_company_currency_name(&nested, guid),
        Ok("₹".to_string())
    );
}
