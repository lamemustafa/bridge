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
        let expected_response = sha256_hex(
            format!(
                "{}:{}",
                sha256_hex(&company_bytes),
                sha256_hex(&catalogue_bytes)
            )
            .as_bytes(),
        );
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
        let content = &response.value["structuredContent"];
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
        assert_eq!(
            content["evidence"]["bytes"],
            2 * (company_bytes.len() + catalogue_bytes.len())
        );
        {
            let records = server.evidence.lock().unwrap();
            let recorded = records.last().unwrap();
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
            10,
            "admission failure sends no extra requests"
        );
        assert_eq!(
            content["evidence"]["request_sha256"],
            sha256_hex(
                format!(
                    "{}:{}",
                    observed[0].request_body_sha256, observed[5].request_body_sha256
                )
                .as_bytes()
            )
        );
        assert!(!directory.path().join("imports").exists());
    }
}
