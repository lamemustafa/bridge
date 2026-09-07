//! Captured source replies prove queued import admission without a Tally write.
use super::*;
use crate::tally::{
    agent_read_request::AgentReadRequest,
    approved_import::{ApprovedImport, ApprovedImportAdmissionError},
};
use bridge_tally_protocol::{
    parse_standard_ledger_catalog_with_identities,
    xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName},
};
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

fn captured_catalogue() -> String {
    let bytes = include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
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

fn captured_journal() -> String {
    let bytes = include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-namespaced-journal.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
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

fn approved_import(companies: &str, date: &str) -> ApprovedImport {
    let identity = identity(companies);
    let catalogue = parse_standard_ledger_catalog_with_identities(
        &captured_catalogue(),
        identity.display_name(),
        identity.company_guid(),
    )
    .expect("captured catalog is admitted for the captured company");
    let selected = catalogue
        .names()
        .next()
        .expect("captured catalog has a ledger")
        .to_string();
    let binding = catalogue
        .bind_selected([selected])
        .expect("selected ledger is bound to its observed GUID");
    let company = ValidatedCompanyName::new(identity.display_name().to_string()).unwrap();
    let ledger_catalogue_request = AgentReadRequest::parse(
        ReadOnlyProfile::StandardLedgerCatalogV1 { company: &company }.render(),
    )
    .expect("static catalog read is admitted");
    ApprovedImport::approved_for_test(
        "<ENVELOPE/>".into(),
        bridge_tally_core::TallyDate::parse(date).unwrap(),
        ledger_catalogue_request,
        binding,
    )
}

/// Each mode probe is status/company, followed by explicit company admission.
/// Each identity-bracketed source read contains two reports and status checks.
fn queued_plans(
    opening_companies: String,
    verification: String,
    catalogue: String,
    final_companies: String,
    post_response: Option<String>,
) -> Vec<ScenarioPlan> {
    let paired = |body: String, companies: &String| {
        vec![
            company_plan(companies.clone()),
            company_plan(body.clone()),
            status_plan(),
            company_plan(body),
            status_plan(),
            company_plan(companies.clone()),
        ]
    };
    // Product/mode observation itself reads the Company collection, followed by
    // the explicit opening company-scope admission.
    let mut plans = vec![
        status_plan(),
        company_plan(opening_companies.clone()),
        company_plan(opening_companies.clone()),
    ];
    plans.extend(paired(verification.clone(), &opening_companies));
    plans.extend(paired(verification, &opening_companies));
    plans.extend(paired(catalogue, &opening_companies));
    plans.extend([
        status_plan(),
        company_plan(final_companies.clone()),
        company_plan(final_companies),
    ]);
    if let Some(response) = post_response {
        plans.push(company_plan(response));
    }
    plans
}

fn paired_observation(
    observed: &[tally_protocol_simulator::ObservedRequest],
    responses: &[Vec<u8>],
    index: usize,
) -> RuntimeReadEvidence {
    RuntimeReadEvidence {
        request_sha256: observed[index].request_body_sha256.clone(),
        response_sha256: sha256_hex(&responses[index]),
        bytes: responses[index].len().saturating_mul(2),
    }
}

fn single_observation(
    observed: &[tally_protocol_simulator::ObservedRequest],
    responses: &[Vec<u8>],
    index: usize,
) -> RuntimeReadEvidence {
    RuntimeReadEvidence {
        request_sha256: observed[index].request_body_sha256.clone(),
        response_sha256: sha256_hex(&responses[index]),
        bytes: responses[index].len(),
    }
}

fn expected_queued_evidence(
    observed: &[tally_protocol_simulator::ObservedRequest],
    responses: &[Vec<u8>],
    final_admission: bool,
) -> RuntimeReadEvidence {
    let mut evidence = single_observation(observed, responses, 0)
        .combine(single_observation(observed, responses, 1))
        .combine(single_observation(observed, responses, 2));
    for index in [4, 10, 16] {
        evidence = evidence.combine(paired_observation(observed, responses, index));
    }
    if final_admission {
        let closing = single_observation(observed, responses, 21)
            .combine(single_observation(observed, responses, 22));
        evidence = evidence
            .combine(closing)
            .combine(single_observation(observed, responses, 23));
    }
    evidence
}

#[tokio::test]
async fn queued_education_change_refuses_after_captured_absence_and_catalogue_reads() {
    let companies = captured_companies();
    let education = companies.replace(
        "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
        "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
    );
    assert_ne!(
        education, companies,
        "fixture mutation is explicit, not live evidence"
    );
    let plans = queued_plans(
        companies.clone(),
        companies.clone(),
        captured_catalogue(),
        education,
        None,
    );
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
            approved_import(&companies, "20260915"),
            |_, _, _, _| Ok(()),
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await;
    let error = result.expect_err("final Education boundary must refuse an unsupported date");
    assert!(error.chain().any(|cause| matches!(
        cause.downcast_ref::<ApprovedImportAdmissionError>(),
        Some(ApprovedImportAdmissionError::EducationVoucherDateUnsupported)
    )));
    assert!(
        !dispatched.load(Ordering::Acquire),
        "refusal precedes durable intent"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(observed.len(), 24, "final refusal precedes import POST");
    assert_eq!(
        error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence,
        expected_queued_evidence(&observed, &responses, true),
        "initial Licensed and final Education observations plus all queued reads survive refusal"
    );
}

#[tokio::test]
async fn queued_company_refusal_retains_captured_source_and_final_identity_evidence() {
    let companies = captured_companies();
    let replaced = companies.replacen(
        &format!("<GUID TYPE=\"String\">{GUID}</GUID>"),
        "<GUID TYPE=\"String\">71c6de69-1748-461c-ad3f-162cb949df9f</GUID>",
        1,
    );
    assert_ne!(replaced, companies);
    let plans = queued_plans(
        companies.clone(),
        companies.clone(),
        captured_catalogue(),
        replaced,
        None,
    );
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
            approved_import(&companies, "20260901"),
            |_, _, _, _| Ok(()),
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await;
    let error = result.expect_err("replaced final company identity must refuse");
    assert!(error.chain().any(|cause| matches!(
        cause.downcast_ref::<CompanyIdentityBracketError>(),
        Some(CompanyIdentityBracketError::AbsentOrAmbiguous)
    )));
    assert!(
        !dispatched.load(Ordering::Acquire),
        "refusal precedes durable intent"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(
        observed.len(),
        24,
        "final identity refusal precedes import POST"
    );
    assert_eq!(
        error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence,
        expected_queued_evidence(&observed, &responses, true),
    );
}

#[tokio::test]
async fn queued_catalogue_rename_refuses_before_intent_or_post() {
    let companies = captured_companies();
    let original_catalogue = captured_catalogue();
    let initial = parse_standard_ledger_catalog_with_identities(
        &original_catalogue,
        identity(&companies).display_name(),
        GUID,
    )
    .expect("captured catalog parses");
    let selected = initial.names().next().unwrap().to_string();
    let renamed = original_catalogue.replacen(
        &format!(r#"NAME="{selected}""#),
        r#"NAME="Bridge queued rename regression""#,
        1,
    );
    assert_ne!(
        renamed, original_catalogue,
        "captured metadata mutation is explicit"
    );
    let mut plans = queued_plans(
        companies.clone(),
        companies.clone(),
        renamed,
        companies.clone(),
        None,
    );
    plans.truncate(21); // This refusal occurs before final admission.
    let responses = plans
        .iter()
        .map(ScenarioPlan::response_bytes)
        .collect::<Vec<_>>();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default();
    let dispatched = Arc::new(AtomicBool::new(false));
    let guard = dispatched.clone();
    let error = runtime
        .post_approved_import(
            TallyConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            &identity(&companies),
            approved_import(&companies, "20260901"),
            move |_, _, current, binding| {
                assert!(!binding
                    .matches(current, identity(&companies).display_name(), GUID)
                    .expect("renamed captured catalog remains structurally valid"));
                Err(ApprovedImportAdmissionError::LedgerIdentityChanged.into())
            },
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await
        .expect_err("queued ledger rename must refuse");
    assert!(error.chain().any(|cause| matches!(
        cause.downcast_ref::<ApprovedImportAdmissionError>(),
        Some(ApprovedImportAdmissionError::LedgerIdentityChanged)
    )));
    assert!(
        !dispatched.load(Ordering::Acquire),
        "changed master binding precedes intent"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(
        observed.len(),
        21,
        "catalogue refusal precedes final admission and POST"
    );
    assert_eq!(
        error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence,
        expected_queued_evidence(&observed, &responses, false),
        "captured queued absence and catalogue evidence survive the master-binding refusal"
    );
}

#[tokio::test]
async fn queued_import_keeps_admission_separate_from_raw_import_wire() {
    let companies = captured_companies();
    // There is no captured Journal-import response fixture. This runtime records
    // raw bytes only, so replay a captured Company body rather than inventing success.
    let plans = queued_plans(
        companies.clone(),
        companies.clone(),
        captured_catalogue(),
        companies.clone(),
        Some(companies.clone()),
    );
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
            approved_import(&companies, "20260901"),
            |_, _, _, _| Ok(()),
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await
        .expect("captured admission permits a valid date");
    assert!(dispatched.load(Ordering::Acquire));
    let observed = simulator.finish().unwrap();
    assert_eq!(observed.len(), 25);
    assert_eq!(
        dispatch.admission_evidence,
        expected_queued_evidence(&observed, &responses, true)
    );
    assert_eq!(
        dispatch.response_evidence.request_sha256,
        observed[24].request_body_sha256
    );
    assert_eq!(
        dispatch.response_evidence.response_sha256,
        sha256_hex(&responses[24])
    );
    assert_eq!(dispatch.response_evidence.bytes, responses[24].len());
    assert_ne!(
        dispatch.response_evidence.request_sha256,
        dispatch.admission_evidence.request_sha256
    );
}

#[tokio::test]
async fn queued_import_refuses_newly_attributed_source_before_intent_or_post() {
    let companies = captured_companies();
    let journal = captured_journal();
    let mut plans = queued_plans(
        companies.clone(),
        journal.clone(),
        captured_catalogue(),
        companies.clone(),
        None,
    );
    plans.truncate(21); // This refusal occurs before final admission.
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
            approved_import(&companies, "20260901"),
            move |first, second, _, _| {
                assert_eq!(first, journal.as_str());
                assert_eq!(second, journal.as_str());
                Err(ApprovedImportAdmissionError::PreexistingIdentity.into())
            },
            move || {
                guard.store(true, Ordering::Release);
                Ok(())
            },
        )
        .await;
    let error = result.expect_err("queue recheck must refuse the attributed Journal");
    assert!(error.chain().any(|cause| matches!(
        cause.downcast_ref::<ApprovedImportAdmissionError>(),
        Some(ApprovedImportAdmissionError::PreexistingIdentity)
    )));
    assert!(
        !dispatched.load(Ordering::Acquire),
        "refusal precedes durable intent"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(
        observed.len(),
        21,
        "refusal precedes final admission and import POST"
    );
    assert_eq!(
        error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence,
        expected_queued_evidence(&observed, &responses, false),
        "initial mode/company and all three queued source reads survive refusal"
    );
}
