//! Captured-source regression cases for selected-ledger reads.
use super::*;

#[tokio::test]
async fn voucher_boundary_refusals_retain_exact_source_commitments() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    for (from, to, code) in [
        (
            "<AMOUNT TYPE=\"Amount\">-101.01</AMOUNT>",
            "<AMOUNT TYPE=\"Amount\">Maybe</AMOUNT>",
            "voucher_amount_invalid",
        ),
        (
            "\n      <ISDEEMEDPOSITIVE TYPE=\"Logical\">Yes</ISDEEMEDPOSITIVE>",
            "\n      <ISDEEMEDPOSITIVE TYPE=\"Logical\">Maybe</ISDEEMEDPOSITIVE>",
            "voucher_accounting_state_not_observed",
        ),
        ("20260801", "2026080A", "voucher_date_invalid"),
        ("20260801", "20260803", "window_not_honoured"),
    ] {
        let damaged = captured.replacen(from, to, 1);
        assert_ne!(damaged, captured);
        let vouchers = ScenarioPlan::new(Fixture::SyntheticXml(damaged))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength);
        let cycle = import_cycle_plans();
        let mut plans = cycle[..4].to_vec();
        plans.extend([
            cycle[0].clone(),
            vouchers.clone(),
            cycle[1].clone(),
            vouchers.clone(),
            cycle[1].clone(),
            cycle[0].clone(),
        ]);
        let company_body = response_bytes(&plans[0]);
        let voucher_body = response_bytes(&plans[5]);
        let expected_hash = join_hashes(&sha256_hex(&company_body), &sha256_hex(&voucher_body));
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let response = server_for(simulator.address(), directory.path())
            .call_tool(
                "vouchers",
                json!({"company_guid":CAPTURED_GUID,
                "from":"20260801","to":"20260802"}),
            )
            .await;
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            code
        );
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["state"], "partial");
        assert_eq!(evidence["reason_code"], code);
        assert_eq!(evidence["response_sha256"], expected_hash);
        assert_eq!(
            evidence["bytes"],
            2 * (company_body.len() + voucher_body.len())
        );
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 10);
        assert_eq!(
            evidence["request_sha256"],
            join_hashes(
                &observed[0].request_body_sha256,
                &observed[5].request_body_sha256
            )
        );
    }
}

#[tokio::test]
async fn selected_ledger_rename_or_unknown_entry_refuses_complete_selection() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let captured = String::from_utf16(&words).unwrap();
    // Negative fault: the returned entries use a renamed ledger. Either the
    // second catalogue observes it or an intervening rename has reverted.
    let renamed = captured.replace("WR2 Sales", "WR2 Renamed Sales");
    assert_ne!(renamed, captured);
    let vouchers = ScenarioPlan::new(Fixture::SyntheticXml(renamed))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength);
    for catalogue_changed in [false, true] {
        let cycle = import_cycle_plans();
        let mut plans = cycle[..10].to_vec();
        plans.extend([
            cycle[0].clone(),
            vouchers.clone(),
            cycle[1].clone(),
            vouchers.clone(),
            cycle[1].clone(),
            cycle[0].clone(),
        ]);
        let mut after = cycle[4..10].to_vec();
        if catalogue_changed {
            for index in [1, 3] {
                let original = after[index].fixture.body();
                let altered = original.replace("WR2 Sales", "WR2 Renamed Sales");
                assert_ne!(altered, original);
                after[index].fixture = Fixture::SyntheticXml(altered);
            }
        }
        plans.extend(after);
        let source_bytes = [0, 5, 11, 17].map(|index| response_bytes(&plans[index]));
        let expected_hash = source_bytes
            .iter()
            .map(|body| sha256_hex(body))
            .reduce(|left, right| join_hashes(&left, &right))
            .unwrap();
        let expected_bytes = 2 * source_bytes.iter().map(Vec::len).sum::<usize>();
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let response = server_for(simulator.address(), directory.path())
            .call_tool(
                "vouchers",
                json!({"company_guid":CAPTURED_GUID,
                "from":"20260801","to":"20260802","ledger":"WR2 Sales"}),
            )
            .await;
        assert_eq!(response["isError"], true);
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "ledger_snapshot_drifted"
        );
        assert_eq!(
            response["structuredContent"]["evidence"]["state"],
            "partial"
        );
        let evidence = &response["structuredContent"]["evidence"];
        assert_eq!(evidence["response_sha256"], expected_hash);
        assert_eq!(evidence["bytes"], expected_bytes);
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 22);
        let request_hash = [0, 5, 11, 17]
            .into_iter()
            .map(|index| observed[index].request_body_sha256.clone())
            .reduce(|left, right| join_hashes(&left, &right))
            .unwrap();
        assert_eq!(evidence["request_sha256"], request_hash);
    }
}

#[tokio::test]
async fn empty_ledger_selection_does_not_replace_source_emptiness() {
    fn captured_plan(bytes: &[u8]) -> ScenarioPlan {
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        ScenarioPlan::new(Fixture::SyntheticXml(String::from_utf16(&words).unwrap()))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    }
    let populated = captured_plan(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    ));
    let empty = captured_plan(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    ));
    assert_eq!(
        parse_agent_rows(&populated.fixture.body()).unwrap().len(),
        3
    );
    assert!(parse_agent_rows(&empty.fixture.body()).unwrap().is_empty());
    for source_is_empty in [false, true] {
        let cycle = import_cycle_plans();
        let mut plans = cycle[..10].to_vec();
        let paired_read = |source: &ScenarioPlan| {
            [
                cycle[0].clone(),
                source.clone(),
                cycle[1].clone(),
                source.clone(),
                cycle[1].clone(),
                cycle[0].clone(),
            ]
        };
        plans.extend(paired_read(if source_is_empty {
            &empty
        } else {
            &populated
        }));
        if source_is_empty {
            // The same captured vouchers in the widened read contradict true
            // source emptiness, even though none touch the selected Cash ledger.
            plans.extend(paired_read(&populated));
        } else {
            plans.extend(cycle[4..10].iter().cloned());
        }
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let response = server_for(simulator.address(), directory.path())
            .call_tool(
                "vouchers",
                json!({"company_guid":CAPTURED_GUID,
                "from":"20260801","to":"20260802","ledger":"Cash"}),
            )
            .await;
        if source_is_empty {
            assert_eq!(response["isError"], true);
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"],
                "window_contradicted"
            );
        } else {
            assert_eq!(response["isError"], false);
            assert_eq!(response["structuredContent"]["result"]["state"], "complete");
            assert_eq!(response["structuredContent"]["result"]["items"], json!([]));
            assert_eq!(response["structuredContent"]["result"]["total"], 0);
            assert_eq!(
                response["structuredContent"]["evidence"]["state"],
                "complete"
            );
        }
        assert_eq!(simulator.finish().unwrap().len(), 22);
    }
}
