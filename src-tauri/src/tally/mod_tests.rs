use super::*;

const CAPTURED_EXTENT_V2: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
);
const CAPTURED_NAME: &str = "BRIDGE PROBE B SANDBOX";
const CAPTURED_GUID: &str = "ec4454ae-5c4c-4bfa-b3b0-68182a749689";
const CAPTURED_NUMBER: &str = "100005";
const CAPTURED_BOOKS_FROM: &str = "20250401";

fn captured_identity(xml: &str, guid: &str) -> VerifiedCompanyIdentity {
    let companies = bridge_tally_protocol::parse_companies_from_collection(xml)
        .expect("captured CompanyBookExtentV2 response parses as a Company collection");
    VerifiedCompanyIdentity::from_observed_companies(
        CAPTURED_NAME.to_string(),
        guid.to_string(),
        CAPTURED_NUMBER.to_string(),
        CAPTURED_BOOKS_FROM.to_string(),
        &companies,
    )
    .expect("captured full tuple remains uniquely observed")
}

#[test]
fn verified_identity_canonicalizes_captured_guid_case_for_closing_comparison() {
    let target_start = CAPTURED_EXTENT_V2
        .find(&format!(r#"<COMPANY NAME="{CAPTURED_NAME}""#))
        .expect("captured target company row exists");
    let target_end = target_start
        + CAPTURED_EXTENT_V2[target_start..]
            .find("</COMPANY>")
            .expect("captured target company row closes")
        + "</COMPANY>".len();
    let target = &CAPTURED_EXTENT_V2[target_start..target_end];
    let upper_guid = CAPTURED_GUID.to_ascii_uppercase();
    let changed_target = target.replacen(CAPTURED_GUID, &upper_guid, 1);
    assert_ne!(
        changed_target, target,
        "captured GUID case mutation must apply"
    );
    let closing = CAPTURED_EXTENT_V2.replacen(target, &changed_target, 1);
    assert_ne!(
        closing, CAPTURED_EXTENT_V2,
        "captured response mutation must apply"
    );

    assert_eq!(
        captured_identity(CAPTURED_EXTENT_V2, CAPTURED_GUID),
        captured_identity(&closing, &upper_guid),
        "GUID casing alone must not make a verified closing identity drift"
    );
}
