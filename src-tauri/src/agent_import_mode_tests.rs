//! Captured licensed-mode replay and refusal-only metadata fault injection.
use super::*;

pub(super) fn licensed_import_probe() -> Vec<ScenarioPlan> {
    let bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml");
    let xml = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    vec![
        ScenarioPlan::new(Fixture::ProductStatus(
            tally_protocol_simulator::ProductStatus::TallyPrime,
        ))
        .with_framing(ResponseFraming::ContentLength),
        ScenarioPlan::new(Fixture::SyntheticXml(xml))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength),
    ]
}

#[tokio::test]
async fn import_build_requires_qualified_mode_bracket_and_retains_probe_evidence() {
    let licensed = licensed_import_probe();
    for fault in [
        "education",
        "unknown",
        "product",
        "editlog",
        "unknown_product",
        "closing",
        "none",
    ] {
        let mut invalid = licensed.clone();
        let xml = invalid[1].fixture.body().into_owned();
        let damaged = match fault {
            "education" | "closing" => xml.replace(
                "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
                "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
            ),
            "unknown" => xml.replace(
                "<SILVER TYPE=\"Logical\">Yes</SILVER>",
                "<SILVER TYPE=\"Logical\">No</SILVER>",
            ),
            "product" => xml.replace(
                "<PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME>",
                "<PRODUCTNAME TYPE=\"String\">Tally ERP 9</PRODUCTNAME>",
            ),
            "editlog" => xml.replace(
                "<PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME>",
                "<PRODUCTNAME TYPE=\"String\">TallyPrime Edit Log</PRODUCTNAME>",
            ),
            "unknown_product" => xml.replace(
                "<PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME>",
                "<PRODUCTNAME TYPE=\"String\">UnknownProduct</PRODUCTNAME>",
            ),
            _ => xml.clone(),
        };
        if fault != "none" {
            assert_ne!(damaged, xml);
        }
        invalid[1].fixture = Fixture::SyntheticXml(damaged);
        let plans = if matches!(fault, "closing" | "none") {
            [
                licensed.clone(),
                import_cycle_plans()[..16].to_vec(),
                import_cycle_plans()[4..10].to_vec(),
                invalid,
            ]
            .concat()
        } else {
            invalid
        };
        let responses = plans
            .iter()
            .map(|plan| tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(super::super::super::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: super::super::super::Redaction::None,
            import_enabled: true,
        });
        let result = server
            .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).unwrap())
            .await;
        let evidence = if fault == "none" {
            let result = result.unwrap();
            assert_eq!(
                result.payload["result"]["live_evidence"],
                "synthetic_lab_readback"
            );
            let batch = result.payload["result"]["batch_id"].as_str().unwrap();
            assert!(directory
                .path()
                .join("imports")
                .join(format!("{batch}.xml"))
                .is_file());
            result.evidence
        } else {
            let error = result.err().unwrap();
            assert_eq!(error.code, "import_mode_unqualified", "{fault}");
            assert!(!directory.path().join("imports").exists());
            assert!(!directory.path().join("agent-import-ledger.jsonl").exists());
            *error.evidence.unwrap()
        };
        let observed = simulator.finish().unwrap();
        let join = |a: &str, b: &str| sha256_hex(format!("{a}:{b}").as_bytes());
        let request = |i: usize| observed[i].request_body_sha256.clone();
        let response = |i: usize| sha256_hex(&responses[i]);
        let mut request_hash = join(&request(0), &request(1));
        let mut response_hash = join(&response(0), &response(1));
        let mut bytes = responses[0].len() + responses[1].len();
        if matches!(fault, "closing" | "none") {
            assert_eq!(observed.len(), 26);
            for i in [2, 7, 13, 19] {
                request_hash = join(&request_hash, &request(i));
                response_hash = join(&response_hash, &response(i));
                bytes += 2 * responses[i].len();
            }
            request_hash = join(&request_hash, &join(&request(24), &request(25)));
            response_hash = join(&response_hash, &join(&response(24), &response(25)));
            bytes += responses[24].len() + responses[25].len();
        } else {
            assert_eq!(observed.len(), 2);
        }
        assert_eq!(evidence.request_sha256, request_hash, "{fault}");
        assert_eq!(evidence.response_sha256, response_hash, "{fault}");
        assert_eq!(evidence.bytes, bytes, "{fault}");
    }
}

#[tokio::test]
async fn import_build_rechecks_captured_catalogue_before_persistence() {
    for fault in ["none", "rename", "identity", "parent", "delete"] {
        let mut plans = qualified_import_cycle_plans()[..26].to_vec();
        if fault != "none" {
            for index in [19, 21] {
                let xml = plans[index].fixture.body().into_owned();
                let changed = match fault {
                    "rename" => xml.replace("Bridge Nested Debtor WR4", "Renamed Debtor"),
                    "identity" => xml.replace(
                        "61c6de69-1748-461c-ad3f-162cb949df9f-000000d5",
                        "61c6de69-1748-461c-ad3f-162cb949df9f-000000ff",
                    ),
                    "parent" => xml.replace("Bridge Nested Debtors WR4", "Changed Parent"),
                    "delete" => {
                        let start = xml
                            .find("<LEDGER NAME=\"Bridge Nested Debtor WR4\"")
                            .unwrap();
                        let end =
                            start + xml[start..].find("</LEDGER>").unwrap() + "</LEDGER>".len();
                        format!("{}{}", &xml[..start], &xml[end..])
                    }
                    _ => unreachable!(),
                };
                assert_ne!(changed, xml);
                plans[index].fixture = Fixture::SyntheticXml(changed);
            }
            plans.truncate(24);
        }
        let responses = plans
            .iter()
            .map(|plan| tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding))
            .collect::<Vec<_>>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(crate::agent::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().into(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: crate::agent::Redaction::None,
            import_enabled: true,
        });
        let result = server
            .build_import_xml(&serde_json::to_value(captured_catalogue_payload()).unwrap())
            .await;
        let evidence = if fault == "none" {
            result.unwrap().evidence
        } else {
            let error = result.err().unwrap();
            assert_eq!(error.code, "import_catalogue_changed", "{fault}");
            assert!(!directory.path().join("imports").exists());
            assert!(!directory.path().join("agent-import-ledger.jsonl").exists());
            *error.evidence.unwrap()
        };
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), if fault == "none" { 26 } else { 24 });
        let join = |a: &str, b: &str| sha256_hex(format!("{a}:{b}").as_bytes());
        let mut request = join(
            &observed[0].request_body_sha256,
            &observed[1].request_body_sha256,
        );
        let mut response = join(&sha256_hex(&responses[0]), &sha256_hex(&responses[1]));
        let mut bytes = responses[0].len() + responses[1].len();
        for i in [2, 7, 13, 19] {
            request = join(&request, &observed[i].request_body_sha256);
            response = join(&response, &sha256_hex(&responses[i]));
            bytes += 2 * responses[i].len();
        }
        assert_eq!(
            observed[7].request_body_sha256,
            observed[19].request_body_sha256
        );
        if fault == "none" {
            request = join(
                &request,
                &join(
                    &observed[24].request_body_sha256,
                    &observed[25].request_body_sha256,
                ),
            );
            response = join(
                &response,
                &join(&sha256_hex(&responses[24]), &sha256_hex(&responses[25])),
            );
            bytes += responses[24].len() + responses[25].len();
        }
        assert_eq!(evidence.request_sha256, request);
        assert_eq!(evidence.response_sha256, response);
        assert_eq!(evidence.bytes, bytes);
    }
}
