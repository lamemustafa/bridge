use super::*;
use crate::tally::runtime::{
    with_read_evidence, NativeLedgerIdentityAdmissionError, RuntimeReadEvidence, RuntimeReadFailure,
};
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

fn assert_response_validation(error: anyhow::Error) {
    let mapped = tally_runtime_command_error(error);
    assert_eq!(mapped.code, "response_validation_failed");
    assert_eq!(mapped.category, "Response validation");
    assert_eq!(mapped.retry, "after_change");
    assert!(!mapped.tally_state_may_have_changed);
    let serialized = serde_json::to_string(&mapped).unwrap();
    assert!(!serialized.contains("private-marker"));
    assert!(!serialized.contains("native_ledger_"));
}

#[test]
fn native_identity_admission_is_typed_through_command_error_wrappers() {
    assert_response_validation(NativeLedgerIdentityAdmissionError::Duplicate.into());
    assert_response_validation(NativeLedgerIdentityAdmissionError::InvalidMasterId.into());
    for kind in [
        NativeLedgerIdentityAdmissionError::Duplicate,
        NativeLedgerIdentityAdmissionError::InvalidMasterId,
    ] {
        let wrapped = with_read_evidence(anyhow::Error::new(kind), RuntimeReadEvidence::empty())
            .context("cancel endpoint invalid private-marker");
        assert_response_validation(wrapped);
    }
}

#[tokio::test]
async fn captured_native_ledger_refusals_reach_command_validation_classification() {
    let decode = |bytes: &[u8]| {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let companies_xml = decode(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"));
    let extent_xml = include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    )
    .to_string();
    let ledger_xml = decode(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-period-opening.utf16le.xml"
    ));
    let companies = bridge_tally_protocol::parse_companies_from_collection(&companies_xml).unwrap();
    let selected = companies
        .iter()
        .find(|company| company.guid.as_deref() == Some("61c6de69-1748-461c-ad3f-162cb949df9f"))
        .unwrap();
    let identity = VerifiedCompanyIdentity::from_observed_companies(
        selected.name.clone(),
        selected.guid.clone().unwrap(),
        selected.company_number.clone().unwrap(),
        selected.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap();
    let row_start = ledger_xml.find("<LEDGER NAME=").unwrap();
    let row_end =
        row_start + ledger_xml[row_start..].find("</LEDGER>").unwrap() + "</LEDGER>".len();
    let id_start = ledger_xml.find("<MASTERID").unwrap();
    let value_start = id_start + ledger_xml[id_start..].find('>').unwrap() + 1;
    let value_end = value_start + ledger_xml[value_start..].find("</MASTERID>").unwrap();
    // Negative mutations of captured rows, not newly authored positive fixtures.
    let duplicate = format!(
        "{}{}{}",
        &ledger_xml[..row_end],
        &ledger_xml[row_start..row_end],
        &ledger_xml[row_end..]
    );
    let malformed = format!(
        "{}invalid{}",
        &ledger_xml[..value_start],
        &ledger_xml[value_end..]
    );
    for (fault, expected) in [
        (duplicate, NativeLedgerIdentityAdmissionError::Duplicate),
        (
            malformed,
            NativeLedgerIdentityAdmissionError::InvalidMasterId,
        ),
    ] {
        let xml = |body: String| {
            ScenarioPlan::new(Fixture::SyntheticXml(body)).with_encoding(WireEncoding::Utf16Le)
        };
        let company = xml(companies_xml.clone());
        let extent = xml(extent_xml.clone());
        let ledger = xml(fault);
        let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
        let plans = vec![
            status.clone(),
            company.clone(),
            company,
            extent.clone(),
            status.clone(),
            extent,
            status.clone(),
            ledger.clone(),
            status.clone(),
            ledger,
            status,
        ];
        let expected_bytes: usize = [0, 1, 7, 9]
            .into_iter()
            .map(|i| encode(&plans[i].fixture.body(), plans[i].encoding).len())
            .sum();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        // This is the runtime operation used by fetch_tally_ledgers, followed
        // by that command's actual error classifier.
        let error = TallyRuntime::default()
            .fetch_ledgers(
                TallyConfig {
                    host: simulator.address().ip().to_string(),
                    port: simulator.address().port(),
                },
                &identity,
            )
            .await
            .unwrap_err();
        assert_eq!(simulator.finish().unwrap().len(), 11);
        assert_eq!(
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<NativeLedgerIdentityAdmissionError>()),
            Some(&expected)
        );
        let retained = error.downcast_ref::<RuntimeReadFailure>().unwrap();
        assert_eq!(retained.evidence.bytes, expected_bytes);
        assert_response_validation(
            if expected == NativeLedgerIdentityAdmissionError::Duplicate {
                error
            } else {
                error.context("cancel endpoint invalid private-marker")
            },
        );
    }
}
