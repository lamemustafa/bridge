use bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile;
use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, encode_tally_xml_request_utf16le,
    parse_companies_from_collection, parse_company_gateway_capability_observation,
    ExpectedTallyTextEncoding,
};
use sha2::{Digest, Sha256};

const CAPTURE: &[u8] =
    include_bytes!("fixtures/agent/native-licensed-release-companies.utf16le.xml");
const RELEASE: &str = r#"<BRIDGERELEASE TYPE="String">7.1</BRIDGERELEASE>"#;

fn captured_xml() -> String {
    decode_tally_xml_response_bytes_limited(
        CAPTURE,
        "text/xml; charset=utf-16",
        ExpectedTallyTextEncoding::Utf16Le,
        CAPTURE.len(),
    )
    .unwrap()
    .text
}

#[test]
fn captured_company_release_matches_exact_request_and_response_commitments() {
    let metadata: serde_json::Value = serde_json::from_str(include_str!(
        "fixtures/agent/native-licensed-release-companies.json"
    ))
    .unwrap();
    assert_eq!(
        CAPTURE.len(),
        metadata["fixture_bytes"].as_u64().unwrap() as usize
    );
    assert_eq!(
        Sha256::digest(CAPTURE)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        metadata["fixture_sha256"]
    );
    let request = encode_tally_xml_request_utf16le(&ReadOnlyProfile::CompanyListV2.render());
    assert_eq!(
        request.len(),
        metadata["source_request_bytes"].as_u64().unwrap() as usize
    );
    assert_eq!(
        Sha256::digest(&request)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        metadata["source_request_sha256"]
    );
    let xml = captured_xml();
    assert_eq!(parse_companies_from_collection(&xml).unwrap().len(), 16);
    let observed = parse_company_gateway_capability_observation(&xml).unwrap();
    assert_eq!(observed.product, "TallyPrime");
    assert_eq!(observed.release.as_deref(), Some("7.1"));
    assert!(!observed.educational_mode);
    assert!(observed.silver && !observed.gold);
}

#[test]
fn release_requires_all_rows_to_agree_without_erasing_historical_mode() {
    let xml = captured_xml();
    assert!(xml.contains(RELEASE));
    for changed in [
        xml.replace(RELEASE, ""),
        xml.replace(RELEASE, "<BRIDGERELEASE/>"),
        xml.replace(RELEASE, "<BRIDGERELEASE>  </BRIDGERELEASE>"),
        xml.replacen(RELEASE, "", 1),
        xml.replacen(RELEASE, "<BRIDGERELEASE>7.2</BRIDGERELEASE>", 1),
    ] {
        let observed = parse_company_gateway_capability_observation(&changed).unwrap();
        assert_eq!(observed.release, None);
        assert_eq!(observed.product, "TallyPrime");
        assert!(!observed.educational_mode && observed.silver && !observed.gold);
    }
    let padded = xml.replace(RELEASE, "<BRIDGERELEASE> 7.1 </BRIDGERELEASE>");
    assert_eq!(
        parse_company_gateway_capability_observation(&padded)
            .unwrap()
            .release
            .as_deref(),
        Some("7.1")
    );
}

#[test]
fn malformed_release_claims_fail_closed_at_the_collection_boundary() {
    let xml = captured_xml();
    for replacement in [
        format!("{RELEASE}{RELEASE}"),
        format!("<BRIDGERELEASE/>{RELEASE}"),
        format!("<BRIDGERELEASE>{}</BRIDGERELEASE>", "x".repeat(256)),
        "<BRIDGERELEASE>7.1&#10;untrusted</BRIDGERELEASE>".to_string(),
    ] {
        assert!(parse_company_gateway_capability_observation(&xml.replacen(
            RELEASE,
            &replacement,
            1
        ))
        .is_err());
    }
}
