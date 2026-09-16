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
        (
            MULTI_LIVE,
            "b64c0d5feb528fa02f81de576de5c766a95e1da1000975b1e2932868ae34118b",
            "$",
            "USD",
            2,
            2,
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
