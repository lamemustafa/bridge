//! Byte-exact source replay; only the second paired body changes by whitespace.
use super::*;
use tally_protocol_simulator::ObservedRequest;

fn config(simulator: &SequenceSimulator) -> TallyConfig {
    TallyConfig {
        host: "127.0.0.1".into(),
        port: simulator.address().port(),
    }
}
fn drift_pair(plans: &mut Vec<ScenarioPlan>, body: &[u8]) {
    let text = decode(body);
    plans.extend([
        xml(text.clone()),
        status(),
        xml(format!("{text}\n")),
        status(),
    ]);
}
fn expected_source(
    requests: &[ObservedRequest],
    responses: &[Vec<u8>],
    indices: &[usize],
    paired: bool,
) -> RuntimeReadEvidence {
    let mut request_hash = requests[indices[0]].request_body_sha256.clone();
    let mut response_hash = sha256_hex(&responses[indices[0]]);
    let mut bytes = responses[indices[0]].len();
    for &i in &indices[1..] {
        request_hash = join(&request_hash, &requests[i].request_body_sha256);
        response_hash = join(&response_hash, &sha256_hex(&responses[i]));
        bytes += responses[i].len();
    }
    RuntimeReadEvidence {
        request_sha256: request_hash,
        response_sha256: response_hash,
        bytes: if paired { bytes * 2 } else { bytes },
    }
}
fn assert_evidence(actual: &RuntimeReadEvidence, expected: RuntimeReadEvidence) {
    assert_eq!(actual.request_sha256, expected.request_sha256);
    assert_eq!(actual.response_sha256, expected.response_sha256);
    assert_eq!(actual.bytes, expected.bytes);
}
fn responses(plans: &[ScenarioPlan]) -> Vec<Vec<u8>> {
    plans.iter().map(ScenarioPlan::response_bytes).collect()
}
fn retained(error: &anyhow::Error) -> &RuntimeReadEvidence {
    &error
        .downcast_ref::<RuntimeReadFailure>()
        .expect("completed bodies retained")
        .evidence
}

#[tokio::test]
async fn probe_failures_retain_completed_get_and_discovery_before_failed_attempt() {
    let wrong_shape = include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml");
    for fallback in [false, true] {
        for framing in [
            ResponseFraming::ContentLength,
            ResponseFraming::Chunked { chunk_bytes: 37 },
        ] {
            let mut plans = vec![status()];
            if fallback {
                plans.push(xml(decode(wrong_shape)));
            }
            let mut failure = xml(companies());
            failure.http_status = 503;
            plans.push(failure);
            for plan in &mut plans {
                plan.framing = framing;
            }
            let bodies = responses(&plans);
            let simulator = SequenceSimulator::spawn(plans).unwrap();
            let error = TallyRuntime::default()
                .probe_with_wire_evidence(config(&simulator))
                .await
                .unwrap_err();
            assert!(error.chain().any(|cause| matches!(
                cause.downcast_ref::<bridge_tally_transport::TallyTransportError>(),
                Some(bridge_tally_transport::TallyTransportError::HttpStatus { status: 503 })
            )));
            let requests = simulator.finish().unwrap();
            assert_eq!(requests.len(), if fallback { 3 } else { 2 });
            let completed = if fallback { vec![0, 1] } else { vec![0] };
            assert_evidence(
                retained(&error),
                expected_source(&requests, &bodies, &completed, false),
            );
        }
    }
}

#[tokio::test]
async fn outstandings_partial_retains_both_drifting_bodies_and_prior_reports() {
    let reports: [&[u8];4] = [
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-receivable.utf16le.xml"),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-groups.utf16le.xml"),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-payable.utf16le.xml"),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ageing-ledgers.utf16le.xml")
    ];
    for fault in 0..reports.len() {
        let mut plans = vec![status(), xml(companies()), xml(companies())];
        pair(&mut plans, xml(extents()));
        for report in &reports[..fault] {
            pair(&mut plans, xml(decode(report)));
        }
        drift_pair(&mut plans, reports[fault]);
        let bodies = responses(&plans);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let identity = identity_for_guid(&companies(), "eebb9a9f-1679-4468-9e8f-814c729674cb");
        let (load, evidence) = TallyRuntime::default()
            .fetch_outstandings_native(
                config(&simulator),
                &identity,
                TallyDate::parse("20260801").unwrap(),
                OutstandingsCurrencyAssertion::Inr,
                OutstandingsAgeingAnchor::DueDate,
            )
            .await
            .unwrap();
        let expected_reason = [
            "native_bills_report_drifted",
            "native_group_snapshot_drifted",
            "native_bills_report_drifted",
            "native_ledger_snapshot_drifted",
        ][fault];
        assert!(
            matches!(load, OutstandingsLoadResult::Partial { reason, .. } if reason == expected_reason.into())
        );
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), 11 + 4 * fault);
        let mut expected = expected_source(&requests, &bodies, &[0, 1], false);
        for prior in 0..fault {
            expected =
                expected.combine(expected_source(&requests, &bodies, &[7 + 4 * prior], true));
        }
        let i = 7 + 4 * fault;
        expected = expected.combine(expected_source(&requests, &bodies, &[i, i + 2], false));
        assert_evidence(&evidence, expected);
    }
}

#[tokio::test]
async fn party_master_drift_retains_prior_reports_and_exact_typed_cause() {
    let reports: [&[u8];3] = [
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-masters.utf16le.xml"),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-balances.utf16le.xml"),
        include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-groups.utf16le.xml")
    ];
    for fault in 0..reports.len() {
        let mut plans = Vec::new();
        pair(&mut plans, xml(extents()));
        for report in &reports[..fault] {
            pair(&mut plans, xml(decode(report)));
        }
        drift_pair(&mut plans, reports[fault]);
        let bodies = responses(&plans);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let identity = company_identity(&companies());
        let client = TallyClient::new(config(&simulator)).unwrap();
        let error = client
            .fetch_party_ledger_master_source(
                &identity,
                DateBoundaryProfile::ModeAgnostic,
                assertion(&extents(), &identity),
            )
            .await
            .unwrap_err();
        let cause = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<PairedReadValidationError>())
            .unwrap();
        assert!(matches!(
            (fault, cause),
            (0, PairedReadValidationError::PartyLedgerMaster)
                | (1, PairedReadValidationError::PartyLedgerBalance)
                | (2, PairedReadValidationError::PartyLedgerGroup)
        ));
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), 8 + 4 * fault);
        let mut expected = RuntimeReadEvidence::empty();
        for prior in 0..fault {
            expected =
                expected.combine(expected_source(&requests, &bodies, &[4 + 4 * prior], true));
        }
        let i = 4 + 4 * fault;
        expected = expected.combine(expected_source(&requests, &bodies, &[i, i + 2], false));
        assert_evidence(retained(&error), expected);
    }
}

#[tokio::test]
async fn currency_and_basic_ledger_drift_retain_completed_sources() {
    for basic in [false, true] {
        let mut plans = if basic {
            vec![status(), xml(companies()), xml(companies())]
        } else {
            vec![xml(companies())]
        };
        pair(&mut plans, xml(extents()));
        let report: &[u8] = if basic {
            include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-masters.utf16le.xml")
        } else {
            include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml")
        };
        let i = plans.len();
        drift_pair(&mut plans, report);
        let bodies = responses(&plans);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let identity = company_identity(&companies());
        let runtime = TallyRuntime::default();
        let error = if basic {
            runtime
                .fetch_ledgers_with_evidence(config(&simulator), &identity)
                .await
                .unwrap_err()
        } else {
            runtime
                .detect_base_currency_with_extent(config(&simulator), &identity)
                .await
                .unwrap_err()
        };
        let cause = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<PairedReadValidationError>())
            .unwrap();
        assert!(matches!(
            (basic, cause),
            (true, PairedReadValidationError::NativeLedgerCollection)
                | (false, PairedReadValidationError::CurrencyMaster)
        ));
        let requests = simulator.finish().unwrap();
        assert_eq!(requests.len(), i + 4);
        let drift = expected_source(&requests, &bodies, &[i, i + 2], false);
        let expected = if basic {
            expected_source(&requests, &bodies, &[0, 1], false).combine(drift)
        } else {
            drift
        };
        assert_evidence(retained(&error), expected);
    }
}
