//! Captured native Trial Balance replay with one admission or stability fault at a time.
use super::*;
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "eebb9a9f-1679-4468-9e8f-814c729674cb";

fn decode(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn companies() -> String {
    decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ))
}

fn extents() -> String {
    include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    )
    .to_string()
}

fn trial_balance() -> String {
    include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
    )
    .to_string()
}

fn identity() -> VerifiedCompanyIdentity {
    let companies = parse_companies_from_collection(&companies()).unwrap();
    let row = companies
        .iter()
        .find(|row| row.guid.as_deref() == Some(GUID))
        .unwrap();
    VerifiedCompanyIdentity::from_observed_companies(
        row.name.clone(),
        GUID.into(),
        row.company_number.clone().unwrap(),
        row.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap()
}

fn xml(text: String) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(text))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}

fn status() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
}

fn education(text: &str) -> String {
    text.replace(
        "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
        "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
    )
}

fn pair(plans: &mut Vec<ScenarioPlan>, response: ScenarioPlan) {
    plans.extend([response.clone(), status(), response, status()]);
}

fn opening_plans(currency: String) -> Vec<ScenarioPlan> {
    let companies = xml(companies());
    let extent = xml(extents());
    let mut plans = vec![status(), companies.clone(), companies.clone()];
    pair(&mut plans, extent);
    pair(&mut plans, xml(currency));
    plans
}

fn complete_plans(currency: String, report: String) -> Vec<ScenarioPlan> {
    let companies = xml(companies());
    let mut plans = opening_plans(currency);
    pair(&mut plans, xml(report));
    pair(&mut plans, xml(extents()));
    plans.extend([companies.clone(), status(), companies]);
    plans
}

fn config(simulator: &SequenceSimulator) -> TallyConfig {
    TallyConfig {
        host: simulator.address().ip().to_string(),
        port: simulator.address().port(),
    }
}

fn join(left: &str, right: &str) -> String {
    sha256_hex(format!("{left}:{right}").as_bytes())
}

#[tokio::test]
async fn trial_balance_refuses_education_before_identity_or_report_dispatch() {
    let simulator = SequenceSimulator::spawn(vec![status(), xml(education(&companies()))]).unwrap();

    let error = TallyRuntime::default()
        .fetch_trial_balance(
            config(&simulator),
            &identity(),
            TrialBalancePeriod::new(
                TallyDate::parse("20260401").unwrap(),
                TallyDate::parse("20260902").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<super::trial_balance::TrialBalanceReadError>()),
        Some(super::trial_balance::TrialBalanceReadError::EducationUnqualified)
    ));
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

#[tokio::test]
async fn trial_balance_refuses_before_books_before_currency_or_report_dispatch() {
    let companies = xml(companies());
    let mut plans = vec![status(), companies.clone(), companies];
    pair(&mut plans, xml(extents()));
    let simulator = SequenceSimulator::spawn(plans).unwrap();

    let error = TallyRuntime::default()
        .fetch_trial_balance(
            config(&simulator),
            &identity(),
            TrialBalancePeriod::new(
                TallyDate::parse("20250101").unwrap(),
                TallyDate::parse("20260902").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<super::trial_balance::TrialBalanceReadError>()),
        Some(super::trial_balance::TrialBalanceReadError::BeforeBooks)
    ));
    assert_eq!(simulator.finish().unwrap().len(), 7);
}

#[tokio::test]
async fn trial_balance_rejects_non_inr_before_trial_balance_dispatch() {
    let captured = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    let foreign = captured.replace(
        "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>",
        "<MAILINGNAME TYPE=\"String\">USD</MAILINGNAME>",
    );
    assert_ne!(foreign, captured);
    let responses = opening_plans(foreign.clone())
        .iter()
        .map(|plan| encode(&plan.fixture.body(), plan.encoding))
        .collect::<Vec<_>>();
    let simulator = SequenceSimulator::spawn(opening_plans(foreign)).unwrap();

    let error = TallyRuntime::default()
        .fetch_trial_balance(
            config(&simulator),
            &identity(),
            TrialBalancePeriod::new(
                TallyDate::parse("20260401").unwrap(),
                TallyDate::parse("20260902").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<super::trial_balance::TrialBalanceReadError>()),
        Some(super::trial_balance::TrialBalanceReadError::Currency(
            "company_base_currency_not_inr"
        ))
    ));
    let evidence = &error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence;
    let observed = simulator.finish().unwrap();
    assert_eq!(
        observed.len(),
        11,
        "currency refusal must precede Trial Balance"
    );
    assert_eq!(
        evidence.request_sha256,
        join(
            &join(
                &observed[0].request_body_sha256,
                &observed[1].request_body_sha256
            ),
            &observed[7].request_body_sha256,
        )
    );
    assert_eq!(
        evidence.response_sha256,
        join(
            &join(&sha256_hex(&responses[0]), &sha256_hex(&responses[1])),
            &sha256_hex(&responses[7])
        )
    );
    assert_eq!(
        evidence.bytes,
        responses[0].len() + responses[1].len() + responses[7].len() * 2
    );
}

#[tokio::test]
async fn trial_balance_replays_captured_native_report_through_all_runtime_brackets() {
    let currency = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    let simulator = SequenceSimulator::spawn(complete_plans(currency, trial_balance())).unwrap();

    let read = TallyRuntime::default()
        .fetch_trial_balance(
            config(&simulator),
            &identity(),
            TrialBalancePeriod::new(
                TallyDate::parse("20260401").unwrap(),
                TallyDate::parse("20260902").unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(read.company_guid, GUID);
    assert_eq!(read.currency.mailing_name, "INR");
    assert!(!read.report.rows.is_empty());
    assert_eq!(simulator.finish().unwrap().len(), 22);
}

#[tokio::test]
async fn trial_balance_rejects_report_or_book_drift_and_retains_completed_source() {
    let currency = decode(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    let captured_report = trial_balance();
    let changed_extent = extents().replace(
        "<ALTMSTID TYPE=\"Number\"> 224</ALTMSTID>",
        "<ALTMSTID TYPE=\"Number\"> 225</ALTMSTID>",
    );
    assert_ne!(changed_extent, extents());

    for fault in ["report", "extent"] {
        let mut plans = opening_plans(currency.clone());
        if fault == "report" {
            plans.extend([
                xml(captured_report.clone()),
                status(),
                xml(format!("{captured_report}\n")),
                status(),
            ]);
        } else {
            pair(&mut plans, xml(captured_report.clone()));
        }
        if fault == "extent" {
            pair(&mut plans, xml(changed_extent.clone()));
        }
        let responses = plans
            .iter()
            .map(|plan| encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let error = TallyRuntime::default()
            .fetch_trial_balance(
                config(&simulator),
                &identity(),
                TrialBalancePeriod::new(
                    TallyDate::parse("20260401").unwrap(),
                    TallyDate::parse("20260902").unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap_err();

        if fault == "report" {
            assert!(matches!(
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<PairedReadValidationError>()),
                Some(PairedReadValidationError::NativeLedgerCollection)
            ));
        } else {
            assert!(matches!(
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<PairedReadValidationError>()),
                Some(PairedReadValidationError::NativeLedgerExtent)
            ));
        }
        let evidence = &error.downcast_ref::<RuntimeReadFailure>().unwrap().evidence;
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), if fault == "report" { 15 } else { 19 });
        let report_index = 11;
        let source_requests = join(
            &join(
                &observed[0].request_body_sha256,
                &observed[1].request_body_sha256,
            ),
            &observed[7].request_body_sha256,
        );
        let report_request = if fault == "report" {
            join(
                &observed[report_index].request_body_sha256,
                &observed[report_index + 2].request_body_sha256,
            )
        } else {
            observed[report_index].request_body_sha256.clone()
        };
        let expected_requests = join(&source_requests, &report_request);
        assert_eq!(evidence.request_sha256, expected_requests);
        let source_response = join(
            &join(&sha256_hex(&responses[0]), &sha256_hex(&responses[1])),
            &sha256_hex(&responses[7]),
        );
        let report_response = if fault == "report" {
            join(
                &sha256_hex(&responses[report_index]),
                &sha256_hex(&responses[report_index + 2]),
            )
        } else {
            sha256_hex(&responses[report_index])
        };
        let expected_response = join(&source_response, &report_response);
        assert_eq!(evidence.response_sha256, expected_response);
        assert_eq!(
            evidence.bytes,
            responses[0].len()
                + responses[1].len()
                + responses[7].len() * 2
                + responses[report_index].len() * if fault == "report" { 1 } else { 2 }
                + if fault == "report" {
                    responses[report_index + 2].len()
                } else {
                    0
                }
        );
    }
}
