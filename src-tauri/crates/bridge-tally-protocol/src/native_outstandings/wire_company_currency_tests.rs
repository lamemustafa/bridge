use super::currency_tests::decode_utf16le;
use super::*;
use crate::native_outstandings::{
    parse_currency_master_list, parse_native_ledger_snapshot_classified,
    render_company_base_currency_request, render_company_currency_request,
    render_company_currency_request_with_originalname,
};

const COMPANY_CURRENCY_EDITED: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/company_currencyname_forex_edited.utf16le.xml"
));
const CURRENCY_ORIGINALNAME_FOREX: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/currency_originalname_forex_live.utf16le.xml"
));
const LEDGERS_CURRENCY_FOREX: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ledgers_currency_forex_live.utf16le.xml"
));
const FOREX_GUID: &str = "b14e9b2d-8a63-4779-804d-25d59eb787eb";
const SHAPE_GUID: &str = "3a6bd6e1-b835-4bff-89dd-8a6af138c346";
const FOREX_ROW_GUID: &str = "<GUID TYPE=\"String\">b14e9b2d-8a63-4779-804d-25d59eb787eb</GUID>";
const FOREX_ROW_CURRENCY: &str = "<CURRENCYNAME TYPE=\"String\">\u{20b9}</CURRENCYNAME>";

fn edited() -> String {
    assert_eq!(
        sha256_hex(COMPANY_CURRENCY_EDITED),
        "f44ff5795891ec4ba180da8d65a16e574385e6cc69ddd75c51b170144f53ec05",
        "edited capture changed"
    );
    decode_utf16le(COMPANY_CURRENCY_EDITED)
}

/// The FOREX row of the edited capture, from its opening tag through its
/// closing tag.
fn forex_row(xml: &str) -> &str {
    let start = xml.find("<COMPANY NAME=\"BRIDGE CORPUS FOREX\"").unwrap();
    let end = start + xml[start..].find("</COMPANY>").unwrap() + "</COMPANY>".len();
    &xml[start..end]
}

/// bridge#551: the requests. The currency request every monetary read sends
/// is unchanged; only the classified outstandings read sends the one with
/// `ORIGINALNAME`, and the Company collection is a plain export
/// (TALLY_PROTOCOL_REFERENCE §9.10a.2).
#[test]
fn only_the_classified_read_fetches_originalname() {
    let plain = render_company_currency_request("Synthetic Company");
    assert!(plain.contains("<FETCH>NAME, MAILINGNAME, DECIMALPLACES</FETCH>"));
    assert!(!plain.contains("ORIGINALNAME"));
    assert_eq!(
        render_company_currency_request_with_originalname("Synthetic Company"),
        plain.replace(
            "DECIMALPLACES</FETCH>",
            "DECIMALPLACES, ORIGINALNAME</FETCH>"
        )
    );
    let company = render_company_base_currency_request("Synthetic & Co");
    assert!(company.contains("<TYPE>Company</TYPE><FETCH>NAME, GUID, CURRENCYNAME</FETCH>"));
    assert!(company.contains("<SVCURRENTCOMPANY>Synthetic &amp; Co</SVCURRENTCOMPANY>"));
    assert!(!company.contains("FILTER") && !company.contains("FORMULA"));
}

/// bridge#551: the company's `CURRENCYNAME` is picked by GUID from a
/// collection that lists other companies, and `CMPINFO`'s
/// `<COMPANY>0</COMPANY>` counter is not a row.
#[test]
fn the_company_currency_name_is_picked_by_guid() {
    let xml = edited();
    assert_eq!(xml.matches("<COMPANY>0</COMPANY>").count(), 1);
    assert_eq!(xml.matches("<COMPANY NAME=").count(), 2);
    for guid in [FOREX_GUID, &FOREX_GUID.to_uppercase(), SHAPE_GUID] {
        assert_eq!(
            parse_company_currency_name(&xml, guid).as_deref(),
            Ok("\u{20b9}"),
            "{guid}"
        );
    }
    assert_eq!(
        parse_company_currency_name(&xml, "00000000-0000-0000-0000-000000000000"),
        Err(NativeOutstandingsError::InvalidResponse(
            "company_currency_row_missing"
        ))
    );
}

/// bridge#551: labelled edits of the edited capture. The value is kept
/// untrimmed; a blank, absent or duplicated value, a GUID answered twice, and a
/// failed `STATUS` are refused.
#[test]
fn the_company_currency_name_fails_closed() {
    let xml = edited();
    let row = forex_row(&xml);
    assert_eq!(row.matches(FOREX_ROW_CURRENCY).count(), 1);
    assert_eq!(row.matches(FOREX_ROW_GUID).count(), 1);
    let with_row = |new_row: &str| xml.replacen(row, new_row, 1);
    let with_currency = |currency: &str| with_row(&row.replace(FOREX_ROW_CURRENCY, currency));

    let padded = with_currency("<CURRENCYNAME TYPE=\"String\"> \u{20b9}</CURRENCYNAME>");
    assert_eq!(
        parse_company_currency_name(&padded, FOREX_GUID).as_deref(),
        Ok(" \u{20b9}")
    );
    for (edit, code) in [
        (
            with_currency("<CURRENCYNAME TYPE=\"String\"></CURRENCYNAME>"),
            "company_currency_name_missing",
        ),
        (
            with_currency("<CURRENCYNAME TYPE=\"String\"> </CURRENCYNAME>"),
            "company_currency_name_missing",
        ),
        (
            with_currency("<CURRENCYNAME/>"),
            "company_currency_name_missing",
        ),
        (with_currency(""), "company_currency_name_missing"),
        (
            with_currency(&format!("{FOREX_ROW_CURRENCY}{FOREX_ROW_CURRENCY}")),
            "company_currency_duplicate_name",
        ),
        (
            with_currency(&format!("{FOREX_ROW_CURRENCY}<CURRENCYNAME/>")),
            "company_currency_duplicate_name",
        ),
        (
            with_row(&format!("{row}\r\n    {row}")),
            "company_currency_row_ambiguous",
        ),
        (
            with_row(&row.replace(FOREX_ROW_GUID, "")),
            "company_currency_guid_missing",
        ),
        (
            with_row(&row.replace(FOREX_ROW_GUID, &format!("{FOREX_ROW_GUID}{FOREX_ROW_GUID}"))),
            "company_currency_duplicate_guid",
        ),
        (
            with_row(&row.replacen(
                "RESERVEDNAME=\"\"",
                "RESERVEDNAME=\"\" RESERVEDNAME=\"\"",
                1,
            )),
            "company_currency_row_malformed_attributes",
        ),
    ] {
        assert_ne!(edit, xml);
        assert_eq!(
            parse_company_currency_name(&edit, FOREX_GUID),
            Err(NativeOutstandingsError::InvalidResponse(code)),
            "{code}"
        );
    }
    assert_eq!(
        parse_company_currency_name(
            &xml.replacen("<STATUS>1</STATUS>", "<STATUS>0</STATUS>", 1),
            FOREX_GUID
        ),
        Err(NativeOutstandingsError::TallyReportedFailure)
    );
}

/// bridge#551, from captures only: FOREX's currency read with `ORIGINALNAME`
/// and its company's `CURRENCYNAME` identify the `I₹` master as the base, INR
/// by its mailing name; against it the captured ledger snapshot sets the
/// three `$` ledgers aside. Without the company's name nothing is identified.
#[test]
fn forex_identifies_its_rupee_base_and_sets_its_dollar_ledgers_aside() {
    let currency = decode_utf16le(CURRENCY_ORIGINALNAME_FOREX);
    let masters = parse_currency_master_list(&currency).unwrap();
    assert_eq!(masters.count(), 2);
    assert!(!parse_company_currency(&currency).unwrap().is_inr);
    assert_eq!(masters.identify_base(None), None);

    let name = parse_company_currency_name(&edited(), FOREX_GUID).unwrap();
    let base = masters.identify_base(Some(&name)).unwrap();
    assert_eq!(base.base().name(), "I\u{20b9}");
    assert!(base.is_inr());
    assert_eq!(base.decimal_places(), 2);

    let snapshot = parse_native_ledger_snapshot_classified(
        &decode_utf16le(LEDGERS_CURRENCY_FOREX),
        base.base(),
    )
    .unwrap();
    let foreign = snapshot
        .foreign
        .iter()
        .map(|ledger| (ledger.ledger.as_str(), ledger.currency.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        foreign,
        [
            ("BRIDGE FX DEBTOR A", "$"),
            ("FX USD Debtor 01", "$"),
            ("FX USD Debtor 02", "$")
        ]
    );
    assert_eq!(snapshot.base.len(), 7);
}
