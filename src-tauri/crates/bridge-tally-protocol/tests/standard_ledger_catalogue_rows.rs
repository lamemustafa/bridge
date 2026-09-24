//! Both parsers of Tally's `List of Ledgers` hold a whole large book
//! (bridge#634). They read the same response, which carries every ledger, and
//! a 1,000-row bound refused any book past it: the catalogue for the `vouchers`
//! ledger filter and import validation, and the identity observation for direct
//! company bootstrap.

use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, parse_standard_ledger_catalog_with_identities,
    parse_standard_ledger_identity_observation, ExpectedTallyTextEncoding,
};

const COMPANY_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const LEDGER_CATALOGUE: &[u8] =
    include_bytes!("fixtures/agent/native-ledger-catalogue.utf16le.xml");

#[test]
fn a_list_of_ledgers_past_a_thousand_rows_parses_whole() {
    let captured = decode_tally_xml_response_bytes_limited(
        LEDGER_CATALOGUE,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        LEDGER_CATALOGUE.len(),
    )
    .expect("captured BOM-less UTF-16LE response decodes")
    .text;
    // Each added row is the capture's first row under its own name and GUID.
    let start = captured.find("<LEDGER NAME=").unwrap();
    let end = start + captured[start..].find("</LEDGER>").unwrap() + "</LEDGER>".len();
    let template = &captured[start..end];
    assert!(template.contains("Bridge Nested Debtor WR4") && template.contains("-000000d5<"));
    let rows = (0..1_000)
        .map(|index| {
            template
                .replace(
                    "Bridge Nested Debtor WR4",
                    &format!("Bulk Ledger {index:04}"),
                )
                .replace("-000000d5<", &format!("-b{index:07x}<"))
        })
        .collect::<String>();
    let close = captured.rfind("</COLLECTION>").unwrap();
    let xml = format!("{}{rows}{}", &captured[..close], &captured[close..]);
    let captured_rows = captured.matches("<LEDGER NAME=").count();
    assert_eq!(xml.matches("<LEDGER NAME=").count(), captured_rows + 1_000);

    let catalogue =
        parse_standard_ledger_catalog_with_identities(&xml, "WR2 Unicode Lab", COMPANY_GUID)
            .expect("a book past a thousand ledgers is one catalogue");
    assert_eq!(catalogue.names().count(), captured_rows + 1_000);
    let observed = parse_standard_ledger_identity_observation(&xml, "WR2 Unicode Lab")
        .expect("a book past a thousand ledgers confirms its company");
    assert_eq!(observed.ledger_count, (captured_rows + 1_000) as u64);
    assert!(observed.company_guid.eq_ignore_ascii_case(COMPANY_GUID));
}
