//! The `parse_bank_statement` tool surface: gating, admission, what leaves the
//! machine, and where the password may and may not appear.
//!
//! Tests that open a PDF need the PDFium library and are ignored by default;
//! run them with `BRIDGE_PDFIUM_LIBRARY=/abs/path/libpdfium.dylib cargo test
//! --lib bank_statement -- --ignored`.
use super::*;

const PASSWORD: &str = "synthetic-user-4321";

fn server(directory: &Path, import_enabled: bool, redaction: Redaction) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
        data_dir: directory.join("agent"),
        max_rows: 500,
        max_bytes: 200_000,
        redaction,
        import_enabled,
        writes_enabled: false,
        batch_post_enabled: false,
    })
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/bridge-bank-statement/tests/fixtures")
        .join(name)
}

/// A private copy of a fixture and a password file, as an operator would
/// have them.
fn statement_files(directory: &Path, pdf: &str, password: &str) -> (PathBuf, PathBuf) {
    let statement = directory.join("statement.pdf");
    fs::copy(fixture(pdf), &statement).unwrap();
    let password_file = directory.join("statement.password");
    fs::write(&password_file, format!("{password}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&password_file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    (statement, password_file)
}

fn arguments(statement: &Path, password_file: &Path) -> Value {
    json!({
        "statement_path": statement.to_str().unwrap(),
        "password_file": password_file.to_str().unwrap(),
        "bank": "hdfc",
        "account_label": "Synthetic CA xx4321",
        "opening_balance": "1,000.00",
        "closing_balance": "1,02,200.00",
        "total_debits": "8,800.00",
        "total_credits": "1,10,000.00",
        "bank_ledger": "Synthetic Bank Ledger",
        "suspense_ledger": "Suspense",
        "mapping": [
            {"party": "NORTHWIND TRADERS", "ledger": "Northwind Traders"},
            {"party": "SILVER OAK MUTUAL", "ledger": "Silver Oak Mutual Fund", "treatment": "auto"}
        ]
    })
}

fn error_code(response: &Value) -> Option<&str> {
    response["structuredContent"]["result"]["error"]["code"].as_str()
}

fn every_file_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[tokio::test]
async fn the_tool_is_hidden_and_refused_without_the_import_opt_in() {
    for (enabled, listed) in [(false, false), (true, true)] {
        assert_eq!(
            tool_definitions(enabled, false)
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "parse_bank_statement"),
            listed
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let (statement, password_file) =
        statement_files(directory.path(), "hdfc-synthetic.pdf", PASSWORD);
    let response = server(directory.path(), false, Redaction::None)
        .call_tool(
            "parse_bank_statement",
            arguments(&statement, &password_file),
        )
        .await;
    assert_eq!(
        error_code(&response),
        Some("import_unverified_on_live_tally")
    );
}

#[test]
fn the_published_schema_uses_only_patterns_the_validator_implements() {
    fn patterns(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(pattern)) = map.get("pattern") {
                    found.push(pattern.clone());
                }
                map.values().for_each(|child| patterns(child, found));
            }
            Value::Array(values) => values.iter().for_each(|child| patterns(child, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    patterns(&input_schema(), &mut found);
    found.sort();
    found.dedup();
    assert_eq!(found, [r"\S", "^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"]);
}

#[tokio::test]
async fn inputs_are_refused_before_the_statement_is_opened() {
    let directory = tempfile::tempdir().unwrap();
    let server = server(directory.path(), true, Redaction::None);
    // no statement file exists at all: every refusal below must come first
    let missing = directory.path().join("absent.pdf");
    let base = arguments(&missing, &directory.path().join("absent.password"));
    let cases: [(&str, Value, &str); 8] = [
        (
            "statement_path",
            json!("relative/statement.pdf"),
            "argument_invalid:statement_path",
        ),
        (
            "password_file",
            json!("statement.password"),
            "argument_invalid:password_file",
        ),
        ("bank", json!("icici"), "argument_invalid:bank"),
        (
            "total_debits",
            json!("-8,800.00"),
            "statement_malformed_control_value",
        ),
        (
            "opening_balance",
            json!("1.005"),
            "statement_malformed_control_value",
        ),
        ("from", json!("2026-02-30"), "invalid_date"),
        (
            "mapping",
            json!([{"party": "UNRESOLVED", "ledger": "Anything"}]),
            "statement_mapping_claims_a_sentinel",
        ),
        (
            "mapping",
            json!([{"party": "A", "ledger": "L", "treatment": "transfer"}]),
            "argument_invalid:mapping",
        ),
    ];
    for (key, value, expected) in cases {
        let mut args = base.clone();
        args[key] = value;
        let response = server.call_tool("parse_bank_statement", args).await;
        assert_eq!(error_code(&response), Some(expected), "{key}");
    }
    let mut one_total = base.clone();
    one_total.as_object_mut().unwrap().remove("total_credits");
    assert_eq!(
        error_code(&server.call_tool("parse_bank_statement", one_total).await),
        Some("statement_control_totals_incomplete")
    );
    // HDFC prints its totals, so they cannot be left out; Union Bank prints none
    let mut no_totals = base.clone();
    for key in ["total_debits", "total_credits"] {
        no_totals.as_object_mut().unwrap().remove(key);
    }
    assert_eq!(
        error_code(
            &server
                .call_tool("parse_bank_statement", no_totals.clone())
                .await
        ),
        Some("statement_control_totals_required")
    );
    no_totals["bank"] = json!("ubi");
    assert_eq!(
        error_code(&server.call_tool("parse_bank_statement", no_totals).await),
        Some("statement_file_unreadable")
    );
    let mut reversed = base.clone();
    reversed["from"] = json!("2026-08-31");
    reversed["to"] = json!("2026-08-01");
    assert_eq!(
        error_code(&server.call_tool("parse_bank_statement", reversed).await),
        Some("statement_reversed_date_window")
    );
    let mut collision = base.clone();
    collision["mapping"] = json!([
        {"party": "ZEPHYR MANUFACTURING", "ledger": "One"},
        {"party": "ZEPHYRMANUFACTURING", "ledger": "Two"}
    ]);
    assert_eq!(
        error_code(&server.call_tool("parse_bank_statement", collision).await),
        Some("statement_mapping_key_collision")
    );
    assert_eq!(
        error_code(&server.call_tool("parse_bank_statement", base).await),
        Some("statement_file_unreadable")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_password_file_others_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let (statement, password_file) =
        statement_files(directory.path(), "hdfc-synthetic.pdf", PASSWORD);
    fs::set_permissions(&password_file, fs::Permissions::from_mode(0o640)).unwrap();
    let response = server(directory.path(), true, Redaction::None)
        .call_tool(
            "parse_bank_statement",
            arguments(&statement, &password_file),
        )
        .await;
    assert_eq!(
        error_code(&response),
        Some("statement_password_file_permissions")
    );
    assert!(!response.to_string().contains(PASSWORD));
}

#[tokio::test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
async fn only_the_summary_leaves_and_the_password_appears_nowhere() {
    assert!(env::var_os("BRIDGE_PDFIUM_LIBRARY").is_some());
    let directory = tempfile::tempdir().unwrap();
    let (statement, password_file) =
        statement_files(directory.path(), "hdfc-synthetic.pdf", PASSWORD);
    let server = server(directory.path(), true, Redaction::None);
    let response = server
        .call_tool(
            "parse_bank_statement",
            arguments(&statement, &password_file),
        )
        .await;
    let result = &response["structuredContent"]["result"];
    assert!(error_code(&response).is_none(), "{response}");
    assert_eq!(result["statement_rows"], 6);
    assert_eq!(result["vouchers"], 6);
    assert_eq!(result["suspense_rows"], 4);
    assert_eq!(result["account_last4"], "4321");
    assert_eq!(result["reconciled"]["running_balance_every_row"], true);
    assert_eq!(result["reconciled"]["totals_match_statement"], true);
    let northwind = result["counterparties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["party"] == "NORTHWIND TRADERS")
        .expect("northwind group");
    assert_eq!(northwind["ledger"], "Northwind Traders");
    assert_eq!(northwind["disposition"], "Receipt");
    assert_eq!(northwind["total"], "10000.00");

    // row-level content stays in the file: no reference, narration, label or
    // transaction date reaches the response
    let text = response.to_string();
    for private in [
        PASSWORD,
        "612345678901",
        "45678901",
        "PAYMENT",
        "narration",
        "bridge_txn_id",
        "st-2026",
        "2026-08-01",
        "00000000004321",
    ] {
        assert!(
            !text.contains(private),
            "{private} left the machine: {text}"
        );
    }

    let path = PathBuf::from(result["path"].as_str().unwrap());
    let bytes = fs::read(&path).unwrap();
    assert_eq!(sha256_hex(&bytes), result["sha256"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let document: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(document["schema"], PROPOSALS_SCHEMA);
    let vouchers = document["vouchers"].as_array().unwrap();
    assert_eq!(vouchers.len(), 6);
    // exactly build_import_xml's voucher fields; no identity of our own
    for voucher in vouchers {
        let mut keys: Vec<&str> = voucher
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "bridge_txn_id",
                "date",
                "entries",
                "narration",
                "voucher_type"
            ]
        );
        assert_eq!(voucher["entries"].as_array().unwrap().len(), 2);
    }
    assert!(!String::from_utf8_lossy(&bytes).contains("REMOTEID"));

    // Through the real MCP framing, which is what writes egress receipts: the
    // password reaches neither the wire nor any file this server wrote.
    let protocol_directory = tempfile::tempdir().unwrap();
    let protocol_server = self::server(protocol_directory.path(), true, Redaction::None);
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"parse_bank_statement","arguments":arguments(&statement, &password_file)}}),
    ];
    let input: String = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect();
    let mut output = Vec::new();
    agent_protocol::serve_stdio(
        protocol_server,
        tokio::io::BufReader::new(input.as_bytes()),
        &mut output,
    )
    .await
    .unwrap();
    let wire = String::from_utf8(output).unwrap();
    assert!(wire.contains("\"proposals_id\""), "{wire}");
    assert!(!wire.contains(PASSWORD));
    let written = every_file_under(&protocol_directory.path().join("agent"));
    assert!(
        written
            .iter()
            .any(|file| file.ends_with("agent-egress.jsonl")),
        "no egress receipt was written: {written:?}"
    );
    for file in written {
        let content = fs::read(&file).unwrap();
        assert!(
            !String::from_utf8_lossy(&content).contains(PASSWORD),
            "password written to {}",
            file.display()
        );
    }

    // a corrected mapping keeps every transaction label
    let mut remapped = arguments(&statement, &password_file);
    remapped["mapping"] = json!([]);
    let second = server.call_tool("parse_bank_statement", remapped).await;
    let second_path = second["structuredContent"]["result"]["path"]
        .as_str()
        .unwrap();
    let second_document: Value = serde_json::from_slice(&fs::read(second_path).unwrap()).unwrap();
    let labels = |document: &Value| {
        document["vouchers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|voucher| voucher["bridge_txn_id"].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(labels(&document), labels(&second_document));
    assert_eq!(second["structuredContent"]["result"]["suspense_rows"], 6);
}

#[tokio::test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
async fn mask_parties_masks_every_name_in_the_summary() {
    assert!(env::var_os("BRIDGE_PDFIUM_LIBRARY").is_some());
    let directory = tempfile::tempdir().unwrap();
    let (statement, password_file) =
        statement_files(directory.path(), "hdfc-synthetic.pdf", PASSWORD);
    let response = server(directory.path(), true, Redaction::MaskParties)
        .call_tool(
            "parse_bank_statement",
            arguments(&statement, &password_file),
        )
        .await;
    assert!(error_code(&response).is_none(), "{response}");
    let text = response.to_string();
    for name in [
        "NORTHWIND TRADERS",
        "Northwind Traders",
        "SILVER OAK MUTUAL",
        "BLUE RIVER CO",
        "GREEN-FIELD",
        "Synthetic Bank Ledger",
        "Suspense",
    ] {
        assert!(!text.contains(name), "{name} was not masked: {text}");
    }
}

#[tokio::test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
async fn refusals_name_a_category_and_a_row_never_the_statement() {
    assert!(env::var_os("BRIDGE_PDFIUM_LIBRARY").is_some());
    let directory = tempfile::tempdir().unwrap();
    let (statement, password_file) =
        statement_files(directory.path(), "hdfc-synthetic.pdf", "not-the-password");
    let server = server(directory.path(), true, Redaction::None);
    let wrong = server
        .call_tool(
            "parse_bank_statement",
            arguments(&statement, &password_file),
        )
        .await;
    assert_eq!(error_code(&wrong), Some("statement_unreadable_pdf"));
    assert!(!wrong.to_string().contains("not-the-password"));

    fs::write(&password_file, PASSWORD).unwrap();
    for (key, value, expected) in [
        (
            "account_label",
            "xx9876",
            "statement_account_not_in_statement",
        ),
        (
            "closing_balance",
            "1,02,201.00",
            "statement_extent_unproven",
        ),
        (
            "total_debits",
            "8,801.00",
            "statement_control_total_mismatch",
        ),
        (
            "opening_balance",
            "1,001.00",
            "statement_balance_chain_broken:row_1",
        ),
    ] {
        let mut args = arguments(&statement, &password_file);
        args[key] = json!(value);
        let response = server.call_tool("parse_bank_statement", args).await;
        assert_eq!(error_code(&response), Some(expected), "{key}");
    }
    // nothing was published for a refused run
    assert!(
        !directory.path().join("agent/bank-statements").exists()
            || every_file_under(&directory.path().join("agent/bank-statements")).is_empty()
    );
}

#[tokio::test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
async fn a_union_bank_statement_parses_without_printed_totals() {
    assert!(env::var_os("BRIDGE_PDFIUM_LIBRARY").is_some());
    let directory = tempfile::tempdir().unwrap();
    let (statement, password_file) =
        statement_files(directory.path(), "ubi-synthetic.pdf", "synthetic-user-7788");
    let server = server(directory.path(), true, Redaction::None);
    let args = json!({
        "statement_path": statement.to_str().unwrap(),
        "password_file": password_file.to_str().unwrap(),
        "bank": "ubi",
        "account_label": "UBI SB xx7788",
        "opening_balance": "10,000.00",
        "closing_balance": "500.00",
        "bank_ledger": "Synthetic Bank Ledger",
        "suspense_ledger": "Suspense",
        "mapping": [{"party": "CASH DEPOSIT", "ledger": "Cash", "treatment": "contra"}]
    });
    let response = server.call_tool("parse_bank_statement", args.clone()).await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["statement_rows"], 6, "{response}");
    assert_eq!(result["vouchers"], 6);
    assert_eq!(result["reconciled"]["total_debits"], "13250.50");
    assert_eq!(result["reconciled"]["total_credits"], "3750.50");
    assert_eq!(result["reconciled"]["totals_match_statement"], false);

    // a closing balance the rows do not reach is still refused
    let mut short = args;
    short["closing_balance"] = json!("0.00");
    assert_eq!(
        error_code(&server.call_tool("parse_bank_statement", short).await),
        Some("statement_extent_unproven")
    );
}
