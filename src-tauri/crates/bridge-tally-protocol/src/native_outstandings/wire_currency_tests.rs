use super::*;
use crate::native_outstandings::model::identify_base_master;
use crate::native_outstandings::render_company_currency_request;

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

fn master(name: &str, original_name: Option<&str>, mailing_name: &str) -> CurrencyMaster {
    CurrencyMaster {
        name: name.to_string(),
        original_name: original_name.map(str::to_string),
        mailing_name: mailing_name.to_string(),
        decimal_places: 2,
    }
}

const ORIGINALNAME_FOREX_LIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/currency_originalname_forex_live.utf16le.xml"
));
const ORIGINALNAME_SHAPE_LIVE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/currency_originalname_shape_live.utf16le.xml"
));

/// bridge#551: the INR rule is the mailing name alone. `I₹`/`₹`/`INR` and
/// `Rs.`/`Indian Rupees` are the captured shapes; the rest are constructed.
/// A rupee symbol, as `NAME` or `ORIGINALNAME`, admits nothing without an
/// Indian mailing name, and `Rs.` alone never admits.
#[test]
fn the_inr_rule_admits_only_an_indian_mailing_name() {
    let rupee = "\u{20b9}";
    let prefixed = "I\u{20b9}";
    for (master, is_inr) in [
        (master(prefixed, Some(rupee), "INR"), true),
        (master(rupee, Some(rupee), "Indian Rupees"), true),
        (master("Rs.", None, "Indian Rupees"), true),
        (master("Rs.", None, "inr"), true),
        (master(prefixed, Some(rupee), ""), false),
        (master(rupee, Some(rupee), "Rupees"), false),
        (master(rupee, None, "Rupees"), false),
        (master("Rs.", None, ""), false),
        (master("Rs.", Some("Rs."), "Pakistani Rupees"), false),
        (master("$", Some("$"), "US Dollar"), false),
    ] {
        assert_eq!(master.is_inr(), is_inr, "{master:?}");
    }
}

/// bridge#551: the base is the only master, or the unique master whose
/// `ORIGINALNAME` is the company's `CURRENCYNAME`; otherwise none is.
/// Identifying a base never makes the company's currency INR:
/// `CompanyCurrency` admits only a single master.
#[test]
fn the_base_is_the_only_master_or_the_one_the_company_names() {
    let rupee = "\u{20b9}";
    let inr = master("I\u{20b9}", Some(rupee), "INR");
    let dollar = master("$", Some("$"), "US Dollar");
    let both = [dollar.clone(), inr.clone()];

    assert_eq!(
        identify_base_master(std::slice::from_ref(&dollar), None),
        Some(&dollar)
    );
    assert_eq!(identify_base_master(&[], Some(rupee)), None, "no masters");
    assert_eq!(identify_base_master(&both, Some(rupee)), Some(&inr));
    assert_eq!(identify_base_master(&both, Some("$")), Some(&dollar));
    for name in [None, Some(""), Some(" "), Some("I\u{20b9}"), Some("€")] {
        assert_eq!(identify_base_master(&both, name), None, "{name:?}");
    }
    assert_eq!(
        identify_base_master(&[inr.clone(), inr.clone()], Some(rupee)),
        None,
        "two masters answering the name identify neither"
    );
    // A blank company value names nothing, even a master whose ORIGINALNAME
    // is present and equally blank.
    for blank in ["", " "] {
        let unnamed = master(rupee, Some(blank), "INR");
        assert_eq!(
            identify_base_master(&[dollar.clone(), unnamed], Some(blank)),
            None,
            "{blank:?}"
        );
    }

    let single = CompanyCurrency::from_masters(std::slice::from_ref(&inr));
    assert!(single.is_inr);
    let symbol_only = CompanyCurrency::from_masters(&[master(rupee, Some(rupee), "Rupees")]);
    assert!(
        !symbol_only.is_inr,
        "a rupee symbol without an Indian mailing name"
    );
    let several = CompanyCurrency::from_masters(&[inr.clone(), dollar.clone()]);
    assert!(!several.is_inr, "an INR first master among several");
    assert_eq!(several.symbol, "I\u{20b9}");
    assert_eq!(several.currency_count, 2);
}

/// bridge#551: the captured two-master books, read with `ORIGINALNAME`
/// (TALLY_PROTOCOL_REFERENCE §9.10a.2).
/// - Through the production parser, each is still a book with two masters and
///   no INR base, as before: `parse_company_currency` takes no company name.
/// - Given the `₹` both books' company `CURRENCYNAME` carried, the rupee master
///   is identified as the base, and it is INR by its mailing name.
/// - `ORIGINALNAME` is read untrimmed: a padded `₹`, injected into the capture,
///   identifies nothing.
#[test]
fn captured_masters_carry_originalname_and_the_company_name_picks_the_base() {
    let rupee = "\u{20b9}";
    for (bytes, sha256, expected) in [
        (
            ORIGINALNAME_FOREX_LIVE,
            "07bd90682e88b1c5155afe7a1b0b5c541c89c8f660aa546a0014f6311261ebc3",
            [("$", "$", "USD"), ("I\u{20b9}", rupee, "INR")],
        ),
        (
            ORIGINALNAME_SHAPE_LIVE,
            "0c3ac1f8bfcc372a8e2213c31980b149faeadc566cf69502f3e1dd61750a241d",
            [("I\u{20b9}", rupee, "INR"), ("UUSD", "USD", "US Dollar")],
        ),
    ] {
        assert_eq!(sha256_hex(bytes), sha256, "captured wire bytes changed");
        let xml = decode_utf16le(bytes);

        let currency = parse_company_currency(&xml).unwrap();
        assert_eq!(currency.currency_count, 2);
        assert!(!currency.is_inr, "several masters stay undetermined");
        assert_eq!(currency.symbol, expected[0].0, "the first master read");

        let masters = parse_currency_masters(&xml).unwrap();
        let read = masters
            .iter()
            .map(|master| {
                (
                    master.name.as_str(),
                    master.original_name.as_deref().unwrap(),
                    master.mailing_name.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(read, expected);
        assert_eq!(identify_base_master(&masters, None), None);
        let base = identify_base_master(&masters, Some(rupee)).unwrap();
        assert_eq!(base.name, "I\u{20b9}");
        assert!(base.is_inr());

        let original = format!(">{rupee}</ORIGINALNAME>");
        assert_eq!(xml.matches(&original).count(), 1);
        let padded =
            parse_currency_masters(&xml.replace(&original, &format!("> {rupee}</ORIGINALNAME>")))
                .unwrap();
        assert!(padded
            .iter()
            .any(|master| master.original_name.as_deref() == Some(&format!(" {rupee}"))));
        assert_eq!(identify_base_master(&padded, Some(rupee)), None);
    }
}

/// bridge#551: `ORIGINALNAME` fails closed at the wire, injected into the
/// captured FOREX read. A second one on the same master, as text or as an
/// empty element, is refused rather than letting either pick the base; a text
/// that does not unescape is refused; a lone empty element reads as present
/// and empty, not absent.
#[test]
fn originalname_is_refused_when_duplicated_or_malformed_and_kept_when_empty() {
    let xml = decode_utf16le(ORIGINALNAME_FOREX_LIVE);
    let original = "<ORIGINALNAME TYPE=\"String\">\u{20b9}</ORIGINALNAME>";
    assert_eq!(xml.matches(original).count(), 1);
    for second in [original, "<ORIGINALNAME/>"] {
        assert_eq!(
            parse_currency_masters(&xml.replace(original, &format!("{original}{second}"))),
            Err(NativeOutstandingsError::InvalidResponse(
                "currency_duplicate_original_name"
            )),
            "{second}"
        );
    }
    assert_eq!(
        parse_currency_masters(&xml.replace(
            original,
            "<ORIGINALNAME TYPE=\"String\">&bogus;</ORIGINALNAME>"
        )),
        Err(NativeOutstandingsError::InvalidResponse(
            "native_xml_invalid_escape"
        ))
    );
    let empty = parse_currency_masters(&xml.replace(original, "<ORIGINALNAME/>")).unwrap();
    assert_eq!(empty[1].name, "I\u{20b9}");
    assert_eq!(empty[1].original_name.as_deref(), Some(""));
}

/// bridge#551: the production request does not fetch `ORIGINALNAME` until its
/// first consumer does (TALLY_PROTOCOL_REFERENCE §9.10a.2).
#[test]
fn the_currency_request_does_not_fetch_originalname_yet() {
    let request = render_company_currency_request("Synthetic Company");
    assert!(request.contains("<FETCH>NAME, MAILINGNAME, DECIMALPLACES</FETCH>"));
    assert!(!request.contains("ORIGINALNAME"));
}
