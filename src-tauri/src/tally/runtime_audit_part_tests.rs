//! One audit_read part: wire-exact bytes, a single identity-bracketed attempt,
//! a typed failure, and a drain debt after any response Bridge abandoned.
use super::*;
use std::time::Duration;
use tally_protocol_simulator::{
    Delivery, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

fn captured(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

struct Lab {
    companies_xml: String,
    report_xml: String,
    identity: VerifiedCompanyIdentity,
    request: String,
}

fn lab() -> Lab {
    let companies_xml = captured(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml"
    ));
    let report_xml = captured(include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    ));
    let companies = parse_companies_from_collection(&companies_xml).unwrap();
    let company = companies
        .iter()
        .find(|company| company.name == "WR2 Unicode Lab")
        .unwrap();
    let identity = VerifiedCompanyIdentity::from_observed_companies(
        company.name.clone(),
        company.guid.clone().unwrap(),
        company.company_number.clone().unwrap(),
        company.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap();
    let request = ReadOnlyProfile::StandardLedgerCatalogV1 {
        company: &bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
            company.name.clone(),
        )
        .unwrap(),
    }
    .render();
    Lab {
        companies_xml,
        report_xml,
        identity,
        request,
    }
}

fn utf16(xml: &str) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(xml.to_string())).with_encoding(WireEncoding::Utf16Le)
}

fn status() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
}

fn config(simulator: &SequenceSimulator) -> TallyConfig {
    TallyConfig {
        host: "127.0.0.1".into(),
        port: simulator.address().port(),
    }
}

fn policy(timeout: Duration) -> bridge_tally_transport::TransportPolicy {
    bridge_tally_transport::TransportPolicy {
        request_timeout: timeout,
        ..Default::default()
    }
}

async fn part(
    runtime: &TallyRuntime,
    simulator: &SequenceSimulator,
    lab: &Lab,
    shape: AuditPartShape,
) -> Result<AuditPart, AuditPartFailure> {
    runtime
        .fetch_audit_part(
            config(simulator),
            &lab.identity,
            super::super::agent_read_request::AgentReadRequest::parse(lab.request.clone()).unwrap(),
            shape,
        )
        .await
}

#[test]
fn the_part_deadline_is_the_ruled_twenty_seconds() {
    // Owner ruling 2 denied raising it; a change here reverses that ruling.
    assert_eq!(AUDIT_PART_DEADLINE, Duration::from_secs(20));
    assert_eq!(
        AUDIT_PART_DEADLINE,
        bridge_tally_transport::DEFAULT_REQUEST_TIMEOUT
    );
}

#[tokio::test]
async fn a_single_part_keeps_the_wire_bytes_between_two_identity_brackets() {
    let lab = lab();
    let report = utf16(&lab.report_xml);
    let wire = report.response_bytes();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        report,
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let part = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect("a clean single part is admitted");
    // UTF-16LE on the wire, so the decoded text cannot stand in for it.
    assert_eq!(part.encoded_body, wire);
    assert_ne!(part.body.as_bytes(), wire.as_slice());
    assert_eq!(part.body, lab.report_xml);
    assert_eq!(part.encoded_sha256, sha256_hex(&wire));
    let observed = simulator.finish().unwrap();
    // company list, the part, company list: nothing else, and once each.
    assert_eq!(observed.len(), 3);
    assert_eq!(
        part.evidence.request_sha256,
        observed[1].request_body_sha256
    );
    assert_eq!(part.evidence.response_sha256, sha256_hex(&wire));
    assert_eq!(part.evidence.bytes, wire.len());
}

#[tokio::test]
async fn a_paired_part_must_repeat_byte_for_byte() {
    let lab = lab();
    let report = utf16(&lab.report_xml);
    let wire = report.response_bytes();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        report.clone(),
        status(),
        report.clone(),
        status(),
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let admitted = part(&runtime, &simulator, &lab, AuditPartShape::Paired)
        .await
        .expect("a stable pair is admitted");
    assert_eq!(admitted.encoded_body, wire);
    assert_eq!(admitted.evidence.bytes, wire.len() * 2);
    assert_eq!(simulator.finish().unwrap().len(), 6);

    let changed = lab.report_xml.replacen("Cash", "Changed Cash", 1);
    assert_ne!(changed, lab.report_xml);
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        report,
        status(),
        utf16(&changed),
    ])
    .unwrap();
    let failure = part(&runtime, &simulator, &lab, AuditPartShape::Paired)
        .await
        .expect_err("a drifted pair is refused");
    assert_eq!(failure.kind, AuditPartFailureKind::PairDrift);
    assert_eq!(failure.kind.code(), "audit_part_pair_drift");
    assert!(failure.kind.retryable(), "someone else was writing");
    assert!(!failure.kind.owes_drain());
    // Both completed bodies are accounted for, and no body is released.
    assert!(failure.evidence.expect("pair evidence").bytes > wire.len());
    // Refused on the drift itself, before the trailing health check.
    assert_eq!(simulator.finish().unwrap().len(), 4);
}

#[tokio::test]
async fn an_identity_change_at_the_closing_bracket_discards_a_body_that_arrived() {
    let lab = lab();
    let changed = lab.companies_xml.replace(
        lab.identity.company_guid(),
        "00000000-0000-4000-8000-000000000099",
    );
    assert_ne!(changed, lab.companies_xml);
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&changed),
    ])
    .unwrap();
    let failure = part(
        &TallyRuntime::default(),
        &simulator,
        &lab,
        AuditPartShape::Single,
    )
    .await
    .expect_err("the closing bracket refuses");
    assert_eq!(failure.kind, AuditPartFailureKind::IdentityChanged);
    assert_eq!(failure.kind.code(), "audit_part_company_identity_changed");
    assert!(!failure.kind.retryable());
    assert!(
        failure.evidence.is_some(),
        "the part that was read is accounted"
    );
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// The deadline fires, so Tally may still be building the abandoned response.
/// Nothing more is sent until two consecutive probes answer quickly; a probe
/// that itself times out keeps the endpoint refused. Then the same part reads.
#[tokio::test]
async fn a_deadline_owes_a_drain_that_only_two_quick_probes_clear() {
    let lab = lab();
    // Under heavy CPU load a 1 s session deadline can expire during the TCP
    // connect itself, which the transport reports as unreachable rather than
    // a timeout; 2 s leaves room for the connect.
    let timeout = Duration::from_millis(2_000);
    let busy = Duration::from_millis(3_000);
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(busy)),
        status().with_delivery(Delivery::SlowHeaders(busy)),
        status(),
        status(),
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let runtime = TallyRuntime::with_transport_policy(policy(timeout))
        .with_audit_drain_probe_interval(Duration::ZERO)
        // Quickness is not this test's subject: count any answered probe, so a
        // loaded host cannot turn an answer into a "slow" one.
        .with_audit_drain_probe_slow(AUDIT_DRAIN_PROBE_DEADLINE);
    let failure = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect_err("the part outlives its deadline");
    assert_eq!(failure.kind, AuditPartFailureKind::Deadline);
    assert_eq!(failure.kind.code(), "audit_part_deadline_exceeded");
    assert!(failure.kind.retryable() && failure.kind.owes_drain());

    // Refused before anything is sent.
    let refused = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect_err("a drain is owed");
    assert_eq!(refused.kind, AuditPartFailureKind::DrainRequired);
    assert_eq!(refused.kind.code(), "audit_part_drain_required");

    // Let the simulated responder finish the abandoned part, then probe.
    tokio::time::sleep(busy).await;
    // The first probe goes unanswered: still owed, and counted as abandoned.
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 0,
            abandoned_probes: 1
        }
    );
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("still owed after a probe timed out")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    // The responder finishes the unanswered probe about `busy` after it was
    // accepted; leave a wide margin past that under CPU load.
    tokio::time::sleep(busy + Duration::from_millis(1_500)).await;
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 1,
            abandoned_probes: 1
        }
    );
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    let part = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect("the drained endpoint serves the part");
    assert_eq!(part.body, lab.report_xml);
    // Exactly the planned requests: the refusals sent nothing.
    assert_eq!(simulator.finish().unwrap().len(), 8);
}

#[tokio::test]
async fn a_slow_probe_answer_does_not_count_towards_the_drain() {
    let lab = lab();
    let timeout = Duration::from_millis(6_000);
    let slow = Duration::from_millis(4_000);
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(Duration::from_millis(6_500))),
        status(),
        status().with_delivery(Delivery::SlowHeaders(slow)),
        status(),
        status(),
    ])
    .unwrap();
    // A 2 s threshold with room either side under CPU load: quick answers
    // are milliseconds, the slow one takes 4 s, the session deadline is 6 s.
    let runtime = TallyRuntime::with_transport_policy(policy(timeout))
        .with_audit_drain_probe_interval(Duration::ZERO)
        .with_audit_drain_probe_slow(Duration::from_millis(2_000));
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("deadline")
            .kind,
        AuditPartFailureKind::Deadline
    );
    // The responder is busy until 3.5 s; probe well after.
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    let owed = |quick_probes| AuditDrainStatus::Owed {
        quick_probes,
        abandoned_probes: 0,
    };
    assert_eq!(runtime.drain_probe(config(&simulator)).await, owed(1));
    // Answered, but slower than the drain threshold: the count starts again,
    // and an answered probe is not an abandoned one.
    assert_eq!(runtime.drain_probe(config(&simulator)).await, owed(0));
    assert_eq!(runtime.drain_probe(config(&simulator)).await, owed(1));
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    assert_eq!(simulator.finish().unwrap().len(), 6);
}

#[tokio::test]
async fn a_probe_with_nothing_owed_sends_nothing() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![utf16(&lab.companies_xml)]).unwrap();
    let runtime = TallyRuntime::default();
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    // The one planned response is still unconsumed.
    drop(simulator);
}

#[tokio::test]
async fn a_dropped_connection_owes_a_drain() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::ResetAfterRequestProcessed {
            delay: Duration::ZERO,
        }),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let failure = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect_err("the connection was dropped");
    assert_eq!(failure.kind, AuditPartFailureKind::ConnectionDropped);
    assert_eq!(failure.kind.code(), "audit_part_connection_dropped");
    assert!(failure.kind.retryable() && failure.kind.owes_drain());
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("owed")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

#[tokio::test]
async fn an_oversized_part_is_not_retried_as_it_stands() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        ScenarioPlan::new(Fixture::Oversized {
            minimum_bytes: 64 * 1024,
        }),
    ])
    .unwrap();
    let runtime = TallyRuntime::with_transport_policy(bridge_tally_transport::TransportPolicy {
        xml_response_max_bytes: 32 * 1024,
        ..Default::default()
    });
    let failure = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect_err("over the response limit");
    assert_eq!(failure.kind, AuditPartFailureKind::SizeLimit);
    assert_eq!(
        failure.kind.code(),
        "audit_part_response_size_limit_exceeded"
    );
    // The plan must divide it; sending it again would fail the same way.
    assert!(!failure.kind.retryable());
    // Bridge stopped reading mid-body, so Tally may still be sending it.
    assert!(failure.kind.owes_drain());
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("the debt was registered, not only classified")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

#[test]
fn every_failure_kind_has_a_distinct_stable_code() {
    let kinds = [
        AuditPartFailureKind::DrainRequired,
        AuditPartFailureKind::Deadline,
        AuditPartFailureKind::SizeLimit,
        AuditPartFailureKind::ConnectionDropped,
        AuditPartFailureKind::Unreachable,
        AuditPartFailureKind::PairDrift,
        AuditPartFailureKind::IdentityChanged,
        AuditPartFailureKind::EducationBoundary,
    ];
    let codes = kinds
        .iter()
        .map(|kind| kind.code())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(codes.len(), kinds.len());
    assert!(codes.iter().all(|code| code.starts_with("audit_part_")));
    // Nothing reached Tally, so nothing is owed, and it may be tried again.
    assert!(AuditPartFailureKind::Unreachable.retryable());
    assert!(!AuditPartFailureKind::Unreachable.owes_drain());
    assert!(!AuditPartFailureKind::IdentityChanged.owes_drain());
    assert!(AuditPartFailureKind::DrainRequired.retryable());
    assert!(!AuditPartFailureKind::Other("request_size_limit_exceeded").retryable());
}

#[tokio::test]
async fn elapsed_times_the_data_request_and_not_the_brackets() {
    let lab = lab();
    let data = Duration::from_millis(400);
    let bracket = Duration::from_millis(600);
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml).with_delivery(Delivery::SlowHeaders(bracket)),
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(data)),
        utf16(&lab.companies_xml).with_delivery(Delivery::SlowHeaders(bracket)),
    ])
    .unwrap();
    let admitted = part(
        &TallyRuntime::default(),
        &simulator,
        &lab,
        AuditPartShape::Single,
    )
    .await
    .expect("a slow part inside the deadline is admitted");
    assert!(admitted.elapsed >= data, "{:?}", admitted.elapsed);
    assert!(admitted.elapsed < data + bracket, "{:?}", admitted.elapsed);
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// A part already queued behind one that abandons a response is refused at
/// dispatch, not only a part that starts afterwards.
#[tokio::test]
async fn a_part_queued_behind_an_abandoned_part_is_refused_at_dispatch() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(Duration::from_millis(2_000))),
    ])
    .unwrap();
    let runtime = TallyRuntime::with_transport_policy(policy(Duration::from_millis(1_000)));
    let first = part(&runtime, &simulator, &lab, AuditPartShape::Single);
    let queued = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        part(&runtime, &simulator, &lab, AuditPartShape::Single).await
    };
    let (first, queued) = tokio::join!(first, queued);
    assert_eq!(
        first.expect_err("deadline").kind,
        AuditPartFailureKind::Deadline
    );
    assert_eq!(
        queued.expect_err("queued behind the abandoned part").kind,
        AuditPartFailureKind::DrainRequired
    );
    // Only the first part's two requests reached the responder.
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

/// A caller that stops waiting drops the future mid-request. Tally keeps
/// working, so the debt armed before sending must survive the drop.
#[tokio::test]
async fn a_dropped_part_leaves_its_drain_owed() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(Duration::from_millis(1_000))),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    assert!(tokio::time::timeout(
        Duration::from_millis(300),
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
    )
    .await
    .is_err());
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("owed after the drop")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

#[tokio::test]
async fn a_request_that_does_not_name_the_verified_company_is_never_sent() {
    let lab = lab();
    let other = ReadOnlyProfile::StandardLedgerCatalogV1 {
        company: &bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
            "Entirely Different Company".to_string(),
        )
        .unwrap(),
    }
    .render();
    let unnamed = lab.request.replace(
        &format!(
            "<SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>",
            lab.identity.display_name()
        ),
        "",
    );
    assert_ne!(unnamed, lab.request, "the fixture names its company");
    let twice = lab.request.replace(
        "<SVCURRENTCOMPANY>",
        "<SVCURRENTCOMPANY>WR2 Unicode Lab</SVCURRENTCOMPANY><SVCURRENTCOMPANY>",
    );
    // One element, right name, wrong place: Tally would not read it there.
    let misplaced = unnamed.replacen(
        "<BODY>",
        &format!(
            "<BODY><NOTE><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></NOTE>",
            lab.identity.display_name()
        ),
        1,
    );
    assert_ne!(misplaced, unnamed);
    let simulator = SequenceSimulator::spawn(vec![utf16(&lab.companies_xml)]).unwrap();
    let runtime = TallyRuntime::default();
    for request in [other, unnamed, twice, misplaced] {
        let failure = runtime
            .fetch_audit_part(
                config(&simulator),
                &lab.identity,
                super::super::agent_read_request::AgentReadRequest::parse(request).unwrap(),
                AuditPartShape::Single,
            )
            .await
            .expect_err("not scoped to the verified company");
        assert_eq!(failure.kind, AuditPartFailureKind::RequestNotCompanyScoped);
        assert!(!failure.kind.retryable() && !failure.kind.owes_drain());
    }
    // Nothing was sent, so nothing is owed either.
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    drop(simulator);
}

#[tokio::test]
async fn an_error_envelope_is_rejected_whole_and_owes_nothing() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16("<ENVELOPE><LINEERROR>Could not find company</LINEERROR></ENVELOPE>"),
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let failure = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect_err("an error envelope is not a part");
    assert_eq!(failure.kind, AuditPartFailureKind::ResponseRejected);
    assert_eq!(failure.kind.code(), "audit_part_response_rejected");
    assert!(!failure.kind.retryable() && !failure.kind.owes_drain());
    assert!(failure.evidence.is_some(), "the response read is accounted");
    // Read to the end, so nothing is left running: the next part is sent.
    part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect("no drain is owed");
    assert_eq!(simulator.finish().unwrap().len(), 5);
}

#[tokio::test]
async fn a_refused_response_head_owes_a_drain() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_http_status(500),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let failure = part(&runtime, &simulator, &lab, AuditPartShape::Single)
        .await
        .expect_err("HTTP 500");
    assert_eq!(
        failure.kind,
        AuditPartFailureKind::HeadRejected("http_status_failure")
    );
    assert!(!failure.kind.retryable() && failure.kind.owes_drain());
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("the body was abandoned unread")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    assert_eq!(simulator.finish().unwrap().len(), 2);
}

async fn owe_a_drain(runtime: &TallyRuntime, simulator: &SequenceSimulator, lab: &Lab) {
    assert_eq!(
        part(runtime, simulator, lab, AuditPartShape::Single)
            .await
            .expect_err("dropped")
            .kind,
        AuditPartFailureKind::ConnectionDropped
    );
}

fn dropped(lab: &Lab) -> Vec<ScenarioPlan> {
    vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::ResetAfterRequestProcessed {
            delay: Duration::ZERO,
        }),
    ]
}

/// The probe's own 5 s deadline, under the production 20 s session deadline.
#[tokio::test]
async fn a_probe_gives_up_at_its_own_deadline_not_the_sessions() {
    let lab = lab();
    let mut plans = dropped(&lab);
    plans.push(status().with_delivery(Delivery::SlowHeaders(Duration::from_millis(7_000))));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default();
    owe_a_drain(&runtime, &simulator, &lab).await;
    let started = std::time::Instant::now();
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 0,
            abandoned_probes: 1
        }
    );
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(4_900), "{waited:?}");
    assert!(waited < Duration::from_millis(6_500), "{waited:?}");
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

#[tokio::test]
async fn probes_are_spaced_and_a_premature_one_sends_nothing() {
    let lab = lab();
    let mut plans = dropped(&lab);
    plans.push(status());
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default()
        // Quickness is not this test's subject: count any answered probe, so a
        // loaded host cannot turn an answer into a "slow" one.
        .with_audit_drain_probe_slow(AUDIT_DRAIN_PROBE_DEADLINE);
    owe_a_drain(&runtime, &simulator, &lab).await;
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 1,
            abandoned_probes: 0
        }
    );
    match runtime.drain_probe(config(&simulator)).await {
        AuditDrainStatus::Wait { retry_after } => {
            assert!(retry_after > Duration::ZERO && retry_after <= AUDIT_DRAIN_PROBE_INTERVAL);
        }
        other => panic!("expected Wait, got {other:?}"),
    }
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// Repeated silence stops the probing; only the operator clears it.
#[tokio::test]
async fn unanswered_probes_stop_and_wait_for_the_operator() {
    let lab = lab();
    let slow = Duration::from_millis(1_500);
    let mut plans = dropped(&lab);
    plans.extend([
        status().with_delivery(Delivery::SlowHeaders(slow)),
        status().with_delivery(Delivery::SlowHeaders(slow)),
    ]);
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::with_transport_policy(policy(Duration::from_millis(1_000)))
        .with_audit_drain_probe_interval(Duration::ZERO);
    owe_a_drain(&runtime, &simulator, &lab).await;
    assert!(!runtime.clear_audit_drain_after_operator_check(&config(&simulator)));
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 0,
            abandoned_probes: 1
        }
    );
    tokio::time::sleep(slow).await;
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::OperatorRequired
    );
    // Nothing more is sent however often it is asked.
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::OperatorRequired
    );
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("still owed")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    assert!(runtime.clear_audit_drain_after_operator_check(&config(&simulator)));
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    // The drain no longer refuses. Three consecutive failures have opened the
    // runtime's own circuit breaker, which still refuses until its cooldown,
    // without sending anything.
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("circuit cooling down")
            .kind,
        AuditPartFailureKind::NotSent("endpoint_circuit_cooldown")
    );
    assert_eq!(simulator.finish().unwrap().len(), 4);
}

/// Two probes sent at once are one probe, not two consecutive ones.
#[tokio::test]
async fn concurrent_probes_cannot_count_twice() {
    let lab = lab();
    let mut plans = dropped(&lab);
    plans.push(status().with_delivery(Delivery::SlowHeaders(Duration::from_millis(200))));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default()
        .with_audit_drain_probe_interval(Duration::ZERO)
        // Quickness is not this test's subject: count any answered probe, so a
        // loaded host cannot turn an answer into a "slow" one.
        .with_audit_drain_probe_slow(AUDIT_DRAIN_PROBE_DEADLINE);
    owe_a_drain(&runtime, &simulator, &lab).await;
    let (a, b) = tokio::join!(
        runtime.drain_probe(config(&simulator)),
        runtime.drain_probe(config(&simulator))
    );
    let statuses = [a, b];
    assert!(
        statuses.contains(&AuditDrainStatus::Owed {
            quick_probes: 1,
            abandoned_probes: 0
        }),
        "{statuses:?}"
    );
    assert!(
        statuses
            .iter()
            .any(|status| matches!(status, AuditDrainStatus::Wait { .. })),
        "{statuses:?}"
    );
    // One probe reached the responder.
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// A part settles only the entry it armed, so a refusal of one part can never
/// clear another part's debt.
#[test]
fn a_part_settles_only_the_debt_it_armed() {
    let registry: AuditDrainRegistry = Arc::new(Mutex::new(HashMap::new()));
    let endpoint = EndpointKey::from_config(&TallyConfig {
        host: "127.0.0.1".into(),
        port: 9,
    })
    .unwrap();
    arm_audit_drain(&registry, &endpoint, 1).unwrap();
    assert!(arm_audit_drain(&registry, &endpoint, 2).is_err());
    settle_audit_drain(&registry, &endpoint, 2, false);
    assert!(registry.lock().unwrap().contains_key(&endpoint));
    settle_audit_drain(&registry, &endpoint, 1, true);
    let debt = registry.lock().unwrap()[&endpoint];
    assert!(!debt.in_flight, "owed, no longer in flight");
    // Settled already: a late settle by its own ticket cannot clear it either.
    settle_audit_drain(&registry, &endpoint, 1, false);
    assert!(registry.lock().unwrap().contains_key(&endpoint));
}

/// A debt at the operator cap sends no probe at all, whatever the circuit
/// breaker is doing.
#[tokio::test]
async fn a_capped_debt_sends_no_probe() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        status().with_delivery(Delivery::SlowHeaders(Duration::from_millis(1_500)))
    ])
    .unwrap();
    let runtime = TallyRuntime::default().with_audit_drain_probe_interval(Duration::ZERO);
    let endpoint = EndpointKey::from_config(&config(&simulator)).unwrap();
    runtime.audit_drain.lock().unwrap().insert(
        endpoint,
        AuditDrainDebt {
            in_flight: false,
            abandoned_probes: AUDIT_DRAIN_ABANDONED_PROBES,
            ..AuditDrainDebt::armed(0)
        },
    );
    let started = std::time::Instant::now();
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::OperatorRequired
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("owed")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    drop(simulator);
}

/// Parts are serialised, not refused, while another part is merely running.
#[tokio::test]
async fn a_part_queued_behind_a_clean_part_is_sent_after_it() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(Duration::from_millis(300))),
        utf16(&lab.companies_xml),
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let first = part(&runtime, &simulator, &lab, AuditPartShape::Single);
    let queued = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        part(&runtime, &simulator, &lab, AuditPartShape::Single).await
    };
    let (first, queued) = tokio::join!(first, queued);
    first.expect("first part");
    queued.expect("the queued part waits and is then sent");
    assert_eq!(simulator.finish().unwrap().len(), 6);
}

/// A probe whose caller stopped waiting must not lock the endpoint out: once
/// stale it counts as abandoned, and the cap still reaches the operator.
#[tokio::test]
async fn a_dropped_probe_counts_as_abandoned_once_stale() {
    let lab = lab();
    let mut plans = dropped(&lab);
    // The dropped probe's answer takes 600 ms; the probe is dropped at 300 ms.
    plans.push(status().with_delivery(Delivery::SlowHeaders(Duration::from_millis(600))));
    plans.push(status());
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let stale = Duration::from_millis(1_500);
    let runtime = TallyRuntime::default()
        .with_audit_drain_probe_interval(Duration::ZERO)
        .with_audit_drain_probe_stale(stale)
        // Quickness is not this test's subject: count any answered probe, so a
        // loaded host cannot turn an answer into a "slow" one.
        .with_audit_drain_probe_slow(AUDIT_DRAIN_PROBE_DEADLINE);
    owe_a_drain(&runtime, &simulator, &lab).await;
    assert!(tokio::time::timeout(
        Duration::from_millis(300),
        runtime.drain_probe(config(&simulator))
    )
    .await
    .is_err());
    // Still within the stale limit: nothing is sent.
    assert!(matches!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Wait { .. }
    ));
    // Past the stale limit, and well past the responder finishing the
    // dropped probe's answer, even under CPU load.
    tokio::time::sleep(stale + Duration::from_millis(1_000)).await;
    // Counted as abandoned; one more probe is sent and answered quickly.
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 1,
            abandoned_probes: 1
        }
    );
    assert_eq!(simulator.finish().unwrap().len(), 4);
}

/// A debt whose part is still running is not drained: no probe is sent and
/// nothing is cleared out from under the part.
#[tokio::test]
async fn a_running_part_is_never_drained_from_under_it() {
    let simulator = SequenceSimulator::spawn(vec![status(), status()]).unwrap();
    let runtime = TallyRuntime::default().with_audit_drain_probe_interval(Duration::ZERO);
    let endpoint = EndpointKey::from_config(&config(&simulator)).unwrap();
    arm_audit_drain(&runtime.audit_drain, &endpoint, 999).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            runtime.drain_probe(config(&simulator)).await,
            AuditDrainStatus::Wait { .. }
        ));
    }
    let debt = runtime.audit_drain.lock().unwrap()[&endpoint];
    assert!(debt.in_flight && debt.quick_probes == 0);
    drop(simulator);
}

/// A part's future dropped mid-request leaves a settled, owed debt, so the
/// next part is refused before it is queued and probes can drain it.
///
/// Deterministic by construction: the part is aborted only once the registry
/// shows it armed, not after a guessed delay, and draining is asserted as
/// "clears within a bounded number of probes", with every answered probe
/// counting, so it does not depend on how long the simulated responder stays
/// busy with the abandoned request.
#[tokio::test]
async fn a_dropped_part_is_owed_and_drainable() {
    let lab = lab();
    let mut plans = vec![
        utf16(&lab.companies_xml),
        // Long enough that the part is always still running when aborted.
        utf16(&lab.report_xml).with_delivery(Delivery::SlowHeaders(Duration::from_millis(1_500))),
    ];
    plans.extend([status(), status()]);
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default()
        .with_audit_drain_probe_interval(Duration::ZERO)
        .with_audit_drain_probe_slow(AUDIT_DRAIN_PROBE_DEADLINE);
    let endpoint = EndpointKey::from_config(&config(&simulator)).unwrap();
    let running = {
        let runtime = runtime.clone();
        let config = config(&simulator);
        let identity = lab.identity.clone();
        let request =
            super::super::agent_read_request::AgentReadRequest::parse(lab.request.clone()).unwrap();
        tokio::spawn(async move {
            runtime
                .fetch_audit_part(config, &identity, request, AuditPartShape::Single)
                .await
        })
    };
    // Abort only once the data request itself has reached the responder
    // (the opening bracket plus the part), while its response is held.
    tokio::time::timeout(Duration::from_secs(20), async {
        while simulator.received() < 2 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the part's data request is received");
    assert!(runtime.audit_drain.lock().unwrap()[&endpoint].in_flight);
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    // Dropped unsettled: owed, and no longer marked as a running part.
    assert!(!runtime.audit_drain.lock().unwrap()[&endpoint].in_flight);
    assert_eq!(
        part(&runtime, &simulator, &lab, AuditPartShape::Single)
            .await
            .expect_err("owed")
            .kind,
        AuditPartFailureKind::DrainRequired
    );
    // The responder finishes the abandoned part (1.5 s) and then answers each
    // probe; every answered probe counts, within the probe's own 5 s deadline.
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Owed {
            quick_probes: 1,
            abandoned_probes: 0
        }
    );
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    drop(simulator);
}

/// A tool call withdrawn before the part is queued (#584) sends nothing and
/// owes nothing.
#[tokio::test]
async fn a_withdrawn_call_sends_no_part_and_owes_nothing() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![utf16(&lab.companies_xml)]).unwrap();
    let runtime = TallyRuntime::default();
    let withdrawn = tokio_util::sync::CancellationToken::new();
    withdrawn.cancel();
    let failure = TOOL_CANCELLATION
        .scope(
            withdrawn,
            part(&runtime, &simulator, &lab, AuditPartShape::Single),
        )
        .await
        .expect_err("withdrawn");
    assert_eq!(
        failure.kind,
        AuditPartFailureKind::NotSent("request_cancelled")
    );
    assert!(failure.kind.retryable() && !failure.kind.owes_drain());
    assert_eq!(
        runtime.drain_probe(config(&simulator)).await,
        AuditDrainStatus::Clear
    );
    drop(simulator);
}

fn education(companies_xml: &str) -> String {
    let flipped = companies_xml.replace(
        "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
        "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
    );
    assert_ne!(flipped, companies_xml, "the fixture carries EDUMODE");
    flipped
}

fn dated(request: &str, day: &str) -> String {
    let dated = request.replacen(
        "</STATICVARIABLES>",
        &format!(
            "<SVFROMDATE TYPE=\"Date\">{day}</SVFROMDATE><SVTODATE TYPE=\"Date\">{day}</SVTODATE></STATICVARIABLES>"
        ),
        1,
    );
    assert_ne!(dated, request, "the request carries STATICVARIABLES");
    dated
}

async fn dated_part(
    runtime: &TallyRuntime,
    simulator: &SequenceSimulator,
    lab: &Lab,
    day: &str,
) -> Result<AuditPart, AuditPartFailure> {
    runtime
        .fetch_audit_part(
            config(simulator),
            &lab.identity,
            super::super::agent_read_request::AgentReadRequest::parse(dated(&lab.request, day))
                .unwrap(),
            AuditPartShape::Single,
        )
        .await
}

/// bridge#581 for audit parts: Education answers a window starting on another
/// day with a well-formed empty collection, so the part is refused before it
/// is sent, and nothing is owed.
#[tokio::test]
async fn an_education_endpoint_refuses_a_part_it_would_serve_empty() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![utf16(&education(&lab.companies_xml))]).unwrap();
    let runtime = TallyRuntime::default();
    let failure = dated_part(&runtime, &simulator, &lab, "20250403")
        .await
        .expect_err("Education does not honour the 3rd");
    assert_eq!(failure.kind, AuditPartFailureKind::EducationBoundary);
    assert_eq!(
        failure.kind.code(),
        "audit_part_window_unsupported_in_education"
    );
    assert!(!failure.kind.retryable() && !failure.kind.owes_drain());
    let endpoint = config(&simulator);
    // Only the opening bracket was sent.
    assert_eq!(simulator.finish().unwrap().len(), 1);
    assert_eq!(runtime.drain_probe(endpoint).await, AuditDrainStatus::Clear);
}

#[tokio::test]
async fn an_education_endpoint_serves_a_part_on_a_day_it_honours() {
    let lab = lab();
    let companies = education(&lab.companies_xml);
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&companies),
        utf16(&lab.report_xml),
        utf16(&companies),
    ])
    .unwrap();
    let admitted = dated_part(&TallyRuntime::default(), &simulator, &lab, "20250401")
        .await
        .expect("the 1st is honoured");
    assert_eq!(
        admitted.boundary_profile,
        DateBoundaryProfile::EducationRestricted
    );
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// A licence dropping to Education during the part: which mode answered is
/// unknown, so the part is refused as if it had been sent in Education, with
/// what was read accounted for. The body was read to the end: nothing owed.
#[tokio::test]
async fn education_first_seen_at_the_closing_bracket_refuses_the_part() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&education(&lab.companies_xml)),
    ])
    .unwrap();
    let runtime = TallyRuntime::default();
    let failure = dated_part(&runtime, &simulator, &lab, "20250403")
        .await
        .expect_err("closing bracket reports Education");
    assert_eq!(failure.kind, AuditPartFailureKind::EducationBoundary);
    assert!(
        failure.evidence.is_some(),
        "the part that was read is accounted"
    );
    let endpoint = config(&simulator);
    assert_eq!(simulator.finish().unwrap().len(), 3);
    assert_eq!(runtime.drain_probe(endpoint).await, AuditDrainStatus::Clear);
}

#[tokio::test]
async fn a_licensed_part_reports_the_ordinary_profile() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let admitted = dated_part(&TallyRuntime::default(), &simulator, &lab, "20250403")
        .await
        .expect("licensed");
    assert_eq!(admitted.boundary_profile, DateBoundaryProfile::ModeAgnostic);
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// Education first reported at the closing bracket on a day it honours: the
/// part is admitted and reports Education, the stricter of the two profiles,
/// so a caller keeps Education in force for the rest of the window.
#[tokio::test]
async fn a_part_reports_education_when_only_the_closing_bracket_saw_it() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&lab.companies_xml),
        utf16(&lab.report_xml),
        utf16(&education(&lab.companies_xml)),
    ])
    .unwrap();
    let admitted = dated_part(&TallyRuntime::default(), &simulator, &lab, "20250401")
        .await
        .expect("the 1st is honoured in either mode");
    assert_eq!(
        admitted.boundary_profile,
        DateBoundaryProfile::EducationRestricted
    );
    assert_eq!(simulator.finish().unwrap().len(), 3);
}

/// The other asymmetric case: Education at the opening bracket, licensed at the
/// closing one, on a day Education honours. Still Education.
#[tokio::test]
async fn a_part_keeps_education_seen_only_at_the_opening_bracket() {
    let lab = lab();
    let simulator = SequenceSimulator::spawn(vec![
        utf16(&education(&lab.companies_xml)),
        utf16(&lab.report_xml),
        utf16(&lab.companies_xml),
    ])
    .unwrap();
    let admitted = dated_part(&TallyRuntime::default(), &simulator, &lab, "20250401")
        .await
        .expect("the 1st is honoured in either mode");
    assert_eq!(
        admitted.boundary_profile,
        DateBoundaryProfile::EducationRestricted
    );
    assert_eq!(simulator.finish().unwrap().len(), 3);
}
