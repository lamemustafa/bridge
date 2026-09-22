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
        (
            "<GUID>61c6de69-1748-461c-ad3f-162cb949df9f-00000001</GUID>",
            "<GUID>71c6de69-1748-461c-ad3f-162cb949df9f-00000001</GUID>",
            "voucher_company_identity_invalid",
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
        // The pre-flight high-water read (protocol reference §11c): ten
        // vouchers cannot exceed the budget, so the window is read whole.
        plans.extend(cycle[10..16].iter().cloned());
        plans.extend([
            cycle[0].clone(),
            vouchers.clone(),
            cycle[1].clone(),
            vouchers.clone(),
            cycle[1].clone(),
            cycle[0].clone(),
        ]);
        let company_body = response_bytes(&plans[0]);
        let high_water_body = response_bytes(&plans[5]);
        let voucher_body = response_bytes(&plans[11]);
        // The window read's own evidence folds its pre-flight first.
        let expected_hash = join_hashes(
            &sha256_hex(&company_body),
            &join_hashes(&sha256_hex(&high_water_body), &sha256_hex(&voucher_body)),
        );
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
            2 * (company_body.len() + high_water_body.len() + voucher_body.len())
        );
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 16);
        assert_eq!(
            evidence["request_sha256"],
            join_hashes(
                &observed[0].request_body_sha256,
                &join_hashes(
                    &observed[5].request_body_sha256,
                    &observed[11].request_body_sha256
                )
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
        plans.extend(cycle[10..16].iter().cloned());
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
        // Company, catalogue, high water, window, repeated catalogue. The
        // window read folds its own pre-flight before it joins the chain.
        let source_bytes = [0, 5, 11, 17, 23].map(|index| response_bytes(&plans[index]));
        let hashes = source_bytes.each_ref().map(|body| sha256_hex(body));
        let expected_hash = join_hashes(
            &join_hashes(
                &join_hashes(&hashes[0], &hashes[1]),
                &join_hashes(&hashes[2], &hashes[3]),
            ),
            &hashes[4],
        );
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
        assert_eq!(observed.len(), 28);
        let requests = [0, 5, 11, 17, 23].map(|index| observed[index].request_body_sha256.clone());
        let request_hash = join_hashes(
            &join_hashes(
                &join_hashes(&requests[0], &requests[1]),
                &join_hashes(&requests[2], &requests[3]),
            ),
            &requests[4],
        );
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
        parse_agent_rows(&populated.fixture.body(), CAPTURED_GUID)
            .unwrap()
            .len(),
        3
    );
    assert!(parse_agent_rows(&empty.fixture.body(), CAPTURED_GUID)
        .unwrap()
        .is_empty());
    for source_is_empty in [false, true] {
        let cycle = import_cycle_plans();
        let mut plans = cycle[..10].to_vec();
        // The pre-flight high-water read before the window. The widened read
        // that corroborates an empty window reuses that mark, and ten vouchers
        // keep both windows whole.
        plans.extend(cycle[10..16].iter().cloned());
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
        assert_eq!(simulator.finish().unwrap().len(), 28);
    }
}

/// #595, through the tool call: a `vouchers` result carries what each request
/// of its window read cost, and so does a refusal of that read.
#[tokio::test]
async fn the_vouchers_tool_reports_its_window_timings_on_success_and_refusal() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let vouchers = ScenarioPlan::new(Fixture::SyntheticXml(String::from_utf16(&words).unwrap()))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength);
    let call = |plans: Vec<ScenarioPlan>| async move {
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let response = server_for(simulator.address(), directory.path())
            .call_tool(
                "vouchers",
                json!({"company_guid":CAPTURED_GUID, "from":"20260801","to":"20260802"}),
            )
            .await;
        simulator.cancel();
        simulator.finish().unwrap();
        response
    };

    // Ten vouchers at the mark: the window is read whole, in one part.
    let cycle = import_cycle_plans();
    let mut plans = cycle[..4].to_vec();
    plans.extend(cycle[10..16].iter().cloned());
    plans.extend([
        cycle[0].clone(),
        vouchers.clone(),
        cycle[1].clone(),
        vouchers.clone(),
        cycle[1].clone(),
        cycle[0].clone(),
    ]);
    let body = response_bytes(&plans[11]);
    let response = call(plans).await;
    assert_eq!(response["isError"], false, "{response}");
    let window = &response["structuredContent"]["result"]["window"];
    assert_eq!(window["marks"]["requests"], 1);
    assert_eq!(window["census"]["requests"], 0);
    let parts = window["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["from"], "20260801");
    assert_eq!(parts[0]["to"], "20260802");
    assert_eq!(parts[0]["served"], true);
    assert_eq!(parts[0]["rows"], 3);
    assert_eq!(parts[0]["bytes"], body.len());
    assert!(parts[0]["ms"].is_u64());
    assert!(window.get("failed").is_none());

    // A mark far over the budget needs a census, and Tally declares the census
    // response over the transport cap: the refusal carries the timings too.
    let heavy = format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY>\
         <GUID>{CAPTURED_GUID}</GUID><ALTVCHID>5000</ALTVCHID><ALTMSTID>7</ALTMSTID>\
         </COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let heavy = ScenarioPlan::new(Fixture::SyntheticXml(heavy))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength);
    let mut plans = cycle[..4].to_vec();
    plans.extend(cycle[10..16].iter().cloned());
    plans[5] = heavy.clone();
    plans[7] = heavy;
    plans.extend([
        cycle[0].clone(),
        vouchers.with_framing(ResponseFraming::DeclaredContentLength {
            bytes: bridge_tally_transport::XML_RESPONSE_MAX_BYTES + 1,
        }),
    ]);
    let response = call(plans).await;
    assert_eq!(response["isError"], true, "{response}");
    let error = &response["structuredContent"]["result"]["error"];
    assert_eq!(error["code"], crate::agent::VOLUME_UNESTIMATED);
    let window = &error["window"];
    assert_eq!(window["marks"]["requests"], 1);
    assert_eq!(window["census"]["requests"], 1);
    assert_eq!(window["parts"], json!([]));
    assert_eq!(window["failed"]["kind"], "census");
    assert!(window["failed"]["ms"].is_u64());
}

/// #595: the window timings are the `vouchers` tool's alone. Another tool
/// whose window read is refused the same way keeps its refusal shape.
#[tokio::test]
async fn only_the_vouchers_tool_reports_window_timings_on_a_refusal() {
    let heavy = format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY>\
         <GUID>{CAPTURED_GUID}</GUID><ALTVCHID>5000</ALTVCHID><ALTMSTID>7</ALTMSTID>\
         </COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"
    );
    let heavy = ScenarioPlan::new(Fixture::SyntheticXml(heavy))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength);
    let cycle = import_cycle_plans();
    // The company, the ledger catalogue, the heavy mark, then a census Tally
    // declares over the transport cap.
    let mut plans = cycle[..10].to_vec();
    plans.extend(cycle[10..16].iter().cloned());
    plans[11] = heavy.clone();
    plans[13] = heavy;
    plans.extend([
        cycle[0].clone(),
        cycle[11]
            .clone()
            .with_framing(ResponseFraming::DeclaredContentLength {
                bytes: bridge_tally_transport::XML_RESPONSE_MAX_BYTES + 1,
            }),
    ]);
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let response = server_for(simulator.address(), directory.path())
        .call_tool(
            "voucher_presence",
            json!({
                "company_guid": CAPTURED_GUID,
                "from": "20260801",
                "to": "20260802",
                "numbering": [{"voucher_type": "Journal", "numbering_method": "manual"}],
                "vouchers": [{
                    "date": "20260801", "voucher_type": "Journal", "voucher_number": "JV-1",
                    "party": "Cash",
                    "entries": [{"ledger": "Cash", "amount": "-12.50"},
                                {"ledger": "WR2 Sales", "amount": "12.50"}],
                }],
            }),
        )
        .await;
    simulator.cancel();
    simulator.finish().unwrap();
    let error = &response["structuredContent"]["result"]["error"];
    // The same refusal the `vouchers` test reaches, so the window read ran.
    assert_eq!(
        error["code"],
        crate::agent::VOLUME_UNESTIMATED,
        "{response}"
    );
    assert!(error.get("window").is_none(), "{error}");
}

/// #595: an empty requested window is corroborated by a wider read of its
/// own. When that read is refused, the timings reported are the requested
/// window's, which was read in full, not the wider window's.
#[tokio::test]
async fn a_refused_corroboration_reports_the_requested_windows_timings() {
    let decode = |bytes: &[u8]| {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let plan = |xml: String| {
        ScenarioPlan::new(Fixture::SyntheticXml(xml))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    };
    let empty = plan(decode(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    )));
    // Dated outside the wider window too, so the corroboration is refused.
    let outside = plan(
        decode(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
        ))
        .replace("20260801", "20261001"),
    );
    let cycle = import_cycle_plans();
    let mut plans = cycle[..4].to_vec();
    plans.extend(cycle[10..16].iter().cloned());
    for body in [&empty, &outside] {
        plans.extend([
            cycle[0].clone(),
            body.clone(),
            cycle[1].clone(),
            body.clone(),
            cycle[1].clone(),
            cycle[0].clone(),
        ]);
    }
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let response = server_for(simulator.address(), directory.path())
        .call_tool(
            "vouchers",
            json!({"company_guid":CAPTURED_GUID, "from":"20260801","to":"20260802"}),
        )
        .await;
    simulator.cancel();
    simulator.finish().unwrap();
    assert_eq!(response["isError"], true, "{response}");
    let window = &response["structuredContent"]["result"]["error"]["window"];
    let parts = window["parts"]
        .as_array()
        .expect("the requested window's parts");
    assert_eq!(parts.len(), 1, "{window}");
    assert_eq!(parts[0]["from"], "20260801");
    assert_eq!(parts[0]["to"], "20260802");
    assert_eq!(parts[0]["rows"], 0);
    assert!(window.get("failed").is_none(), "{window}");
}
