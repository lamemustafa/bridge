use super::egress::EGRESS_TAIL_CHUNK_BYTES;
use super::*;
use std::fs::OpenOptions;
use std::io::Write;
use tally_protocol_simulator::{
    Fixture, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};
use tokio::io::AsyncReadExt;

pub(super) fn company_collection_xml() -> String {
    "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"BRIDGE SYNTHETIC BOOK\"><GUID>00000000-0000-4000-8000-000000000001</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>".to_string()
}

fn voucher_collection_xml() -> String {
    "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERNUMBER>PV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>voucher-guid</GUID><ALLLEDGERENTRIES.LIST><LEDGERNAME>Expense</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>Bank</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>".to_string()
}

#[test]
fn movement_native_parser_rejects_missing_flags_and_out_of_window_cancelled_rows() {
    let xml = voucher_collection_xml();
    assert!(parse_movement_vouchers(&xml, "20260901", "20260902", "voucher").is_err());
    let cancelled = xml.replace(
        "<GUID>",
        "<ISCANCELLED>Yes</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><GUID>",
    );
    assert!(
        parse_movement_vouchers(&cancelled, "20260901", "20260902", "voucher")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        parse_movement_vouchers(&cancelled, "20260902", "20260902", "voucher")
            .err()
            .as_deref(),
        Some("window_not_honoured")
    );
}

#[test]
fn ordinary_vouchers_reject_missing_or_empty_core_fields_before_selection() {
    for (field, value) in [("DATE", "20260901"), ("VOUCHERTYPENAME", "Payment")] {
        for replacement in [
            String::new(),
            format!("<{field}></{field}>"),
            format!("<{field}> </{field}>"),
        ] {
            let xml = voucher_collection_xml()
                .replace(&format!("<{field}>{value}</{field}>"), &replacement);
            assert_eq!(
                parse_agent_rows(&xml, "voucher"),
                Err("agent_read_protocol_invalid".into()),
                "{field}"
            );
        }
    }
}

#[test]
fn voucher_profiles_fetch_accounting_state_and_bill_allocations() {
    for request in [
        render_agent_vouchers("Book", "20260901", "20260902", None).unwrap(),
        render_agent_changed_vouchers("Book", 1, 2),
    ] {
        let mut reader = quick_xml::Reader::from_str(&request);
        let mut fields = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Start(event) if event.name().as_ref() == b"FETCH" => {
                    fields = String::from_utf8_lossy(&reader.read_text(event.name()).unwrap())
                        .split(',')
                        .map(str::to_string)
                        .collect();
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        for field in [
            "ISCANCELLED",
            "ISOPTIONAL",
            // The ENTRY wildcard, which 2.4a proves correct on the instance where
            // curated allocation paths misreport New Ref/Agst Ref as On Account.
            // The narrower BILLALLOCATIONS.* is cheaper and measured equivalent
            // HERE, but untested THERE -- and an unverified narrowing is not worth
            // a payload saving when the failure is silently-wrong evidence.
            "ALLLEDGERENTRIES.*",
        ] {
            assert!(fields.iter().any(|value| value == field), "missing {field}");
        }
        for narrower in [
            "ALLLEDGERENTRIES.BILLALLOCATIONS.NAME",
            "ALLLEDGERENTRIES.BILLALLOCATIONS.BILLTYPE",
            "ALLLEDGERENTRIES.BILLALLOCATIONS.*",
        ] {
            assert!(
                !fields.iter().any(|value| value == narrower),
                "{narrower} must not be narrowed back in while 2.4a's instance is unverified"
            );
        }
    }
    // A read should fetch what it returns: ledger_movement discards allocations.
    let movement = render_agent_movement_vouchers("Book", "20260901", "20260902").unwrap();
    assert!(
        !movement.contains("BILLALLOCATIONS"),
        "movement must not pay the allocation payload for data MovementEntry drops"
    );
    assert!(movement.contains("ALLLEDGERENTRIES.LEDGERNAME"));

    let xml = voucher_collection_xml().replace(
        "<GUID>",
        "<ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><GUID>",
    );
    let vouchers = parse_movement_vouchers(&xml, "20260901", "20260902", "voucher").unwrap();
    assert_eq!(vouchers.len(), 1);
    assert_eq!(vouchers[0].ledger_entries.len(), 2);
    let cancelled = xml.replace("<ISCANCELLED>No", "<ISCANCELLED>Yes");
    assert!(
        parse_movement_vouchers(&cancelled, "20260901", "20260902", "voucher")
            .unwrap()
            .is_empty()
    );
}

fn settings(address: std::net::SocketAddr, data_dir: PathBuf) -> Settings {
    Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: address.port(),
        },
        data_dir,
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::MaskParties,
        import_enabled: false,
        writes_enabled: false,
    }
}

#[test]
fn agent_voucher_profile_uses_literal_filters_and_redaction_never_reveals_party() {
    let request = render_agent_vouchers("BRIDGE SYNTHETIC BOOK", "20260401", "20260430", Some(99))
        .expect("safe profile");
    assert!(request.contains("<FILTERS>BridgeAgentWindow</FILTERS>"));
    assert!(request.contains("$AlterID > 99"));
    assert_eq!(
        redact_value(
            json!({"party":party_name("Acme Party"), "narration":"private"}),
            Redaction::MaskParties
        )["party"],
        "Ac…ty"
    );
    let definitions = tool_definitions(true, false);
    let outstandings = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "outstandings"))
        .expect("outstandings tool definition");
    assert_eq!(
        outstandings["inputSchema"]["required"],
        json!(["company_guid"])
    );
    assert_eq!(
        outstandings["inputSchema"]["properties"]["direction"]["enum"],
        json!(["receivable", "payable", "both"])
    );
    let recursively_redacted = redact_value(
        json!({"proof":{"name":party_name("Acme Party")},"changed":{"ledger":party_name("Cash Ledger")}}),
        Redaction::MaskParties,
    );
    assert_eq!(recursively_redacted["proof"]["name"], "Ac…ty");
    assert_eq!(recursively_redacted["changed"]["ledger"], "Ca…er");
    assert_eq!(negotiate_protocol("2024-11-05"), Ok("2024-11-05"));
    assert_eq!(negotiate_protocol("2023-01-01"), Ok("2025-06-18"));
    assert!(parse_agent_rows(
        "<ENVELOPE><BODY><RESPONSE>bad</RESPONSE></BODY></ENVELOPE>",
        "voucher"
    )
    .is_err());
}

#[test]
fn voucher_company_name_is_validated_and_xml_escaped_without_a_tdl_literal() {
    let request = render_agent_vouchers("Bridge, + खर्चा", "20260901", "20260902", None)
        .expect("company name is an XML value");
    assert!(request.contains("<SVCURRENTCOMPANY>Bridge, + खर्चा</SVCURRENTCOMPANY>"));
    assert_eq!(
        render_agent_vouchers("invalid\ncompany", "20260901", "20260902", None),
        Err("company_name_invalid".to_string())
    );
}

#[test]
fn built_batch_stays_in_band_when_its_egress_receipt_fails() {
    let mut response = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{"type": "text", "text": ""}],
            "structuredContent": {
                "evidence": {"state": "complete"},
                "result": {"batch_id": "bridge-built"}
            },
            "isError": false
        }
    });
    assert!(attach_build_egress_failure(&mut response));
    assert_eq!(
        response["result"]["structuredContent"]["result"]["batch_id"],
        "bridge-built"
    );
    assert_eq!(
        response["result"]["structuredContent"]["result"]["egress_recorded"],
        false
    );
    assert_eq!(response["result"]["isError"], true);
    assert!(!attach_build_egress_failure(&mut json!({"result": {}})));
}

#[test]
fn tool_arguments_reject_unknown_keys_before_tool_dispatch() {
    assert_eq!(
        validate_tool_arguments(
            "vouchers",
            &json!({"company_guid":"company", "from":"2026-09-01", "to":"2026-09-02", "ledgre":"Cash"}),
        ),
        Err("argument_unknown".to_string())
    );
}

#[tokio::test]
async fn invalid_scope_arguments_are_rejected_before_any_tally_probe() {
    let directory = tempfile::tempdir().expect("agent directory");
    let server = Server::new(settings(
        "127.0.0.1:9".parse().unwrap(),
        directory.path().to_path_buf(),
    ));
    for (tool, args, code) in [
        (
            "outstandings",
            json!({"company_guid":"00000000-0000-4000-8000-000000000001", "direction":"receivble"}),
            "argument_invalid:direction",
        ),
        (
            "ledger_masters",
            json!({"company_guid":"00000000-0000-4000-8000-000000000001", "fields":"complaince"}),
            "argument_invalid:fields",
        ),
        (
            "ledger_movement",
            json!({"company_guid":"00000000-0000-4000-8000-000000000001", "from":"2026-09-01", "to":"2026-09-02", "limit":0}),
            "pagination_invalid",
        ),
        (
            "vouchers",
            json!({"company_guid":"00000000-0000-4000-8000-000000000001", "from":"2026-09-01", "to":"2026-09-02", "ledger":42}),
            "argument_invalid:ledger",
        ),
        (
            "changed_since",
            json!({"company_guid":"00000000-0000-4000-8000-000000000001", "master_snapshot_alter_id":-1}),
            "changed_since_unqualified",
        ),
    ] {
        let response = server.call_tool(tool, args).await;
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"], code,
            "{tool}"
        );
    }
}

#[test]
fn ledger_master_fields_reject_unknown_schema_values() {
    assert_eq!(
        ledger_master_fields("complaince"),
        Err("argument_invalid:fields".to_string())
    );
}

#[test]
fn ledger_movement_schema_exposes_offset_and_limit() {
    let definitions = tool_definitions(true, false);
    let movement = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "ledger_movement"))
        .expect("ledger movement tool definition");
    assert_eq!(
        movement["inputSchema"]["properties"]["offset"]["minimum"],
        0
    );
    assert_eq!(movement["inputSchema"]["properties"]["limit"]["minimum"], 1);
}

#[test]
fn mask_parties_walks_every_tool_sample_response_without_leaking_party_names() {
    let known_parties = ["Customer One", "Supplier Two", "PAN Holder", "Entry Ledger"];
    let samples = BTreeMap::from([
        ("tally_status", json!({"product":"TallyPrime"})),
        (
            "list_companies",
            json!({"companies":[{"name":"Bridge Books"}]}),
        ),
        ("voucher_schema", json!({"schema":{"type":"object"}})),
        (
            "validate_masters",
            json!({"masters":[{"requested":party_name("Customer One"),"exact_live_spelling":party_name("Customer One"),"candidates":[party_name("Customer One")]}]}),
        ),
        (
            "build_import_xml",
            json!({"masters":[{"requested":party_name("Supplier Two"),"candidates":[party_name("Supplier Two")]}]}),
        ),
        ("verify_import", json!({"vouchers":[]})),
        (
            "ledger_masters",
            json!({"items":[{"name":party_name("Customer One"),"compliance":mark_compliance_party_names(json!({"name_on_pan":"PAN Holder","bank_account_holder_name":"PAN Holder","bank_details":"PAN Holder"}))}]}),
        ),
        (
            "vouchers",
            mark_voucher_party_names(
                json!({"party":"Customer One","party_ledger_name":"Customer One","amounts":[{"ledger":"Entry Ledger"}]}),
            ),
        ),
        (
            "changed_since",
            json!({"masters":[mark_changed_master_party_name(json!({"name":"Supplier Two"}))]}),
        ),
        (
            "outstandings",
            json!({"top_parties":[{"party":party_name("Customer One")}],"open_bills":[{"party":party_name("Supplier Two")}],"unallocated":{"parties":[{"party":party_name("Supplier Two")}]}}),
        ),
        (
            "ledger_movement",
            json!({"ledgers":[{"ledger":party_name("Entry Ledger")}]}),
        ),
        (
            "trial_balance",
            json!({"ledgers":[{"ledger":party_name("Entry Ledger")}]}),
        ),
        ("read_evidence", json!({"records":[]})),
        ("egress_log", json!({"records":[]})),
    ]);
    assert_eq!(samples.len(), 14);
    for (tool, sample) in samples {
        let redacted = redact_value(sample, Redaction::MaskParties);
        assert_no_known_party_name(&redacted, &known_parties, tool);
    }
}

fn assert_no_known_party_name(value: &Value, known_parties: &[&str], tool: &str) {
    match value {
        Value::String(text) => assert!(
            known_parties.iter().all(|party| !text.contains(party)),
            "{tool} leaked a party name: {text}"
        ),
        Value::Array(values) => {
            for value in values {
                assert_no_known_party_name(value, known_parties, tool);
            }
        }
        Value::Object(values) => {
            assert!(
                !values.contains_key(PARTY_NAME_MARKER),
                "{tool} returned an unmaterialized PartyName marker"
            );
            for value in values.values() {
                assert_no_known_party_name(value, known_parties, tool);
            }
        }
        _ => {}
    }
}

#[test]
fn redaction_setting_defaults_only_when_unset_and_rejects_unknown_values() {
    assert_eq!(Redaction::from_setting(None), Ok(Redaction::None));
    assert_eq!(Redaction::parse("none"), Ok(Redaction::None));
    assert_eq!(Redaction::parse("mask_parties"), Ok(Redaction::MaskParties));
    assert_eq!(
        Redaction::parse("drop_narration"),
        Ok(Redaction::DropNarration)
    );
    assert_eq!(
        Redaction::parse("mask_everything"),
        Err("redaction_setting_invalid".to_string())
    );
}

#[tokio::test]
async fn malformed_tool_name_is_in_band_and_the_same_session_serves_the_next_request() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let (mut client, server_io) = tokio::io::duplex(4_096);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let serve = tokio::spawn(async move {
        serve_stdio(server, BufReader::new(server_read), &mut server_write).await
    });
    client.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\"}}\n").await.expect("initialize");
    client
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\",\"params\":{}}\n",
        )
        .await
        .expect("write requests");
    client.shutdown().await.expect("close request stream");
    let mut output = String::new();
    client
        .read_to_string(&mut output)
        .await
        .expect("read responses");
    let output = output
        .split_once('\n')
        .expect("initialize response")
        .1
        .to_string();
    serve
        .await
        .expect("server task")
        .expect("session remains healthy");
    let responses = output
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON-RPC response"))
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["error"]["code"], -32602);
    assert_eq!(responses[0]["error"]["message"], "tool_name_required");
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[1]["result"], json!({}));
}

#[tokio::test]
async fn egress_receipt_uses_the_final_jsonrpc_replacement_when_only_the_envelope_exceeds_cap() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let base_settings = Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    };
    let unframed = Server::new(base_settings.clone())
        .call_tool("voucher_schema", json!({}))
        .await;
    let server = Server::new(Settings {
        max_bytes: unframed.to_string().len(),
        ..base_settings
    });
    let (mut client, server_io) = tokio::io::duplex(16_384);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let serve = tokio::spawn(async move {
        serve_stdio(server, BufReader::new(server_read), &mut server_write).await
    });
    client.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\"}}\n").await.expect("initialize");
    client
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"voucher_schema\",\"arguments\":{}}}\n",
        )
        .await
        .expect("write tool request");
    client.shutdown().await.expect("close request stream");
    let mut output = String::new();
    client
        .read_to_string(&mut output)
        .await
        .expect("read response");
    let output = output
        .split_once('\n')
        .expect("initialize response")
        .1
        .to_string();
    serve
        .await
        .expect("server task")
        .expect("session remains healthy");

    let response: Value = serde_json::from_str(output.trim_end()).expect("JSON-RPC response");
    assert_eq!(response["error"]["message"], "agent_response_too_large");
    let receipt_line =
        fs::read_to_string(directory.path().join("agent-egress.jsonl")).expect("egress receipt");
    let receipt: Value =
        serde_json::from_str(receipt_line.lines().next().unwrap()).expect("receipt JSON");
    assert_eq!(receipt["bytes_prepared"], output.len());
    assert_eq!(receipt["response_sha256"], sha256_hex(output.as_bytes()));
    assert_eq!(receipt["rows_prepared"], 0);
    assert_eq!(receipt["fields_prepared"], json!([]));
    assert_eq!(receipt["truncated"], false);
}

#[tokio::test]
async fn tools_call_notifications_are_refused_and_receipted_without_dispatch() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let (mut client, server_io) = tokio::io::duplex(4_096);
    let (server_read, mut server_write) = tokio::io::split(server_io);
    let serve = tokio::spawn(async move {
        serve_stdio(server, BufReader::new(server_read), &mut server_write).await
    });
    client
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"tools/call\",\"params\":{\"name\":\"list_companies\",\"arguments\":{}}}\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":{}}\n")
        .await
        .expect("write notification and ping");
    client.shutdown().await.expect("close request stream");
    let mut output = String::new();
    client
        .read_to_string(&mut output)
        .await
        .expect("read response");
    serve
        .await
        .expect("server task")
        .expect("session remains healthy");
    assert_eq!(
        serde_json::from_str::<Value>(output.trim_end()).expect("ping response")["id"],
        1
    );
    let receipt = fs::read_to_string(directory.path().join("agent-egress.jsonl"))
        .expect("notification refusal receipt");
    assert!(receipt.contains("list_companies"));
    assert_eq!(receipt.lines().count(), 1);
}

#[tokio::test]
async fn pagination_rejects_present_invalid_values_in_helpers_and_row_tools() {
    assert_eq!(arg_usize(&json!({}), "limit", 20), Ok(20));
    for value in [json!(-1), json!(1.5), json!("10")] {
        assert_eq!(
            arg_usize(&json!({"limit": value}), "limit", 20),
            Err("pagination_invalid".to_string())
        );
    }
    for key in ["limit", "top"] {
        let mut args = json!({});
        args[key] = json!(0);
        assert_eq!(
            arg_positive_usize(&args, key, 20),
            Err("pagination_invalid".to_string())
        );
    }

    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let response = server.call_tool("egress_log", json!({"limit": "10"})).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"],
        "pagination_invalid"
    );
}

#[test]
fn optional_filters_reject_non_string_values_before_widening_a_read() {
    assert_eq!(
        optional_string(&json!({"ledger": 42}), "ledger"),
        Err("argument_invalid:ledger".to_string())
    );
    assert_eq!(
        optional_string(&json!({"voucher_type": []}), "voucher_type"),
        Err("argument_invalid:voucher_type".to_string())
    );
    assert_eq!(optional_string(&json!({}), "ledger"), Ok(None));
}

#[test]
fn configured_agent_limits_reject_malformed_or_out_of_range_values() {
    for value in ["not-a-number", "0", "10001"] {
        assert_eq!(
            parse_bounded_limit("BRIDGE_AGENT_MAX_ROWS", value, 1, 10_000),
            Err("limit_setting_invalid:BRIDGE_AGENT_MAX_ROWS".to_string())
        );
    }
    assert_eq!(
        parse_bounded_limit("BRIDGE_AGENT_MAX_BYTES", "256", 256, 5_000_000),
        Ok(256)
    );
}

#[test]
fn concurrent_egress_appends_leave_two_parseable_json_lines() {
    let directory = tempfile::tempdir().expect("temporary egress directory");
    let path = directory.path().join("agent-egress.jsonl");
    let first = path.clone();
    let second = path.clone();
    let first = std::thread::spawn(move || append_egress_line(&first, r#"{"tool":"one"}"#));
    let second = std::thread::spawn(move || append_egress_line(&second, r#"{"tool":"two"}"#));
    first.join().expect("first writer").expect("first append");
    second
        .join()
        .expect("second writer")
        .expect("second append");
    let lines = fs::read_to_string(path)
        .expect("egress file")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("parseable receipt"))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().any(|line| line["tool"] == "one"));
    assert!(lines.iter().any(|line| line["tool"] == "two"));
}

#[test]
fn egress_tail_waits_for_an_exclusive_append_lock() {
    let directory = tempfile::tempdir().expect("temporary egress directory");
    let path = directory.path().join("agent-egress.jsonl");
    let mut writer = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&path)
        .expect("writer");
    writer.lock().expect("writer lock");
    writer
        .write_all(b"{\"tool\":\"complete\"}\n")
        .expect("complete row");
    let reader_path = path.clone();
    let reader = std::thread::spawn(move || read_egress_tail(&reader_path, 1));
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(
        !reader.is_finished(),
        "tail read must wait for the writer lock"
    );
    writer.unlock().expect("writer unlock");
    assert_eq!(
        reader.join().expect("reader").expect("tail").records,
        vec!["{\"tool\":\"complete\"}"]
    );
}

#[test]
fn voucher_parser_keeps_pipe_characters_inside_structured_ledger_names() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-1</GUID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><ALLLEDGERENTRIES.LIST><LEDGERNAME>A|B</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let rows = parse_agent_rows(xml, "voucher").expect("voucher rows");
    assert_eq!(rows[0]["amounts"][0]["ledger"], "A|B");
    assert_eq!(rows[0]["amounts"][0]["amount"], "-10");
}

#[test]
fn voucher_and_changed_parsers_reject_incomplete_ledger_entries() {
    for entry in [
        "<AMOUNT>-10</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
        "<LEDGERNAME>Expense</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
        "<LEDGERNAME>Expense</LEDGERNAME><AMOUNT>-10</AMOUNT>",
    ] {
        let xml = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>voucher-guid</GUID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST>{entry}</ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        );
        assert_eq!(
            parse_agent_rows(&xml, "voucher"),
            Err("agent_read_protocol_invalid".to_string())
        );
        assert_eq!(
            parse_agent_changed_rows(&xml, "voucher"),
            Err("agent_read_protocol_invalid".to_string())
        );
    }
}

#[test]
fn voucher_parsers_decode_entities_in_vouchers_and_change_feeds() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>voucher-guid</GUID><VOUCHERNUMBER>R&amp;D</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>R&amp;D</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let vouchers = parse_agent_rows(xml, "voucher").expect("voucher rows");
    assert_eq!(vouchers[0]["voucher_number"], "R&D");
    assert_eq!(vouchers[0]["amounts"][0]["ledger"], "R&D");
    let changed = parse_agent_changed_rows(xml, "voucher").expect("changed voucher rows");
    assert_eq!(changed[0]["voucher_number"], "R&D");
    assert_eq!(changed[0]["amounts"][0]["ledger"], "R&D");
}

#[test]
fn voucher_parsers_preserve_entity_adjacent_whitespace() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>voucher-guid</GUID><VOUCHERNUMBER> before&amp;after </VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME> Input&amp;CGST </LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let vouchers = parse_agent_rows(xml, "voucher").expect("voucher rows");
    assert_eq!(vouchers[0]["voucher_number"], " before&after ");
    assert_eq!(vouchers[0]["amounts"][0]["ledger"], " Input&CGST ");
    let changed = parse_agent_changed_rows(xml, "voucher").expect("changed voucher rows");
    assert_eq!(changed[0]["voucher_number"], " before&after ");
    assert_eq!(changed[0]["amounts"][0]["ledger"], " Input&CGST ");
}

#[test]
fn voucher_ledger_filter_drops_mixed_response_rows_that_do_not_match_live_spelling() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-keep</GUID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><VOUCHERNUMBER>keep</VOUCHERNUMBER><ALLLEDGERENTRIES.LIST><LEDGERNAME>R and D</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER><VOUCHER><GUID>voucher-drop</GUID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><VOUCHERNUMBER>drop</VOUCHERNUMBER><ALLLEDGERENTRIES.LIST><LEDGERNAME>Sales</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let rows = parse_agent_rows(xml, "voucher").expect("mixed voucher rows");
    let rows = filter_voucher_rows_for_ledger(rows, "R and D");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["voucher_number"], "keep");
}

#[test]
fn resolved_ledger_filter_keeps_only_the_exact_live_spelling() {
    let resolved = resolve_ledger_name(["AB", "A-B"].into_iter(), "AB")
        .expect("exact requested ledger resolves");
    let rows = filter_voucher_rows_for_ledger(
        vec![
            json!({"voucher_number":"exact","amounts":[{"ledger":"AB"}]}),
            json!({"voucher_number":"near","amounts":[{"ledger":"A-B"}]}),
        ],
        &resolved,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["voucher_number"], "exact");
}

#[test]
fn voucher_window_is_validated_before_the_ledger_selector() {
    let rows = vec![
        json!({"date":"20260901","amounts":[{"ledger":"AB"}]}),
        json!({"date":"20260915","amounts":[{"ledger":"A-B"}]}),
    ];
    assert_eq!(
        validate_then_filter_voucher_rows(rows, "20260901", "20260902", Some("AB")),
        Err("window_not_honoured".to_string())
    );
}

#[test]
fn client_side_ledger_filter_accepts_unquoted_tdl_ledger_names() {
    for ledger in ["Input CGST 9%", "=BVL Zeta Formula", "खर्चा"] {
        let resolved = resolve_ledger_name([ledger].into_iter(), ledger)
            .expect("live ledger spelling resolves");
        let request = render_agent_vouchers("BRIDGE SYNTHETIC BOOK", "20260901", "20260902", None)
            .expect("date-only voucher request");
        assert!(!request.contains(ledger), "ledger must not enter TDL");
        let rows = filter_voucher_rows_for_ledger(
            vec![json!({"amounts":[{"ledger": ledger}]})],
            &resolved,
        );
        assert_eq!(rows.len(), 1, "ledger remains selected after parsing");
    }
}

#[test]
fn changed_voucher_rows_require_typed_accounting_state() {
    let complete = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>voucher-guid</GUID><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>Yes</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    let rows = parse_agent_changed_rows(complete, "voucher").expect("changed voucher rows");
    assert_eq!(rows[0]["cancelled"], false);
    assert_eq!(rows[0]["optional"], true);

    let absent = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>voucher-guid</GUID><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(
        parse_agent_changed_rows(absent, "voucher"),
        Err("voucher_accounting_state_not_observed".to_string())
    );
}

#[test]
fn changed_voucher_rows_require_a_stable_identity() {
    let missing = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(
        parse_agent_changed_rows(missing, "voucher"),
        Err("voucher_company_identity_invalid".to_string())
    );

    let master_id = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><MASTERID>3</MASTERID><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(
        parse_agent_changed_rows(master_id, "voucher"),
        Err("voucher_company_identity_invalid".into())
    );
}

#[test]
fn changed_voucher_rows_require_date_and_voucher_type() {
    for missing in [
        "<VOUCHERTYPENAME>Payment</VOUCHERTYPENAME>",
        "<DATE>20260901</DATE>",
    ] {
        let xml = format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-guid</GUID>{missing}<ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        );
        assert_eq!(
            parse_agent_changed_rows(&xml, "voucher"),
            Err("change_row_core_field_invalid".to_string())
        );
    }
}

#[tokio::test]
async fn stored_evidence_records_have_individual_timestamps_and_durations() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    server.call_tool("voucher_schema", json!({})).await;
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    server.call_tool("voucher_schema", json!({})).await;
    let store = server.evidence.lock().expect("evidence records");
    let records = &store.records;
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.duration_ms.is_some()));
    assert_ne!(records[0].read_at, records[1].read_at);
}

#[test]
fn status_endpoint_uses_the_transport_canonical_loopback_origin() {
    for (host, expected) in [
        ("127.0.0.1", "http://127.0.0.1:9000"),
        ("::1", "http://[::1]:9000"),
    ] {
        assert_eq!(
            endpoint_origin(&TallyEndpointConfig {
                host: host.to_string(),
                port: 9000,
            }),
            Ok(expected.to_string())
        );
    }
}

#[test]
fn egress_log_tail_reads_only_the_last_bounded_chunks() {
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let path = directory.path().join("agent-egress.jsonl");
    let mut body = String::new();
    for index in 0..12_000 {
        body.push_str(&format!(
            "{{\"index\":{index},\"padding\":\"xxxxxxxxxxxxxxxxxxxxxxxx\"}}\n"
        ));
    }
    assert!(body.len() > EGRESS_TAIL_CHUNK_BYTES);
    fs::write(&path, body).expect("large egress fixture");
    let tail = read_egress_tail(&path, 2).expect("bounded tail");
    assert_eq!(tail.records.len(), 2);
    assert!(tail.records[0].contains("11999"));
    assert!(tail.records[1].contains("11998"));
    assert!(tail.truncated);
}

#[test]
fn ledger_lookup_prefers_exact_live_spelling_and_rejects_ambiguous_matches() {
    let names = ["Bank Charges", "Sales-Ledger"];
    assert_eq!(
        resolve_ledger_name(names.into_iter(), "bank_charges"),
        Ok("Bank Charges".to_string())
    );
    assert_eq!(
        resolve_ledger_name(names.into_iter(), "missing ledger"),
        Err("ledger_not_found".to_string())
    );
    let ambiguous = ["A-B", "AB"];
    assert_eq!(
        resolve_ledger_name(ambiguous.into_iter(), "a_b"),
        Err("ledger_ambiguous".to_string())
    );
    assert_eq!(
        resolve_ledger_name(ambiguous.into_iter(), "AB"),
        Ok("AB".to_string())
    );
}

#[test]
fn tally_port_defaults_only_when_the_environment_value_is_absent() {
    assert_eq!(tally_port(None), Ok(9000));
    assert_eq!(tally_port(Some("9001".to_string())), Ok(9001));
    for port in [1, u16::MAX] {
        assert_eq!(tally_port(Some(port.to_string())), Ok(port));
    }
    for value in ["0", "65536", "-1", "", "not-a-port"] {
        assert_eq!(
            tally_port(Some(value.to_string())),
            Err("port_setting_invalid".to_string()),
            "{value} must fail at settings admission"
        );
    }
}

#[test]
fn only_change_feed_responses_may_trim_cursor_rows() {
    for (tool, key) in [
        ("verify_import", "vouchers"),
        ("validate_masters", "masters"),
    ] {
        let response = json!({"result": {key: [{"value": "x".repeat(512)}]}});
        assert_eq!(
            enforce_response_byte_cap(response, 128),
            Err("agent_response_too_large".to_string()),
            "{tool} must not have rows trimmed without a change-feed cursor"
        );
    }
}

#[test]
fn ledger_movement_evidence_changes_when_a_voucher_response_changes() {
    let evidence = |response| Evidence {
        request_sha256: "request".into(),
        response_sha256: sha256_hex(response),
        bytes: response.len(),
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    let baseline = combine_evidence(evidence(b"ledger"), evidence(b"voucher-one"));
    let changed = combine_evidence(evidence(b"ledger"), evidence(b"voucher-two"));
    assert_ne!(baseline.response_sha256, changed.response_sha256);
}

#[test]
fn ledger_masters_evidence_changes_when_a_ledger_response_changes() {
    let identity = Evidence {
        request_sha256: "company-request".into(),
        response_sha256: "company-response".into(),
        bytes: 10,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    let read = |response| {
        evidence_from_runtime_read(RuntimeReadEvidence::paired(
            "ledger-request",
            sha256_hex(response),
            response.len(),
        ))
    };
    let baseline = combine_evidence(identity.clone(), read(b"ledger-one"));
    let changed = combine_evidence(identity, read(b"ledger-two"));

    assert_ne!(baseline.response_sha256, changed.response_sha256);
    assert!(changed.bytes > 10, "the ledger read bytes are retained");
}

#[test]
fn ledger_movement_rejects_a_window_before_the_observed_books_from() {
    assert_eq!(
        ensure_movement_window_within_books("20260331", "20260401"),
        Err("window_precedes_books_from".to_string())
    );
    assert_eq!(
        ensure_movement_window_within_books("20260401", "20260401"),
        Ok(())
    );
}

#[test]
fn ledger_movement_opening_export_is_pinned_to_admitted_books_from() {
    use bridge_tally_protocol::native_outstandings::{
        render_native_ledger_export_request, NativeLedgerExportPeriod,
        NativeLedgerExportPeriodError,
    };
    use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;

    let books_from = bridge_tally_core::TallyDate::parse("20260401").expect("BooksFrom");
    let last_voucher = bridge_tally_core::TallyDate::parse("20260915").expect("last voucher");
    let matching = NativeLedgerExportPeriod::new(
        DateBoundaryProfile::EducationRestricted,
        books_from,
        last_voucher.clone(),
    )
    .expect("an admitted BOOKSFROM period");
    let request = render_native_ledger_export_request("Book", &matching);
    assert!(request.contains("<SVFROMDATE TYPE=\"Date\">20260401</SVFROMDATE>"));

    assert_eq!(
        NativeLedgerExportPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            bridge_tally_core::TallyDate::parse("20260415").expect("mismatched date"),
            last_voucher,
        ),
        Err(NativeLedgerExportPeriodError::UnsupportedBoundary),
        "an unsupported opening boundary is rejected before Tally can substitute its display period",
    );
}

#[test]
fn ledger_movement_marks_an_unobserved_post_books_opening_partial() {
    let (row, partial) = ledger_movement_row(
        LedgerMovementRow {
            name: "Customer A".to_string(),
            parent: Some("Sundry Debtors".to_string()),
            opening: None,
            debit: "10".to_string(),
            credit: "0".to_string(),
            vouchers_touching: 1,
        },
        Redaction::None,
    )
    .expect("partial movement row");
    assert!(partial);
    assert_eq!(row["opening"], Value::Null);
    assert_eq!(row["closing"], Value::Null);
    assert_eq!(row["state"], "partial");
    assert_eq!(row["reason"], "opening_balance_not_observed");
}

#[test]
fn outstandings_top_ranking_uses_all_open_bills_not_the_report_cap() {
    let bills = (1..=12)
        .map(|index| OpenBillRow {
            party: format!("Party {index:02}"),
            reference: format!("REF-{index}"),
            bill_date: "20260901".to_string(),
            due_date: "20260901".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse(index.to_string())
                .expect("synthetic amount"),
            age_days: Some(index),
            kind: ExposureDirection::Receivable,
        })
        .collect::<Vec<_>>();
    let ranked = ranked_parties_from_exposure(&bills, &[], 12).expect("ranked parties");
    assert_eq!(ranked.len(), 12);
    let ranked = redact_value(json!({"parties": ranked}), Redaction::None);
    assert_eq!(ranked["parties"][0]["party"], "Party 12");
    assert_eq!(ranked["parties"][11]["party"], "Party 01");
}

#[test]
fn outstandings_top_ranking_includes_wholly_unallocated_parties() {
    let bills = vec![OpenBillRow {
        party: "Billed Party".to_string(),
        reference: "B-1".to_string(),
        bill_date: "20260901".to_string(),
        due_date: "20260901".to_string(),
        amount: bridge_tally_core::ExactDecimal::parse("40".to_string()).expect("synthetic amount"),
        age_days: Some(1),
        kind: ExposureDirection::Receivable,
    }];
    let unallocated = vec![UnallocatedParty {
        party: "Unallocated Party".to_string(),
        amount: bridge_tally_core::ExactDecimal::parse("100".to_string())
            .expect("synthetic amount"),
        direction: ExposureDirection::Receivable,
    }];

    let ranked = redact_value(
        json!({"parties": ranked_parties_from_exposure(&bills, &unallocated, 2).expect("ranking")}),
        Redaction::None,
    );
    assert_eq!(ranked["parties"][0]["party"], "Unallocated Party");
    assert_eq!(ranked["parties"][0]["gross_billed"], "0");
    assert_eq!(ranked["parties"][0]["unallocated_receivable"], "100");
    assert_eq!(ranked["parties"][0]["gross_exposure"], "100");
}

#[test]
fn payable_outstandings_views_exclude_mixed_receivable_rows() {
    let bill = |party: &str, amount: &str, age_days, kind| OpenBillRow {
        party: party.to_string(),
        reference: format!("{party}-REF"),
        bill_date: "20260901".to_string(),
        due_date: "20260901".to_string(),
        amount: bridge_tally_core::ExactDecimal::parse(amount.to_string())
            .expect("synthetic amount"),
        age_days: Some(age_days),
        kind,
    };
    let mixed_bills = vec![
        bill("Customer A", "100", 12, ExposureDirection::Receivable),
        bill("Supplier B", "200", 45, ExposureDirection::Payable),
    ];
    let payable_bills = mixed_bills
        .into_iter()
        .filter(|bill| direction_matches(bill.kind, "payable"))
        .collect::<Vec<_>>();
    assert_eq!(payable_bills.len(), 1);
    assert_eq!(payable_bills[0].party, "Supplier B");
    assert_eq!(
        outstanding_totals_from_open_bills(&payable_bills).expect("payable totals"),
        json!({"scope":"open_bills_only", "receivable":"0", "payable":"200", "gross_billed":"200"})
    );
    assert_eq!(
        ageing_buckets_from_open_bills(&payable_bills).expect("payable ageing"),
        json!({"unaged":"0", "days_0_30":"0", "days_31_60":"200", "days_61_90":"0", "days_90_plus":"0"})
    );
    let ranked = redact_value(
        json!({"parties": ranked_parties_from_exposure(&payable_bills, &[], 10).expect("payable ranking")}),
        Redaction::None,
    );
    assert_eq!(ranked["parties"][0]["party"], "Supplier B");
    let mixed_unallocated = vec![
        UnallocatedParty {
            party: "Customer A".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse("30".to_string())
                .expect("synthetic amount"),
            direction: ExposureDirection::Receivable,
        },
        UnallocatedParty {
            party: "Supplier B".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse("40".to_string())
                .expect("synthetic amount"),
            direction: ExposureDirection::Payable,
        },
    ];
    let payable_unallocated = mixed_unallocated
        .into_iter()
        .filter(|party| direction_matches(party.direction, "payable"))
        .collect::<Vec<_>>();
    assert_eq!(payable_unallocated.len(), 1);
    assert_eq!(payable_unallocated[0].party, "Supplier B");
    assert_eq!(
        unallocated_totals_from_parties(&payable_unallocated).expect("payable unallocated"),
        json!({"receivable":"0", "payable":"40", "gross_unallocated":"40"})
    );
}

#[test]
fn future_due_open_bills_remain_unaged() {
    let bill = OpenBillRow {
        party: "Customer".to_string(),
        reference: "FUTURE".to_string(),
        bill_date: "20260910".to_string(),
        due_date: "20260910".to_string(),
        amount: bridge_tally_core::ExactDecimal::parse("25".to_string()).expect("amount"),
        age_days: None,
        kind: ExposureDirection::Receivable,
    };
    assert_eq!(
        ageing_buckets_from_open_bills(&[bill]).expect("ageing buckets"),
        json!({"unaged":"25", "days_0_30":"0", "days_31_60":"0", "days_61_90":"0", "days_90_plus":"0"})
    );
}

#[test]
fn unallocated_parties_are_bounded_without_dropping_the_aggregate() {
    let (page, truncated, next_offset) =
        paginate_open_bills(vec![json!({"party":"A"}), json!({"party":"B"})], 0, 1);
    assert_eq!(page.len(), 1);
    assert!(truncated);
    assert_eq!(next_offset, Some(1));

    let mut response = json!({
        "result": {
            "unallocated": {
                "count": 2,
                "amount": "30",
                "parties": [json!({"party":"A"}), json!({"party":"B"})],
                "truncated": false,
                "next_offset": null,
            }
        }
    });
    assert!(truncate_response_items(&mut response).expect("trims unallocated parties"));
    assert_eq!(response["result"]["unallocated"]["count"], 2);
    assert_eq!(response["result"]["unallocated"]["amount"], "30");
    assert_eq!(
        response["result"]["unallocated"]["parties"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(response["result"]["unallocated"]["next_offset"], 1);
}

#[test]
fn byte_trimming_keeps_each_cursor_relative_to_its_requested_offset() {
    for key in ["items", "open_bills", "ledgers"] {
        let mut response = json!({"result": {"offset": 500}});
        response["result"][key] = json!(vec![json!({"row": 1}); 101]);
        assert!(truncate_response_items(&mut response).expect("trims result page"));
        assert_eq!(response["result"][key].as_array().map(Vec::len), Some(100));
        assert_eq!(response["result"]["next_offset"], 600);
    }

    let mut response = json!({
        "result": {
            "offset": 500,
            "unallocated": {
                "parties": vec![json!({"party": "Supplier"}); 101],
                "next_offset": 601,
            }
        }
    });
    assert!(truncate_response_items(&mut response).expect("trims unallocated page"));
    assert_eq!(
        response["result"]["unallocated"]["parties"]
            .as_array()
            .map(Vec::len),
        Some(100)
    );
    assert_eq!(response["result"]["unallocated"]["next_offset"], 600);
}

#[test]
fn byte_trimming_change_feed_rows_stops_checkpoint_advancement() {
    let mut response = json!({"result": {
        "voucher_alter_id": 10, "master_alter_id": 20,
        "next_voucher_alter_id": 12, "next_master_alter_id": 22,
        "checkpoint_advanceable": true,
        "vouchers": [{"alter_id": 11}, {"alter_id": 12}],
        "masters": [{"alter_id": 21}]
    }});
    assert!(truncate_response_items(&mut response).expect("trims vouchers"));
    assert_eq!(
        response["result"]["vouchers"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(response["result"]["next_voucher_alter_id"], 11);
    assert_eq!(response["result"]["checkpoint_advanceable"], false);
    assert!(response["truncated"].as_bool().expect("truncated"));
}

#[tokio::test]
async fn imports_are_hidden_and_refused_without_explicit_live_evidence_opt_in() {
    let disabled_tools = tool_definitions(false, false);
    let names = disabled_tools
        .as_array()
        .expect("tool list")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(!names.contains(&"build_import_xml"));
    assert!(tool_definitions(true, false)
        .as_array()
        .expect("tool list")
        .iter()
        .any(|tool| tool["name"] == "build_import_xml"));
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let response = server.call_tool("build_import_xml", json!({})).await;
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"],
        "import_unverified_on_live_tally"
    );
}

#[test]
fn outstandings_receipt_counts_wholly_unallocated_party_rows() {
    let response = json!({
        "result": {
            "top_parties": [{"party":"On-account"}],
            "open_bills": [],
            "unallocated": {"parties": [{"party":"On-account"}]},
        }
    });
    assert_eq!(response_row_count(&response), Some(1));
}

#[test]
fn open_bill_paging_marks_remaining_rows_and_returns_a_cursor() {
    let (page, truncated, next_offset) = paginate_open_bills(vec!["a", "b", "c"], 0, 2);
    assert_eq!(page, vec!["a", "b"]);
    assert!(truncated);
    assert_eq!(next_offset, Some(2));

    let (last_page, last_truncated, last_next_offset) =
        paginate_open_bills(vec!["a", "b", "c"], 2, 2);
    assert_eq!(last_page, vec!["c"]);
    assert!(!last_truncated);
    assert_eq!(last_next_offset, None);
}

#[test]
fn changed_since_checkpoints_round_trip_as_numbers_with_numeric_string_compatibility() {
    let observed =
        observed_checkpoint(Some(&"42".to_string()), "voucher").expect("numeric high water");
    let response = json!({"next_voucher_alter_id": observed});
    assert_eq!(
        checkpoint_arg(&response, "next_voucher_alter_id"),
        Ok(Some(42))
    );
    assert_eq!(
        checkpoint_arg(&json!({"voucher_alter_id":"42"}), "voucher_alter_id"),
        Ok(Some(42))
    );
    assert_eq!(
        checkpoint_arg(&json!({"voucher_alter_id":"bad"}), "voucher_alter_id"),
        Err("checkpoint_invalid".to_string())
    );
}

#[test]
fn change_feed_snapshot_cursor_excludes_rows_inserted_after_first_page() {
    let row = |alter_id| json!({"alter_id": alter_id});
    let (first_page, first_truncated, first_cursor) =
        stable_change_page(vec![row(3), row(1), row(2)], 0, 3, 2).expect("first page");
    assert!(first_truncated);
    assert_eq!(
        first_page
            .iter()
            .map(|row| row["alter_id"].as_u64())
            .collect::<Vec<_>>(),
        vec![Some(1), Some(2)]
    );
    assert_eq!(first_cursor, 2);

    // ALTERID 4 is inserted after page one. The original high-water 3 pins
    // page two, so ID 3 is still returned and the new row cannot shift it.
    let (second_page, second_truncated, second_cursor) =
        stable_change_page(vec![row(4), row(3)], first_cursor, 3, 2)
            .expect("snapshot-pinned second page");
    assert!(!second_truncated);
    assert_eq!(second_page, vec![row(3)]);
    assert_eq!(second_cursor, 3);

    let voucher_request = render_agent_changed_vouchers("BRIDGE SYNTHETIC BOOK", 2, 3);
    let master_request =
        render_agent_changed_masters("BRIDGE SYNTHETIC BOOK", 2, 3, MasterKind::Ledger);
    for request in [voucher_request, master_request] {
        assert!(request.contains("$AlterID &gt; 2 AND $AlterID &lt;= 3"));
        assert!(request.contains("<SORT>Default: $AlterID</SORT>"));
    }
    let definitions = catalog::registered_tool_definitions(false, false);
    let changed_since = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "changed_since"))
        .expect("changed-since schema");
    assert!(changed_since["inputSchema"]["properties"]["offset"].is_null());
}

#[test]
fn truncated_change_feed_never_advances_the_checkpoint() {
    assert!(!checkpoint_advanceable(Some(12), 0, 12, true));
    assert!(!checkpoint_advanceable(Some(11), 0, 12, false));
    assert!(checkpoint_advanceable(Some(12), 0, 12, false));
}

#[test]
fn change_scan_rejects_any_missing_or_malformed_row_alter_id() {
    assert_eq!(
        validate_change_row_alter_ids(&[json!({"alter_id":null})]),
        Err("change_row_alterid_invalid".to_string())
    );
    assert_eq!(
        parse_agent_changed_masters("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER><NAME>Bad</NAME><ALTERID>nope</ALTERID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"),
        Err("change_row_alterid_invalid".to_string())
    );
    assert_eq!(
        parse_agent_changed_masters("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><GROUP><ALTERID>3</ALTERID></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"),
        Err("change_row_name_invalid".to_string())
    );
    assert_eq!(
        parse_agent_changed_masters("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER><NAME>   </NAME><ALTERID>3</ALTERID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"),
        Err("change_row_name_invalid".to_string())
    );
}

#[test]
fn changed_master_parser_decodes_entity_fragments() {
    let rows = parse_agent_changed_masters("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER><NAME>R&amp;D</NAME><PARENT>Income &amp; Expense</PARENT><ALTERID>3</ALTERID><GUID>ledger-guid</GUID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>").expect("changed master");
    assert_eq!(rows[0]["name"], "R&D");
    assert_eq!(rows[0]["parent"], "Income & Expense");
}

#[test]
fn changed_master_rows_require_guid_or_master_id() {
    let missing_identity = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER><NAME>Cash</NAME><ALTERID>3</ALTERID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(
        parse_agent_changed_masters(missing_identity),
        Err("change_row_identity_invalid".to_string())
    );
    assert!(
        render_agent_changed_masters("BRIDGE SYNTHETIC BOOK", 2, 3, MasterKind::Ledger)
            .contains("<FETCH>NAME,PARENT,ALTERID,GUID,MASTERID</FETCH>")
    );
}

#[test]
fn master_domain_high_water_ignores_unsupported_master_types() {
    let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER><ALTERID>4</ALTERID></LEDGER><GROUP><ALTERID>7</ALTERID></GROUP><STOCKITEM><ALTERID>99</ALTERID></STOCKITEM></COLLECTION></DATA></BODY></ENVELOPE>";
    assert_eq!(parse_master_domain_high_water(xml), Ok(7));
    for kind in MasterKind::ALL {
        let request = render_agent_master_domain_high_water("Book", kind);
        assert!(request.contains("<TYPE>Ledger</TYPE>") || request.contains("<TYPE>Group</TYPE>"));
        assert!(!request.contains("ALTMSTID"));
    }
}

#[test]
fn response_byte_cap_covers_both_complete_content_representations() {
    let input = json!({"truncated":false,"result":{"offset":0,"items":[{"name":"first"},{"name":"second"}]}});
    let cap =
        json!({"truncated":true,"result":{"offset":0,"items":[{"name":"first"}],"next_offset":1}})
            .to_string()
            .len();
    let (bounded, truncated, rows) = enforce_response_byte_cap(input, cap).expect("one row fits");
    assert!(truncated);
    assert_eq!(rows, 1);
    assert_eq!(bounded["result"]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(bounded["result"]["next_offset"], 1);
    assert_eq!(
        enforce_response_byte_cap(json!({"result":{"items":[{"name":"x".repeat(500)}]}}), 32),
        Err("agent_response_too_large".to_string())
    );

    let one_item = json!({
        "content":[{"type":"text","text":""}],
        "structuredContent":{"company":{"name":"Book"},"evidence":{"state":"complete"},"truncated":true,"result":{"offset":0,"items":[{"name":"first"}],"next_offset":1}},
        "isError":false,
    });
    let mut one_item = one_item;
    enforce_mcp_result_byte_cap(&mut one_item, 10_000, "vouchers", 1).expect("summary");
    let cap = json!({"jsonrpc":"2.0","id":1,"result":one_item})
        .to_string()
        .len()
        + 1;
    let mut response = json!({
        "jsonrpc":"2.0",
        "id":1,
        "result":{
            "content":[{"type":"text","text":""}],
            "structuredContent":{"company":{"name":"Book"},"evidence":{"state":"complete"},"truncated":false,"result":{"offset":0,"items":[{"name":"first"},{"name":"second"}]}},
            "isError":false,
        }
    });
    enforce_mcp_result_byte_cap(&mut response["result"], 10_000, "vouchers", 2).expect("summary");
    enforce_jsonrpc_response_byte_cap(&mut response, cap).expect("one page row fits");
    assert!(response.to_string().len() <= cap);
    assert_eq!(
        response["result"]["structuredContent"]["result"]["items"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        serde_json::from_str::<Value>(response["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap(),
        response["result"]["structuredContent"]
    );
}

#[test]
fn accounting_day_uses_tally_host_local_calendar_at_utc_midnight() {
    let east = chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("offset");
    let local = east
        .with_ymd_and_hms(2026, 9, 5, 0, 15, 0)
        .single()
        .expect("local timestamp");
    assert_eq!(format_tally_date(local), "20260905");
}

#[test]
fn voucher_window_rejects_out_of_range_rows_and_requires_a_wider_empty_check() {
    assert!(!window_honoured(
        &[json!({"date":"20260915"})],
        "20260901",
        "20260902"
    ));
    assert!(window_honoured(&[], "20260901", "20260902"));
    assert_eq!(
        widened_window("20260901", "20260902"),
        Ok(("20260831".to_string(), "20260903".to_string()))
    );
}

#[test]
fn empty_voucher_window_corroboration_handles_all_three_control_branches() {
    assert_eq!(
        corroborate_empty_voucher_window(
            &[json!({"date":"20260831"}), json!({"date":"20260903"})],
            "20260901",
            "20260902",
            None,
        ),
        Ok((false, None))
    );
    assert_eq!(
        corroborate_empty_voucher_window(
            &[json!({"date":"20260901"})],
            "20260901",
            "20260902",
            None,
        ),
        Err("window_contradicted".to_string())
    );
    assert_eq!(
        corroborate_empty_voucher_window(&[], "20260901", "20260902", Some(0)),
        Ok((false, Some("company_has_no_vouchers")))
    );
    assert_eq!(
        corroborate_empty_voucher_window(&[], "20260901", "20260902", Some(1)),
        Ok((true, Some("empty_uncorroborated")))
    );
}

#[tokio::test]
async fn simulator_company_read_records_evidence_while_down_endpoint_is_typed() {
    let company_plan = || {
        ScenarioPlan::new(Fixture::SyntheticXml(company_collection_xml()))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    };
    let status_plan = || {
        ScenarioPlan::new(Fixture::ProductStatus(
            tally_protocol_simulator::ProductStatus::TallyPrime,
        ))
        .with_framing(ResponseFraming::ContentLength)
    };
    let simulator = SequenceSimulator::spawn(vec![
        company_plan(),
        status_plan(),
        company_plan(),
        status_plan(),
    ])
    .expect("synthetic loopback server");
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(settings(
        simulator.address(),
        directory.path().to_path_buf(),
    ));
    let response = server.call_tool("list_companies", json!({})).await;
    assert_eq!(
        response["structuredContent"]["result"]["companies"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert!(response["structuredContent"]["evidence"]["request_sha256"].is_string());
    assert_eq!(simulator.finish().expect("simulator result").len(), 4);

    let down = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let down_response = down.call_tool("tally_status", json!({})).await;
    assert!(down_response["structuredContent"]["result"]["error"]["code"].is_string());
}

#[tokio::test]
async fn voucher_read_evidence_uses_utf16_transport_bytes() {
    let captured_vouchers = include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/unit_a_optional_voucher_live.xml"
    );
    let company_bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml");
    let company_xml = String::from_utf16(
        &company_bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let company_plan = || {
        ScenarioPlan::new(Fixture::SyntheticXml(company_xml.clone()))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    };
    let voucher_plan = || {
        ScenarioPlan::new(Fixture::SyntheticXml(captured_vouchers.to_string()))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    };
    let status_plan = || {
        ScenarioPlan::new(Fixture::ProductStatus(
            tally_protocol_simulator::ProductStatus::TallyPrime,
        ))
        .with_framing(ResponseFraming::ContentLength)
    };
    let simulator = SequenceSimulator::spawn(vec![
        company_plan(),
        status_plan(),
        company_plan(),
        status_plan(),
        company_plan(),
        voucher_plan(),
        status_plan(),
        voucher_plan(),
        status_plan(),
        company_plan(),
    ])
    .expect("synthetic loopback server");
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(settings(
        simulator.address(),
        directory.path().to_path_buf(),
    ));
    let response = server
        .call_tool(
            "vouchers",
            json!({"company_guid":"bb8ad19e-6aef-4239-a917-87fec0c6215e","from":"2026-04-01","to":"2026-04-01"}),
        )
        .await;
    assert_eq!(
        response["structuredContent"]["result"]["items"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let rows = response["structuredContent"]["result"]["items"]
        .as_array()
        .unwrap();
    assert!(rows.iter().any(|row| row["optional"] == true));
    assert!(rows
        .iter()
        .all(|row| row["cancelled"].is_boolean() && row["optional"].is_boolean()));
    let expected_bytes = bridge_tally_protocol::encode_tally_xml_request_utf16le(&company_xml)
        .len()
        + bridge_tally_protocol::encode_tally_xml_request_utf16le(captured_vouchers).len();
    assert_eq!(
        response["structuredContent"]["evidence"]["bytes"],
        expected_bytes * 2
    );
    assert_ne!(
        response["structuredContent"]["evidence"]["bytes"],
        company_xml.len() + captured_vouchers.len()
    );
    assert_eq!(simulator.finish().expect("simulator result").len(), 10);
}

#[tokio::test]
async fn tally_status_does_not_infer_product_from_status_banner() {
    let simulator = SequenceSimulator::spawn(vec![
        ScenarioPlan::new(Fixture::ProductStatus(
            tally_protocol_simulator::ProductStatus::TallyPrime,
        )),
        ScenarioPlan::new(Fixture::SyntheticXml(company_collection_xml()))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength),
    ])
    .expect("synthetic loopback server");
    let directory = tempfile::tempdir().expect("temporary agent directory");
    let server = Server::new(settings(
        simulator.address(),
        directory.path().to_path_buf(),
    ));
    let response = server.call_tool("tally_status", json!({})).await;
    assert_eq!(
        response["structuredContent"]["result"]["product"],
        "not_observed"
    );
    assert_eq!(
        response["structuredContent"]["evidence"]["state"],
        "complete"
    );
    assert_eq!(simulator.finish().expect("simulator result").len(), 2);
}

#[test]
fn native_cmpinfo_counters_are_not_voucher_or_master_rows() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).expect("captured UTF-16LE response");
    assert!(parse_agent_rows(&xml, "voucher")
        .expect("native empty vouchers")
        .is_empty());
    assert!(parse_agent_changed_rows(&xml, "voucher")
        .expect("native empty changed vouchers")
        .is_empty());
    assert!(parse_agent_changed_masters(&xml)
        .expect("native empty masters")
        .is_empty());
    assert_eq!(parse_master_domain_high_water(&xml), Ok(0));
    assert_eq!(
        parse_company_high_water(&xml, "missing-guid"),
        Err("company_high_water_identity_absent".to_string())
    );
}

#[test]
fn native_captured_vouchers_keep_direct_amounts_and_padded_identifiers() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).expect("captured UTF-16LE response");
    let rows = parse_agent_changed_rows(&xml, "61c6de69-1748-461c-ad3f-162cb949df9f")
        .expect("captured native vouchers");
    assert_eq!(rows.len(), 3);
    for (row, (id, amount)) in rows
        .iter()
        .zip([(1, "-101.01"), (2, "-102.02"), (3, "-103.03")])
    {
        assert_eq!(row["alter_id"], id);
        assert_eq!(row["amounts"].as_array().unwrap().len(), 2);
        assert_eq!(row["amounts"][0]["amount"], amount);
    }
    assert_eq!(
        parse_movement_vouchers(
            &xml,
            "20260801",
            "20260801",
            "61c6de69-1748-461c-ad3f-162cb949df9f"
        )
        .unwrap()
        .len(),
        3
    );
}

#[test]
fn evidence_reads_disclose_requested_limits_and_permanent_retention_eviction() {
    let directory = tempfile::tempdir().unwrap();
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.path().into(),
        max_rows: 1000,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    assert!(!server.read_evidence(&json!({"limit":1})).unwrap().truncated);
    let record = |index: usize| Evidence {
        request_sha256: index.to_string(),
        response_sha256: "observed".into(),
        bytes: 0,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    for index in 0..3 {
        server.record_evidence(record(index));
    }
    let limited = server.read_evidence(&json!({"limit":2})).unwrap();
    assert!(limited.truncated);
    assert_eq!(
        limited.payload["result"]["records"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        limited.payload["result"]["records"][0]["request_sha256"],
        "2"
    );
    assert!(!server.read_evidence(&json!({"limit":3})).unwrap().truncated);
    for index in 3..MAX_EVIDENCE_RECORDS {
        server.record_evidence(record(index));
    }
    assert!(
        !server
            .read_evidence(&json!({"limit":MAX_EVIDENCE_RECORDS}))
            .unwrap()
            .truncated
    );
    server.record_evidence(record(MAX_EVIDENCE_RECORDS));
    let evicted = server
        .read_evidence(&json!({"limit":MAX_EVIDENCE_RECORDS + 1}))
        .unwrap();
    assert!(evicted.truncated);
    let rows = evicted.payload["result"]["records"].as_array().unwrap();
    assert_eq!(rows.len(), MAX_EVIDENCE_RECORDS);
    assert_eq!(rows.last().unwrap()["request_sha256"], "1");
}

#[tokio::test]
async fn diagnostic_history_reads_honor_the_configured_global_row_cap() {
    for max_rows in [1, 2] {
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: 9,
            },
            data_dir: directory.path().into(),
            max_rows,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
        });
        for index in 0..3 {
            server.record_evidence(Evidence {
                request_sha256: index.to_string(),
                response_sha256: "observed".into(),
                bytes: 0,
                state: "complete",
                read_at: None,
                duration_ms: None,
                reason_code: None,
            });
            append_egress_line(
                &directory.path().join("agent-egress.jsonl"),
                &json!({"index": index}).to_string(),
            )
            .unwrap();
        }
        for tool in ["read_evidence", "egress_log"] {
            for (args, expected) in [
                (json!({}), max_rows),
                (json!({"limit":256}), max_rows),
                (json!({"limit":1}), 1),
            ] {
                let response = server.call_tool(tool, args).await;
                assert_eq!(response["isError"], false, "{tool}");
                let content = &response["structuredContent"];
                assert_eq!(
                    content["result"]["records"].as_array().unwrap().len(),
                    expected,
                    "{tool} max_rows={max_rows}"
                );
                assert_eq!(
                    content["truncated"], true,
                    "omitted records remain explicit: {tool}"
                );
            }
        }
    }
}
