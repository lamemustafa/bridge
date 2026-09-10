//! Every company, ledger, party and voucher number below is fabricated or is
//! the repository's existing synthetic capture. Nothing here is evidence about
//! a real book.
use super::*;
use bridge_tally_transport::TallyEndpointConfig;
use tally_protocol_simulator::{
    Fixture, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const CAPTURED_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const GUID: &str = "00000000-0000-4000-8000-000000000001";

fn offline_server(directory: &std::path::Path) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    })
}

fn proposal(number: &str, party: &str, total: &str) -> Value {
    json!({
        "date": "20260901",
        "voucher_type": "Journal",
        "voucher_number": number,
        "party": party,
        "entries": [
            {"ledger": party, "amount": format!("-{total}")},
            {"ledger": "WR2 Sales", "amount": total},
        ],
    })
}

fn args(vouchers: Value, numbering: &str) -> Value {
    json!({
        "company_guid": CAPTURED_GUID,
        "from": "20260901",
        "to": "20260930",
        "numbering": [{"voucher_type": "Journal", "numbering_method": numbering}],
        "vouchers": vouchers,
    })
}

// --- admission, before any Tally read ----------------------------------

#[tokio::test]
async fn presence_arguments_are_bounded_before_any_tally_probe() {
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    let base = proposal("JV-1", "Cash", "12.50");
    for (arguments, code) in [
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}]}),
            "vouchers_required",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "vouchers":[base.clone()]}),
            "numbering_required",
        ),
        (
            json!({"company_guid":GUID,"from":"20260930","to":"20260901",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[base.clone()]}),
            "invalid_date_range",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"sometimes"}],
                "vouchers":[base.clone()]}),
            "argument_invalid:numbering",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[]}),
            "argument_invalid:vouchers",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[{"date":"20260901","voucher_type":"Journal","entries":[]}]}),
            "argument_invalid:vouchers",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[{"date":"20260901","voucher_type":"Journal",
                    "entries":[{"ledger":"Cash","amount":"maybe"}]}]}),
            "presence_amount_invalid",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[base.clone()],"ledger":"Cash"}),
            "argument_unknown",
        ),
    ] {
        let response = server
            .call_tool_response("voucher_presence", arguments)
            .await;
        assert_eq!(response.value["isError"], true, "{code}");
        assert_eq!(
            response.value["structuredContent"]["result"]["error"]["code"], code,
            "{code}"
        );
        // Nothing may cost a Tally read.
        assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
    }
}

#[tokio::test]
async fn an_over_large_proposal_set_is_refused_rather_than_trimmed() {
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    let vouchers = vec![proposal("JV-1", "Cash", "12.50"); MAX_PRESENCE_VOUCHERS + 1];
    let response = server
        .call_tool_response("voucher_presence", args(json!(vouchers), "manual"))
        .await;
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "argument_invalid:vouchers"
    );
}

#[test]
fn the_published_schema_names_the_three_numbering_methods_and_its_bounds() {
    let definitions = tool_definitions(true, false);
    let tool = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .expect("voucher_presence tool definition");
    let schema = &tool["inputSchema"];
    assert_eq!(
        schema["properties"]["numbering"]["items"]["properties"]["numbering_method"]["enum"],
        json!(["manual", "automatic", "unknown"])
    );
    assert_eq!(
        schema["properties"]["vouchers"]["maxItems"],
        json!(MAX_PRESENCE_VOUCHERS)
    );
    assert_eq!(
        schema["required"],
        json!(["company_guid", "from", "to", "numbering", "vouchers"])
    );
    // The tool reads; it must not be annotated as a write.
    assert!(tool.get("annotations").is_none());
}

#[test]
fn an_unknown_numbering_method_is_refused_at_the_published_schema() {
    assert_eq!(
        validate_tool_arguments(
            "voucher_presence",
            &json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[],"vouchers":[proposal("JV-1","Cash","12.50")]}),
        ),
        Err("argument_invalid:numbering".to_string())
    );
}

#[tokio::test]
async fn cross_input_refusals_also_cost_no_tally_read() {
    // A date outside the window and an undeclared voucher type depend only on
    // the arguments. Deferring them to the crate boundary would spend a
    // company probe, a catalogue read and a full voucher window first.
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    for (arguments, code) in [
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[{"date":"20261015","voucher_type":"Journal",
                    "entries":[{"ledger":"Cash","amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]}]}),
            "presence_window_does_not_cover",
        ),
        (
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[{"date":"20260901","voucher_type":"Part and Labour Sale",
                    "entries":[{"ledger":"Cash","amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]}]}),
            "presence_numbering_method_undeclared",
        ),
    ] {
        let response = server
            .call_tool_response("voucher_presence", arguments)
            .await;
        assert_eq!(
            response.value["structuredContent"]["result"]["error"]["code"], code,
            "{code}"
        );
        assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
    }
}

#[tokio::test]
async fn nested_arguments_are_bounded_to_the_published_schema() {
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    let long = "x".repeat(agent_import::MAX_MASTER_NAME_CHARS + 1);
    let entries =
        json!([{"ledger":"Cash","amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]);
    for (vouchers, numbering, code) in [
        // A voucher number past the advertised 1024 characters.
        (
            json!([{"date":"20260901","voucher_type":"Journal","voucher_number":long,"entries":entries}]),
            json!([{"voucher_type":"Journal","numbering_method":"manual"}]),
            "argument_invalid:vouchers",
        ),
        // A ledger name past the advertised limit.
        (
            json!([{"date":"20260901","voucher_type":"Journal",
                "entries":[{"ledger":long,"amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]}]),
            json!([{"voucher_type":"Journal","numbering_method":"manual"}]),
            "argument_invalid:vouchers",
        ),
        // An amount past the advertised 64 characters.
        (
            json!([{"date":"20260901","voucher_type":"Journal",
                "entries":[{"ledger":"Cash","amount":"1".repeat(65)},{"ledger":"WR2 Sales","amount":"1.00"}]}]),
            json!([{"voucher_type":"Journal","numbering_method":"manual"}]),
            "argument_invalid:vouchers",
        ),
        // A nested property the schema does not declare.
        (
            json!([{"date":"20260901","voucher_type":"Journal","narration":"hello","entries":entries}]),
            json!([{"voucher_type":"Journal","numbering_method":"manual"}]),
            "argument_invalid:vouchers",
        ),
        // A blank nested string.
        (
            json!([{"date":"20260901","voucher_type":"Journal","party":"   ","entries":entries}]),
            json!([{"voucher_type":"Journal","numbering_method":"manual"}]),
            "argument_invalid:vouchers",
        ),
        // The same discipline on the numbering declaration.
        (
            json!([{"date":"20260901","voucher_type":"Journal","entries":entries}]),
            json!([{"voucher_type":"Journal","numbering_method":"manual","note":"x"}]),
            "argument_invalid:numbering",
        ),
    ] {
        let response = server
            .call_tool_response(
                "voucher_presence",
                json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                    "numbering":numbering,"vouchers":vouchers}),
            )
            .await;
        assert_eq!(
            response.value["structuredContent"]["result"]["error"]["code"], code,
            "{code}"
        );
        assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
    }
}

// --- typed parses -------------------------------------------------------

#[test]
fn one_voucher_type_declared_two_ways_is_refused_before_a_read() {
    assert_eq!(
        parse_numbering(&json!({"numbering":[
            {"voucher_type":"Journal","numbering_method":"manual"},
            {"voucher_type":"journal","numbering_method":"automatic"},
        ]}))
        .expect_err("conflict"),
        "presence_numbering_method_conflict".to_string()
    );
}

#[test]
fn a_window_row_becomes_a_book_voucher_without_inventing_a_remote_id() {
    let row = json!({
        "guid": format!("{CAPTURED_GUID}-00000001"),
        "date": "20260901",
        "voucher_number": "JV-1",
        "voucher_type": "Journal",
        "party": "Bridge Nested Debtor WR4",
        "cancelled": false,
        "optional": false,
        "amounts": [
            {"ledger": "Bridge Nested Debtor WR4", "amount": "-12.50"},
            {"ledger": "WR2 Sales", "amount": "12.50"},
        ],
    });
    let voucher = book_voucher(&row).expect("book voucher");
    assert_eq!(voucher.key(), format!("{CAPTURED_GUID}-00000001"));
    assert_eq!(voucher.magnitude().as_str(), "12.5");
    assert!(voucher.balanced());
    assert_eq!(voucher.party(), Some("Bridge Nested Debtor WR4"));
}

#[test]
fn party_names_are_marked_for_egress_and_accounting_selectors_are_not() {
    let entry = json!({
        "position": 0,
        "voucher_number": "JV-1",
        "party": {"party_state": "bound", "catalog_name": "Bridge Nested Debtor WR4"},
        "presence": "present",
        "book_key": "book-1",
        "differences": [
            {"field": "party", "proposed": "Debtor As Written", "observed": "Bridge Nested Debtor WR4"},
            {"field": "amount", "proposed": "12.5", "observed": "11.5"},
        ],
    });
    let marked = mark_presence_party_names(entry);
    assert_eq!(
        marked["party"]["catalog_name"][super::super::PARTY_NAME_MARKER],
        "Bridge Nested Debtor WR4"
    );
    assert_eq!(
        marked["differences"][0]["proposed"][super::super::PARTY_NAME_MARKER],
        "Debtor As Written"
    );
    // An amount is not a party name and must not be wrapped.
    assert_eq!(marked["differences"][1]["proposed"], "12.5");
    assert_eq!(marked["voucher_number"], "JV-1");
    let masked = redact_value(marked, Redaction::MaskParties);
    let text = masked.to_string();
    assert!(!text.contains("Bridge Nested Debtor WR4"), "{text}");
    assert!(text.contains("12.5"));
}

// --- one live-shaped cycle ---------------------------------------------

fn company_xml() -> String {
    format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"WR2 Unicode Lab\"><GUID>{CAPTURED_GUID}</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>")
}

fn catalogue_xml() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    String::from_utf16(&words).expect("captured native catalogue")
}

/// Two vouchers already in the book, one of them posted short of its source.
fn window_xml() -> String {
    format!(
        concat!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER><DATE>20260901</DATE><VOUCHERNUMBER>JV-1</VOUCHERNUMBER>",
            "<VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>{guid}-00000001</GUID>",
            "<MASTERID>1</MASTERID><ALTERID>12</ALTERID>",
            "<PARTYLEDGERNAME>Bridge Nested Debtor WR4</PARTYLEDGERNAME>",
            "<ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Bridge Nested Debtor WR4</LEDGERNAME>",
            "<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>WR2 Sales</LEDGERNAME>",
            "<ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER>",
            "<VOUCHER><DATE>20260902</DATE><VOUCHERNUMBER>JV-2</VOUCHERNUMBER>",
            "<VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><GUID>{guid}-00000002</GUID>",
            "<MASTERID>2</MASTERID><ALTERID>13</ALTERID>",
            "<PARTYLEDGERNAME>Café Naïve Traders</PARTYLEDGERNAME>",
            "<ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Café Naïve Traders</LEDGERNAME>",
            "<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-7.00</AMOUNT></ALLLEDGERENTRIES.LIST>",
            "<ALLLEDGERENTRIES.LIST><LEDGERNAME>WR2 Sales</LEDGERNAME>",
            "<ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>7.00</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER>",
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        ),
        guid = CAPTURED_GUID
    )
}

/// The shapes the runtime actually issues: an identity pair, then one block
/// per paired native read. Built rather than hand-indexed, because this tool
/// performs three reads and an off-by-one in a literal list is a debugging
/// session, not a test failure.
enum Step {
    Company,
    Status,
    Payload(String),
}

fn paired_read(payload: &str) -> Vec<Step> {
    vec![
        Step::Company,
        Step::Payload(payload.to_string()),
        Step::Status,
        Step::Payload(payload.to_string()),
        Step::Status,
        Step::Company,
    ]
}

fn presence_plans() -> Vec<ScenarioPlan> {
    let catalogue = catalogue_xml();
    let mut steps = vec![Step::Company, Step::Status, Step::Company, Step::Status];
    // Catalogue, then the voucher window, then the catalogue again: the
    // verdict is built from two observations and the second read proves the
    // first still holds.
    steps.extend(paired_read(&catalogue));
    steps.extend(paired_read(&window_xml()));
    steps.extend(paired_read(&catalogue));
    steps
        .into_iter()
        .map(|step| match step {
            Step::Status => ScenarioPlan::new(Fixture::ProductStatus(
                tally_protocol_simulator::ProductStatus::TallyPrime,
            ))
            .with_framing(ResponseFraming::ContentLength),
            Step::Company => ScenarioPlan::new(Fixture::SyntheticXml(company_xml()))
                .with_encoding(WireEncoding::Utf16Le)
                .with_framing(ResponseFraming::ContentLength),
            Step::Payload(body) => ScenarioPlan::new(Fixture::SyntheticXml(body))
                .with_encoding(WireEncoding::Utf16Le)
                .with_framing(ResponseFraming::ContentLength),
        })
        .collect()
}

#[tokio::test]
async fn a_live_shaped_cycle_separates_present_undecided_and_absent() {
    let simulator = SequenceSimulator::spawn(presence_plans()).expect("simulator");
    let directory = tempfile::tempdir().expect("directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    });
    let response = server
        .call_tool(
            "voucher_presence",
            json!({
                "company_guid": CAPTURED_GUID,
                "from": "20260901",
                "to": "20260930",
                "numbering": [{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers": [
                    // Already in the book, and the book agrees.
                    proposal("JV-1", "Bridge Nested Debtor WR4", "12.50"),
                    // Already in the book, posted short by 0.50.
                    proposal("JV-2", "Café Naïve Traders", "7.50"),
                    // Not in the book, and nothing resembles it.
                    proposal("JV-9", "नमस्ते ट्रेडर्स", "99.00"),
                ],
            }),
        )
        .await;
    assert_eq!(response["isError"], false, "{response}");
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["profile"], "agent_voucher_presence_v1");
    assert_eq!(result["window"]["from"], "20260901");
    assert_eq!(result["window"]["to"], "20260930");
    assert_eq!(result["totals"]["requested"], 3);
    assert_eq!(result["totals"]["present"], 2);
    assert_eq!(result["totals"]["absent"], 1);
    assert_eq!(result["totals"]["possibly_present"], 0);

    let vouchers = result["vouchers"].as_array().expect("vouchers");
    assert_eq!(vouchers[0]["presence"], "present");
    assert_eq!(vouchers[0]["basis"], "manual_voucher_number");
    assert_eq!(vouchers[0]["book_key"], format!("{CAPTURED_GUID}-00000001"));
    assert!(vouchers[0]["differences"]
        .as_array()
        .expect("differences")
        .is_empty());

    // The number identified it, so the date and amount the book disagrees on
    // are findings about the book, not evidence against the match.
    assert_eq!(vouchers[1]["presence"], "present");
    let differences = vouchers[1]["differences"].as_array().expect("differences");
    assert_eq!(differences.len(), 2);
    assert_eq!(differences[0]["field"], "date");
    assert_eq!(differences[0]["proposed"], "20260901");
    assert_eq!(differences[0]["observed"], "20260902");
    assert_eq!(differences[1]["field"], "amount");
    assert_eq!(differences[1]["proposed"], "7.5");
    assert_eq!(differences[1]["observed"], "7");

    assert_eq!(vouchers[2]["presence"], "absent");
    assert!(vouchers[2].get("book_key").is_none());
    assert_eq!(vouchers[2]["party"]["catalog_name"], "नमस्ते ट्रेडर्स");

    // The book half of the report is computed, not asked for.
    assert_eq!(result["book"]["window_voucher_count"], 2);
    assert_eq!(result["book"]["remote_id_observed"], false);
    assert_eq!(result["book"]["duplicate_number_group_count"], 0);
    assert_eq!(result["book"]["unmatched_book_vouchers"], 0);
    assert_eq!(
        response["structuredContent"]["evidence"]["state"],
        "complete"
    );
    let observed = simulator.finish().expect("requests");
    assert_eq!(observed.len(), 22);
}
