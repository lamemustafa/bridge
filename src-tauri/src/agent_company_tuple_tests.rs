//! Admission rules reuse the desktop boundary and reject altered captured scalars.
use super::*;
use crate::tally::VerifiedCompanyIdentityError;
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

#[test]
fn company_number_admission_uses_the_existing_ascii_digit_contract() {
    use crate::tally::validators::is_valid_company_number;
    for value in ["0", "1", "000001", "100004", "1234567890123456"] {
        assert!(is_valid_company_number(value), "{value}");
    }
    for value in [
        "",
        " ",
        "abc",
        "12345678901234567",
        "+1",
        "1.0",
        "१२३",
        "1\n",
    ] {
        assert!(!is_valid_company_number(value), "{value:?}");
    }
}

#[tokio::test]
async fn malformed_observed_company_numbers_never_authorize_scoped_reads() {
    let bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml");
    let captured = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    for number in ["abc", "12345678901234567", "+1", "1.0", "१२३"] {
        let xml = captured.replace(
            "<COMPANYNUMBER TYPE=\"Number\"> 100004</COMPANYNUMBER>",
            &format!("<COMPANYNUMBER TYPE=\"Number\">{number}</COMPANYNUMBER>"),
        );
        let companies = bridge_tally_protocol::parse_companies_from_collection(&xml).unwrap();
        let selected = companies
            .iter()
            .find(|company| company.guid.as_deref() == Some(GUID))
            .unwrap();
        assert_eq!(
            company_json(selected, &companies)["identity_state"],
            "invalid_company_number"
        );
        assert_eq!(
            VerifiedCompanyIdentity::from_observed_companies(
                selected.name.clone(),
                GUID.into(),
                selected.company_number.clone().unwrap(),
                selected.books_from.clone().unwrap(),
                &companies,
            )
            .err(),
            Some(VerifiedCompanyIdentityError::InvalidCompanyNumber)
        );
        for (tool, args) in [
            (
                "vouchers",
                json!({"company_guid":GUID,"from":"20260801","to":"20260801"}),
            ),
            (
                "validate_masters",
                json!({"company_guid":GUID,"ledgers":["Cash"]}),
            ),
        ] {
            let plan = ScenarioPlan::new(Fixture::SyntheticXml(xml.clone()))
                .with_encoding(WireEncoding::Utf16Le);
            let response_bytes = encode(&plan.fixture.body(), plan.encoding);
            let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
            let simulator =
                SequenceSimulator::spawn(vec![plan.clone(), status.clone(), plan, status]).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let response = identity_tests::server(simulator.address(), directory.path())
                .call_tool(tool, args)
                .await;
            assert_eq!(response["isError"], true);
            let content = &response["structuredContent"];
            assert_eq!(content["result"]["error"]["code"], "company_number_invalid");
            let observed = simulator.finish().unwrap();
            assert_eq!(
                observed.len(),
                4,
                "only the company admission read is allowed"
            );
            assert_eq!(content["evidence"]["state"], "partial");
            assert_eq!(
                content["evidence"]["request_sha256"],
                observed[0].request_body_sha256
            );
            assert_eq!(
                content["evidence"]["response_sha256"],
                sha256_hex(&response_bytes)
            );
            assert_eq!(content["evidence"]["bytes"], 2 * response_bytes.len());
        }
    }
}
