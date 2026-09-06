//! Controlled replay of captured bodies; altered metadata is refusal testing,
//! not evidence that Education mode actually returns an empty literal-date read.
use super::*;

#[tokio::test]
async fn verification_qualifies_absence_without_hiding_positive_historical_rows() {
    let captured = boundary_tests::captured_vouchers();
    let row = parse_import_vouchers(&captured, CAPTURED_GUID)
        .unwrap()
        .rows
        .remove(0);
    // Preserve captured accounting fields; change the type and first narration
    // only to correlate a synthetic historical batch with this captured row.
    let positive = captured
        .replace(
            "<VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>",
            "<VOUCHERTYPENAME>Journal</VOUCHERTYPENAME>",
        )
        .replacen(
            row.narration.as_deref().unwrap(),
            "[BRIDGE:mode-observed]",
            1,
        );
    let expected = ImportVoucher {
        bridge_txn_id: "mode-observed".into(),
        date: row.date.clone().unwrap(),
        voucher_type: VoucherType::Journal,
        narration: None,
        reference: None,
        voucher_number: row.voucher_number.clone(),
        entries: row
            .entries
            .iter()
            .map(|entry| ImportEntry {
                ledger: entry.ledger.clone(),
                amount: entry.amount.trim_start_matches('-').into(),
                side: if entry.is_deemed_positive == "Yes" {
                    EntrySide::Dr
                } else {
                    EntrySide::Cr
                },
            })
            .collect(),
    };
    let empty_bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    );
    let empty = String::from_utf16(
        &empty_bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let mut cases = vec![
        ("education_positive", "education", "education", true, false),
        ("education_empty", "education", "education", false, false),
        ("education_mixed", "education", "education", true, true),
        ("licensed_empty", "none", "none", false, false),
        ("mode_changed", "none", "education", false, false),
        ("closing_http_failure", "none", "education", false, false),
        (
            "closing_all_http_failure",
            "none",
            "education",
            false,
            false,
        ),
        (
            "unknown_profile_positive",
            "release_and_tier_unknown",
            "none",
            true,
            false,
        ),
    ];
    for fault in [
        "release_missing",
        "release_unknown",
        "license_gold",
        "license_ambiguous",
    ] {
        cases.extend([
            (fault, fault, "none", true, false),
            (fault, fault, "none", false, false),
            (fault, fault, "none", true, true),
            (fault, "none", fault, false, false),
            (fault, "none", fault, true, true),
        ]);
    }
    for (case, opening_fault, closing_fault, has_rows, missing_expected) in cases {
        let cycle = import_cycle_plans();
        let mut reads = cycle[16..].to_vec();
        for index in [5, 7, 11, 13] {
            reads[index].fixture = Fixture::SyntheticXml(if has_rows {
                positive.clone()
            } else {
                empty.clone()
            });
        }
        let negative = !has_rows || missing_expected;
        let close = negative && opening_fault == "none";
        let closing_http_failure =
            matches!(case, "closing_http_failure" | "closing_all_http_failure");
        let mut closing = if close {
            mode_tests::import_profile_probe(closing_fault)
        } else {
            vec![]
        };
        if closing_http_failure {
            closing.last_mut().unwrap().http_status = 503;
            if case == "closing_all_http_failure" {
                closing[0].http_status = 503;
            }
        }
        let plans = [
            mode_tests::import_profile_probe(opening_fault),
            reads,
            closing,
        ]
        .concat();
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
        let company =
            bridge_tally_protocol::parse_companies_from_collection(&cycle[0].fixture.body())
                .unwrap()
                .remove(0);
        let mut vouchers = vec![expected.clone()];
        if missing_expected {
            let mut missing = expected.clone();
            missing.bridge_txn_id = "mode-missing".into();
            missing.entries[0].ledger = "Unobserved test ledger".into();
            vouchers.push(missing);
        }
        let line = ImportLedgerLine {
            identity_scheme: None,
            batch_id: "mode-history".into(),
            company_guid: CAPTURED_GUID.into(),
            company: Some(import_company_tuple(&company).unwrap()),
            txn_ids: vouchers.iter().map(|v| v.bridge_txn_id.clone()).collect(),
            date_from: row.date.clone().unwrap(),
            date_to: row.date.clone().unwrap(),
            sha256: "synthetic-history".into(),
            built_at: now(),
            status: "built".into(),
            pre_import_mark: PreImportMark {
                kind: "company_high_water".into(),
                value: Some(0),
                master_value: Some(0),
            },
            vouchers,
        };
        server.append_import_ledger(&line).unwrap();
        let generation = server
            .latest_import_snapshot(&line.batch_id)
            .unwrap()
            .unwrap()
            .generation;
        server
            .persist_import_verification(&json!({"batch_id":line.batch_id}), &line, generation)
            .unwrap();
        let paths = [
            server.settings.data_dir.join("agent-import-ledger.jsonl"),
            server
                .imports_dir()
                .unwrap()
                .join("mode-history.proof.json"),
            server.imports_dir().unwrap().join("mode-history.proof.md"),
        ];
        let before = paths
            .iter()
            .map(fs::read)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let response = server
            .call_tool_response(
                "verify_import",
                json!({"company_guid":CAPTURED_GUID,"batch_id":line.batch_id}),
            )
            .await;
        let content = &response.value["structuredContent"];
        let refused = negative && (opening_fault != "none" || closing_fault != "none");
        let refusal_fault = if opening_fault != "none" {
            opening_fault
        } else {
            closing_fault
        };
        assert_eq!(response.value["isError"], refused, "{case}: {content}");
        if refused {
            assert_eq!(
                content["result"]["error"]["code"],
                if closing_http_failure {
                    "import_mode_probe_failed"
                } else if refusal_fault.starts_with("release_") {
                    "verification_release_unqualified"
                } else if refusal_fault.starts_with("license_") {
                    "verification_license_tier_unqualified"
                } else {
                    "verification_mode_unqualified"
                },
                "{case}"
            );
            assert_eq!(content["evidence"]["state"], "partial");
            let after = paths
                .iter()
                .map(fs::read)
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(
                before, after,
                "refusal preserves prior proof and status: {case}"
            );
        } else {
            assert_eq!(
                content["result"]["counts"][if has_rows {
                    "posted_verified"
                } else {
                    "not_found"
                }],
                1,
                "{case}"
            );
        }
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), if close { 20 } else { 18 }, "{case}");
        let join = |a: &str, b: &str| sha256_hex(format!("{a}:{b}").as_bytes());
        let mut request = join(
            &observed[0].request_body_sha256,
            &observed[1].request_body_sha256,
        );
        let mut response = join(&sha256_hex(&responses[0]), &sha256_hex(&responses[1]));
        let mut bytes = responses[0].len() + responses[1].len();
        for i in [2, 7, 13] {
            request = join(&request, &observed[i].request_body_sha256);
            response = join(&response, &sha256_hex(&responses[i]));
            bytes += 2 * responses[i].len();
        }
        if close && !closing_http_failure {
            request = join(
                &request,
                &join(
                    &observed[18].request_body_sha256,
                    &observed[19].request_body_sha256,
                ),
            );
            response = join(
                &response,
                &join(&sha256_hex(&responses[18]), &sha256_hex(&responses[19])),
            );
            bytes += responses[18].len() + responses[19].len();
        } else if case == "closing_http_failure" {
            // The closing GET completed before the failing company POST. Keep
            // exactly that source, without counting the rejected POST body.
            request = join(&request, &observed[18].request_body_sha256);
            response = join(&response, &sha256_hex(&responses[18]));
            bytes += responses[18].len();
        }
        assert_eq!(content["evidence"]["request_sha256"], request, "{case}");
        assert_eq!(content["evidence"]["response_sha256"], response, "{case}");
        assert_eq!(content["evidence"]["bytes"], bytes, "{case}");
    }
}
