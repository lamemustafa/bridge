//! Error-path regressions retain commitments from successful source reads.
use super::*;
use crate::agent::{Redaction, Settings};

fn response_bytes(plan: &ScenarioPlan) -> Vec<u8> {
    tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding)
}

#[tokio::test]
async fn import_post_read_failures_retain_source_evidence_and_admission_errors_stay_empty() {
    for malformed_catalogue in [false, true] {
        let mut plans = import_cycle_plans()[..10].to_vec();
        if malformed_catalogue {
            // Remove one required attribute from the captured native catalogue.
            // This is fault injection, not a claimed additional Tally fixture.
            let source = plans[5].fixture.body().into_owned();
            let invalid = source.replacen("<LEDGER NAME=", "<LEDGER MISSINGNAME=", 1);
            assert_ne!(invalid, source);
            for index in [5, 7] {
                plans[index].fixture = Fixture::SyntheticXml(invalid.clone());
            }
        }
        let company_bytes = response_bytes(&plans[0]);
        let catalogue_bytes = response_bytes(&plans[5]);
        let mut expected_response = sha256_hex(
            format!(
                "{}:{}",
                sha256_hex(&company_bytes),
                sha256_hex(&catalogue_bytes)
            )
            .as_bytes(),
        );
        let mut expected_bytes = 2 * (company_bytes.len() + catalogue_bytes.len());
        if !malformed_catalogue {
            let probe = mode_tests::licensed_import_probe();
            let probe_response = sha256_hex(
                format!(
                    "{}:{}",
                    sha256_hex(&response_bytes(&probe[0])),
                    sha256_hex(&response_bytes(&probe[1]))
                )
                .as_bytes(),
            );
            let identity_response =
                sha256_hex(format!("{probe_response}:{}", sha256_hex(&company_bytes)).as_bytes());
            expected_response = sha256_hex(
                format!("{identity_response}:{}", sha256_hex(&catalogue_bytes)).as_bytes(),
            );
            expected_bytes += response_bytes(&probe[0]).len() + response_bytes(&probe[1]).len();
            plans = [probe, plans].concat();
        }
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: true,
            writes_enabled: false,
        });
        let response = if malformed_catalogue {
            server
                .call_tool_response(
                    "validate_masters",
                    json!({"company_guid":CAPTURED_GUID,"ledgers":["Cash"]}),
                )
                .await
        } else {
            let mut input = captured_catalogue_payload();
            for voucher in &mut input.vouchers {
                voucher.date = "20200101".into();
            }
            server
                .call_tool_response("build_import_xml", serde_json::to_value(input).unwrap())
                .await
        };
        let content = response.value["structuredContent"].clone();
        let code = if malformed_catalogue {
            "ledger_export_invalid"
        } else {
            "voucher_date_outside_company_extent"
        };
        assert_eq!(response.value["isError"], true);
        assert_eq!(content["result"]["error"]["code"], code);
        assert_eq!(content["evidence"]["state"], "partial");
        assert_eq!(content["evidence"]["reason_code"], code);
        assert_eq!(content["evidence"]["response_sha256"], expected_response);
        assert_eq!(content["evidence"]["bytes"], expected_bytes);
        let mut wire = Vec::new();
        crate::agent::agent_protocol::finish_response(
            &server,
            &mut wire,
            json!(1),
            Ok(response.value),
            Some(response.egress),
            response.recovery_batch_id,
            true,
        )
        .await
        .unwrap();
        {
            let records = server.evidence.lock().unwrap();
            let recorded = records.records.last().unwrap();
            assert_eq!(recorded.response_sha256, expected_response);
            assert_eq!(recorded.reason_code.as_deref(), Some(code));
        }
        let refused = server
            .call_tool_response(
                "validate_masters",
                json!({"company_guid":CAPTURED_GUID,"ledgers":[]}),
            )
            .await;
        assert_eq!(refused.value["isError"], true);
        assert_eq!(refused.value["structuredContent"]["evidence"]["bytes"], 0);
        let observed = simulator.finish().unwrap();
        assert_eq!(
            observed.len(),
            if malformed_catalogue { 10 } else { 12 },
            "admission failure sends no extra requests"
        );
        let join = |a: &str, b: &str| sha256_hex(format!("{a}:{b}").as_bytes());
        let req = |i: usize| observed[i].request_body_sha256.as_str();
        let expected_request = if malformed_catalogue {
            join(req(0), req(5))
        } else {
            join(&join(&join(req(0), req(1)), req(2)), req(7))
        };
        assert_eq!(content["evidence"]["request_sha256"], expected_request);
        assert!(!directory.path().join("imports").exists());
    }
}

#[tokio::test]
async fn a_name_the_core_refuses_costs_no_live_read() {
    // The tool schema and the binding core do not admit the same names: a
    // control character satisfies the schema's length and count rules and is
    // refused by `SourceEntity::new`. Parsing after the company verification
    // and the catalogue read meant the refusal arrived only after two live
    // round trips against the operator's books, and after evidence of them had
    // been retained — for a request that could not have succeeded against any
    // catalogue.
    //
    // The control is the endpoint itself: nothing is listening on it. A reach
    // for Tally before the request is parsed cannot come back as
    // `master_name_unsafe`, it comes back as a failure to connect — so the
    // reason code is the proof that no read was attempted, not a restatement
    // of the assertion.
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            // Port 1 is privileged and unbound here; any connection is refused.
            port: 1,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: true,
        writes_enabled: false,
    });

    let refused = server
        .call_tool_response(
            "validate_masters",
            json!({"company_guid": CAPTURED_GUID, "ledgers": ["Cash\u{7}"]}),
        )
        .await;
    assert_eq!(refused.value["isError"], true);
    let content: Value =
        serde_json::from_str(refused.value["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(content["result"]["error"]["code"], "master_name_unsafe");
    // Nothing was read, so there is nothing to commit to.
    assert_eq!(refused.value["structuredContent"]["evidence"]["bytes"], 0);
}
