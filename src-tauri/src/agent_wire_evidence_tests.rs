//! Transport regressions reuse the existing simulator/captured catalogue inputs.
use super::*;
use crate::agent::{parse_agent_rows, Redaction, Settings};

fn server_for(address: std::net::SocketAddr, data_dir: &Path) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: address.ip().to_string(),
            port: address.port(),
        },
        data_dir: data_dir.to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    })
}

fn response_bytes(plan: &ScenarioPlan) -> Vec<u8> {
    tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding)
}

fn join_hashes(left: &str, right: &str) -> String {
    sha256_hex(format!("{left}:{right}").as_bytes())
}

#[tokio::test]
async fn status_evidence_matches_observed_request_and_encoded_response_bodies() {
    let mut previous = None;
    for framing in [
        ResponseFraming::ContentLength,
        ResponseFraming::Chunked { chunk_bytes: 37 },
    ] {
        let plans = import_cycle_plans();
        let status = plans[1].clone();
        let company = plans[0].clone().with_framing(framing);
        let expected_bytes = response_bytes(&status).len() + response_bytes(&company).len();
        let expected_response = join_hashes(
            &sha256_hex(&response_bytes(&status)),
            &sha256_hex(&response_bytes(&company)),
        );
        let simulator = SequenceSimulator::spawn(vec![status, company]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_for(simulator.address(), directory.path());
        let response = server.call_tool("tally_status", json!({})).await;
        assert_eq!(response["isError"], false);
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["bytes"], expected_bytes);
        assert_eq!(evidence["response_sha256"], expected_response);
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 2);
        assert_eq!(
            evidence["request_sha256"],
            join_hashes(
                &observed[0].request_body_sha256,
                &observed[1].request_body_sha256
            )
        );
        let result = &response["structuredContent"]["result"];
        if let Some((old_result, old_hash)) = previous {
            assert_eq!(result["loaded_companies"], old_result);
            assert_eq!(evidence["response_sha256"], old_hash);
        }
        previous = Some((
            result["loaded_companies"].clone(),
            evidence["response_sha256"].clone(),
        ));
    }
}

#[tokio::test]
async fn voucher_selector_catalogue_contributes_to_final_wire_evidence() {
    let mut previous = None;
    for framing in [
        ResponseFraming::ContentLength,
        ResponseFraming::Chunked { chunk_bytes: 37 },
    ] {
        let cycle = import_cycle_plans();
        let mut plans = cycle[..10].to_vec();
        plans[5] = plans[5].clone().with_framing(framing);
        plans[7] = plans[7].clone().with_framing(framing);
        plans.extend([
            cycle[0].clone(),
            cycle[21].clone(),
            cycle[1].clone(),
            cycle[21].clone(),
            cycle[1].clone(),
            cycle[0].clone(),
        ]);
        plans.extend(cycle[4..10].iter().cloned());
        let company = response_bytes(&plans[0]);
        let catalogue = response_bytes(&plans[5]);
        let vouchers = response_bytes(&plans[11]);
        let expected_response = join_hashes(
            &join_hashes(
                &join_hashes(&sha256_hex(&company), &sha256_hex(&catalogue)),
                &sha256_hex(&vouchers),
            ),
            &sha256_hex(&catalogue),
        );
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_for(simulator.address(), directory.path());
        let response = server
            .call_tool(
                "vouchers",
                json!({"company_guid":CAPTURED_GUID,
            "from":"20260901","to":"20260902","ledger":"Cash"}),
            )
            .await;
        assert_eq!(
            response["isError"], false,
            "{}",
            response["structuredContent"]["result"]
        );
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["response_sha256"], expected_response);
        assert_eq!(
            evidence["bytes"],
            2 * (company.len() + 2 * catalogue.len() + vouchers.len())
        );
        let items = &response["structuredContent"]["result"]["items"];
        assert_eq!(items.as_array().unwrap().len(), 2);
        if let Some((old_items, old_hash)) = previous {
            assert_eq!(*items, old_items);
            assert_eq!(evidence["response_sha256"], old_hash);
        }
        previous = Some((items.clone(), evidence["response_sha256"].clone()));
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 22);
        assert_eq!(
            evidence["request_sha256"],
            join_hashes(
                &join_hashes(
                    &join_hashes(
                        &observed[0].request_body_sha256,
                        &observed[5].request_body_sha256,
                    ),
                    &observed[11].request_body_sha256,
                ),
                &observed[17].request_body_sha256,
            ),
        );
    }
}

#[test]
fn zero_byte_wire_response_is_not_an_absent_evidence_sentinel() {
    use crate::tally::runtime::RuntimeReadEvidence;
    let empty_body = sha256_hex(b"");
    let observed_empty = RuntimeReadEvidence {
        request_sha256: empty_body.clone(),
        response_sha256: empty_body.clone(),
        bytes: 0,
    };
    let next = RuntimeReadEvidence {
        request_sha256: sha256_hex(b"request"),
        response_sha256: sha256_hex(b"response"),
        bytes: 8,
    };
    let combined = RuntimeReadEvidence::empty()
        .combine(observed_empty)
        .combine(next.clone());
    assert_eq!(
        combined.request_sha256,
        join_hashes(&empty_body, &next.request_sha256)
    );
    assert_eq!(
        combined.response_sha256,
        join_hashes(&empty_body, &next.response_sha256)
    );
    assert_eq!(combined.bytes, next.bytes);
}

#[tokio::test]
async fn write_shaped_adapter_request_is_refused_before_any_transport() {
    let plans = import_cycle_plans();
    let companies =
        bridge_tally_protocol::parse_companies_from_collection(&plans[0].fixture.body()).unwrap();
    let identity = crate::tally::VerifiedCompanyIdentity::from_observed_companies(
        "WR2 Unicode Lab".into(),
        CAPTURED_GUID.into(),
        "1".into(),
        "20260401".into(),
        &companies,
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_for("127.0.0.1:9".parse().unwrap(), directory.path());
    for operation in ["Import Data", "Execute", "Import"] {
        let request = format!("<ENVELOPE><HEADER><TALLYREQUEST>{operation}</TALLYREQUEST><TYPE>Collection</TYPE></HEADER><BODY/></ENVELOPE>");
        assert_eq!(
            server
                .post_read(&identity, request)
                .await
                .err()
                .unwrap()
                .code,
            "agent_write_dispatch_forbidden"
        );
    }
}

#[path = "agent_voucher_selection_tests.rs"]
mod selection_tests;

#[tokio::test]
async fn opening_mode_refusals_retain_probe_evidence_through_agent_mapping() {
    for (tool, args, code) in [
        (
            "ledger_masters",
            json!({"company_guid": CAPTURED_GUID}),
            "financial_read_profile_unqualified",
        ),
        (
            "ledger_movement",
            json!({"company_guid": CAPTURED_GUID, "from":"20260901", "to":"20260902"}),
            "financial_read_profile_unqualified",
        ),
    ] {
        // Reuse the existing identity/status replay without a recognized mode.
        // The new opening probe must refuse before any ledger export and its
        // observed bytes must survive the adapter's stable public error code.
        let cycle = import_cycle_plans();
        let mut plans = cycle[..4].to_vec();
        plans.extend([cycle[1].clone(), cycle[0].clone()]);
        let company = response_bytes(&cycle[0]);
        let status = response_bytes(&cycle[1]);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_for(simulator.address(), directory.path());
        let response = server.call_tool(tool, args).await;
        let content = &response["structuredContent"];
        assert_eq!(content["result"]["error"]["code"], code);
        let evidence = &content["evidence"];
        assert_eq!(evidence["state"], "partial");
        assert_eq!(evidence["bytes"], 3 * company.len() + status.len());
        assert_eq!(
            evidence["response_sha256"],
            join_hashes(
                &sha256_hex(&company),
                &join_hashes(&sha256_hex(&status), &sha256_hex(&company))
            )
        );
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 6);
        assert_eq!(
            evidence["request_sha256"],
            join_hashes(
                &observed[0].request_body_sha256,
                &join_hashes(
                    &observed[4].request_body_sha256,
                    &observed[5].request_body_sha256
                )
            )
        );
        let history = server.call_tool("read_evidence", json!({})).await;
        assert_eq!(
            history["structuredContent"]["result"]["records"][0],
            *evidence
        );
    }
}

#[tokio::test]
async fn paired_transport_refusal_retains_completed_catalogue_through_tool_and_history() {
    for fail_at in [6, 7, 8, 9] {
        let mut plans = import_cycle_plans()[..10].to_vec();
        let company_bytes = response_bytes(&plans[0]);
        let catalogue_bytes = response_bytes(&plans[5]);
        plans[fail_at].http_status = 503;
        plans.truncate(fail_at + 1);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut server = server_for(simulator.address(), directory.path());
        server.settings.import_enabled = true;
        let response = server
            .call_tool(
                "validate_masters",
                json!({"company_guid":CAPTURED_GUID,"ledgers":["Cash"]}),
            )
            .await;
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "agent_runtime_read_failed"
        );
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), fail_at + 1);
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["state"], "partial");
        assert_eq!(evidence["reason_code"], "agent_runtime_read_failed");
        assert_eq!(
            evidence["request_sha256"],
            join_hashes(
                &observed[0].request_body_sha256,
                &observed[5].request_body_sha256
            )
        );
        assert_eq!(
            evidence["response_sha256"],
            join_hashes(&sha256_hex(&company_bytes), &sha256_hex(&catalogue_bytes))
        );
        assert_eq!(
            evidence["bytes"],
            company_bytes.len() * 2 + catalogue_bytes.len() * if fail_at < 8 { 1 } else { 2 }
        );
        let history = server.call_tool("read_evidence", json!({"limit":1})).await;
        assert_eq!(
            history["structuredContent"]["result"]["records"][0],
            *evidence
        );
    }
}

#[tokio::test]
async fn paired_company_refusal_retains_completed_discovery_source() {
    for fail_at in [1, 2, 3] {
        let mut plans = import_cycle_plans()[..4].to_vec();
        let body = response_bytes(&plans[0]);
        // A non-transient HTTP refusal isolates one admitted paired attempt.
        plans[fail_at].http_status = 400;
        plans.truncate(fail_at + 1);
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server_for(simulator.address(), directory.path());
        let response = server.call_tool("list_companies", json!({})).await;
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "company_collection_invalid"
        );
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), fail_at + 1);
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["state"], "partial");
        assert_eq!(evidence["reason_code"], "company_collection_invalid");
        assert_eq!(evidence["request_sha256"], observed[0].request_body_sha256);
        assert_eq!(evidence["response_sha256"], sha256_hex(&body));
        assert_eq!(
            evidence["bytes"],
            body.len() * if fail_at < 3 { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn company_retries_commit_only_the_terminal_attempt() {
    let cycle = import_cycle_plans();
    let first = cycle[0].clone();
    let mut last = first.clone();
    let original = last.fixture.body().into_owned();
    let changed = original.replace("WR2 Unicode Lab", "WR2 Changed Lab");
    assert_ne!(original, changed);
    last.fixture = Fixture::SyntheticXml(changed);
    let expected = response_bytes(&last);
    let mut failure = cycle[1].clone();
    failure.http_status = 503;
    let simulator = SequenceSimulator::spawn(vec![
        first.clone(),
        failure.clone(),
        first,
        failure.clone(),
        last,
        failure,
    ])
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_for(simulator.address(), directory.path());
    let response = server.call_tool("list_companies", json!({})).await;
    assert_eq!(response["isError"], true);
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"],
        "company_collection_invalid"
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(
        observed.len(),
        6,
        "three native read attempts, each followed by a failed health check"
    );
    let evidence = &response["structuredContent"]["evidence"];
    assert_eq!(evidence["state"], "partial");
    assert_eq!(evidence["request_sha256"], observed[4].request_body_sha256);
    assert_eq!(evidence["response_sha256"], sha256_hex(&expected));
    assert_eq!(
        evidence["bytes"],
        expected.len(),
        "terminal-attempt source commitment, not a traffic counter"
    );
}
