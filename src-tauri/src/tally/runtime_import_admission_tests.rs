//! Captured Company responses prove queued import admission without a Tally write.
use super::*;
use crate::tally::approved_import::{ApprovedImport, ApprovedImportAdmissionError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tally_protocol_simulator::{
    Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

fn captured_companies() -> String {
    let bytes = include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn company_plan(xml: String) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(xml))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}

fn status_plan() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
        .with_framing(ResponseFraming::ContentLength)
}

fn identity(xml: &str) -> VerifiedCompanyIdentity {
    let companies = parse_companies_from_collection(xml).unwrap();
    let company = companies
        .iter()
        .find(|company| company.guid.as_deref() == Some(GUID))
        .unwrap();
    VerifiedCompanyIdentity::from_observed_companies(
        company.name.clone(),
        GUID.into(),
        company.company_number.clone().unwrap(),
        company.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap()
}

fn expected_admission(
    observed: &[tally_protocol_simulator::ObservedRequest],
    responses: &[Vec<u8>],
) -> RuntimeReadEvidence {
    let join = |left: &str, right: &str| sha256_hex(format!("{left}:{right}").as_bytes());
    let empty_request = sha256_hex(&[]);
    RuntimeReadEvidence {
        request_sha256: join(
            &join(&empty_request, &observed[1].request_body_sha256),
            &observed[2].request_body_sha256,
        ),
        response_sha256: join(
            &join(&sha256_hex(&responses[0]), &sha256_hex(&responses[1])),
            &sha256_hex(&responses[2]),
        ),
        bytes: responses[0].len() + responses[1].len() + responses[2].len(),
    }
}

#[tokio::test]
async fn queued_education_date_refusal_retains_captured_mode_and_final_company_evidence() {
    let companies = captured_companies();
    let education = companies.replace(
        "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
        "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
    );
    assert_ne!(education, companies);
    let plans = vec![
        status_plan(),
        company_plan(education.clone()),
        company_plan(education),
    ];
    let responses = plans
        .iter()
        .map(ScenarioPlan::response_bytes)
        .collect::<Vec<_>>();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default();
    let dispatched = Arc::new(AtomicBool::new(false));
    let guard = dispatched.clone();
    let result = runtime
        .post_approved_import(
            TallyConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            &identity(&companies),
            ApprovedImport::approved_for_test(
                "<ENVELOPE/>".into(),
                bridge_tally_core::TallyDate::parse("20260915").unwrap(),
            ),
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await;
    let error = result.expect_err("Education date must refuse after queue admission");
    assert!(error.chain().any(|cause| matches!(
        cause.downcast_ref::<ApprovedImportAdmissionError>(),
        Some(ApprovedImportAdmissionError::EducationVoucherDateUnsupported)
    )));
    assert!(
        !dispatched.load(Ordering::Acquire),
        "refusal precedes durable intent"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(observed.len(), 3, "refusal must precede the import POST");
    assert_eq!(
        error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence,
        expected_admission(&observed, &responses),
        "captured mode and final-company admission survive the refusal"
    );
}

#[tokio::test]
async fn queued_company_refusal_retains_captured_mode_and_final_company_evidence() {
    let companies = captured_companies();
    let malformed = companies.replacen(
        &format!("<GUID TYPE=\"String\">{GUID}</GUID>"),
        "<GUID TYPE=\"String\"></GUID>",
        1,
    );
    assert!(malformed != companies);
    let plans = vec![
        status_plan(),
        company_plan(companies.clone()),
        company_plan(malformed),
    ];
    let responses = plans
        .iter()
        .map(ScenarioPlan::response_bytes)
        .collect::<Vec<_>>();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default();
    let dispatched = Arc::new(AtomicBool::new(false));
    let guard = dispatched.clone();
    let result = runtime
        .post_approved_import(
            TallyConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            &identity(&companies),
            ApprovedImport::approved_for_test(
                "<ENVELOPE/>".into(),
                bridge_tally_core::TallyDate::parse("20260901").unwrap(),
            ),
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await;
    let error = result.expect_err("malformed final Company tuple must refuse");
    assert!(
        !dispatched.load(Ordering::Acquire),
        "refusal precedes durable intent"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(observed.len(), 3, "refusal must precede the import POST");
    assert_eq!(
        error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence,
        expected_admission(&observed, &responses),
        "captured mode and malformed final-company observation survive the refusal"
    );
}

#[tokio::test]
async fn queued_import_keeps_final_company_admission_separate_from_raw_import_wire() {
    let companies = captured_companies();
    // There is no captured Journal-import response fixture. This runtime only
    // records raw bytes here; it does not parse import semantics, so replay a
    // captured Company body instead of inventing a success-shaped response.
    let captured_non_import_response = companies.clone();
    let plans = vec![
        status_plan(),
        company_plan(companies.clone()),
        company_plan(companies.clone()),
        company_plan(captured_non_import_response),
    ];
    let responses = plans
        .iter()
        .map(ScenarioPlan::response_bytes)
        .collect::<Vec<_>>();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default();
    let dispatched = Arc::new(AtomicBool::new(false));
    let guard = dispatched.clone();
    let dispatch = runtime
        .post_approved_import(
            TallyConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            &identity(&companies),
            ApprovedImport::approved_for_test(
                "<ENVELOPE/>".into(),
                bridge_tally_core::TallyDate::parse("20260901").unwrap(),
            ),
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await
        .expect("captured admission permits a valid date");
    assert!(dispatched.load(Ordering::Acquire));
    let observed = simulator.finish().unwrap();
    assert_eq!(observed.len(), 4);
    assert_eq!(
        dispatch.admission_evidence,
        expected_admission(&observed, &responses)
    );
    assert_eq!(
        dispatch.response_evidence.request_sha256,
        observed[3].request_body_sha256
    );
    assert_eq!(
        dispatch.response_evidence.response_sha256,
        sha256_hex(&responses[3])
    );
    assert_eq!(dispatch.response_evidence.bytes, responses[3].len());
    assert_ne!(
        dispatch.response_evidence.request_sha256, dispatch.admission_evidence.request_sha256,
        "raw import request commitment must not be replaced by the admission composite"
    );
}
