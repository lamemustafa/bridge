//! Captured readback sources qualify only the current build window.
use super::*;

fn captured_xml(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[tokio::test]
async fn build_preflight_refuses_unreadable_or_out_of_window_sources_before_files() {
    let captured = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let source = captured_xml(captured);
    let parsed = parse_import_vouchers(&source, CAPTURED_GUID).unwrap();
    assert_eq!(parsed.rows.len(), 3);
    assert!(parsed
        .rows
        .iter()
        .all(|row| row.date.as_deref() == Some("20260801")));
    for fault in [
        "out_of_window",
        "invalid_amount",
        "missing_alter_id",
        "http",
    ] {
        let mut input = captured_catalogue_payload();
        if fault == "missing_alter_id" {
            for voucher in &mut input.vouchers {
                voucher.date = "2026-08-01".into();
            }
        }
        let mut plans = qualified_import_cycle_plans()[..30].to_vec();
        if fault == "http" {
            plans[25].http_status = 503;
            plans.truncate(26);
        } else {
            // Refusal-only scalar fault injection into an otherwise unchanged capture.
            let body = if fault == "invalid_amount" {
                let start = source.find("<AMOUNT TYPE=\"Amount\">").unwrap()
                    + "<AMOUNT TYPE=\"Amount\">".len();
                let end = start + source[start..].find("</AMOUNT>").unwrap();
                format!("{}invalid{}", &source[..start], &source[end..])
            } else if fault == "missing_alter_id" {
                let mut body = source.clone();
                for id in [1, 2, 3] {
                    let tag = format!("<ALTERID TYPE=\"Number\"> {id}</ALTERID>");
                    assert_eq!(body.matches(&tag).count(), 1);
                    body = body.replace(&tag, "");
                }
                let rows = parse_import_vouchers(&body, CAPTURED_GUID).unwrap();
                assert_eq!(rows.rows.len(), 3);
                assert!(rows.rows.iter().all(|row| row.alter_id.is_none()));
                assert!(rows
                    .rows
                    .iter()
                    .all(|row| row.date.as_deref() == Some("20260801")));
                body
            } else {
                source.clone()
            };
            for index in [25, 27] {
                plans[index].fixture = Fixture::SyntheticXml(body.clone());
            }
        }
        let responses = plans
            .iter()
            .map(|p| tally_protocol_simulator::encode(&p.fixture.body(), p.encoding))
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
        let error = server
            .build_import_xml(&serde_json::to_value(input).unwrap())
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.code,
            match fault {
                "out_of_window" => "window_not_honoured",
                "invalid_amount" => "import_verification_amount_invalid",
                "http" => "agent_runtime_read_failed",
                "missing_alter_id" => "verification_incomplete:window_not_corroborated",
                _ => unreachable!(),
            },
            "{fault}"
        );
        assert!(!directory.path().join("imports").exists(), "{fault}");
        assert!(
            !directory.path().join("agent-import-ledger.jsonl").exists(),
            "{fault}"
        );
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), if fault == "http" { 26 } else { 30 });
        let join = |a: &str, b: &str| sha256_hex(format!("{a}:{b}").as_bytes());
        let mut request = join(
            &observed[0].request_body_sha256,
            &observed[1].request_body_sha256,
        );
        let mut response = join(&sha256_hex(&responses[0]), &sha256_hex(&responses[1]));
        let mut bytes = responses[0].len() + responses[1].len();
        for index in [2, 7, 13, 19]
            .into_iter()
            .chain((fault != "http").then_some(25))
        {
            request = join(&request, &observed[index].request_body_sha256);
            response = join(&response, &sha256_hex(&responses[index]));
            bytes += 2 * responses[index].len();
        }
        let evidence = error.evidence.unwrap();
        assert_eq!(evidence.request_sha256, request, "{fault}");
        assert_eq!(evidence.response_sha256, response, "{fault}");
        assert_eq!(evidence.bytes, bytes, "{fault}");
    }
}
