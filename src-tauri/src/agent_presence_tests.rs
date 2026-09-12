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

/// `remote_id` is no longer an accepted input: the shipped read cannot fetch
/// `REMOTEID`, so supplying one could only ever withhold a verdict that a
/// unique manual number would otherwise settle. Refusing the input is more
/// honest than accepting it and degrading.
#[tokio::test]
async fn a_remote_id_is_not_an_accepted_input_at_this_surface() {
    let definitions = tool_definitions(true, false);
    let schema = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .expect("voucher_presence schema")["inputSchema"]
        .clone();
    assert!(schema["properties"]["vouchers"]["items"]["properties"]
        .get("remote_id")
        .is_none());
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    let response = server
        .call_tool_response(
            "voucher_presence",
            json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers":[{"date":"20260901","voucher_type":"Journal","remote_id":"tally-1",
                    "entries":[{"ledger":"Cash","amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]}]}),
        )
        .await;
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "argument_invalid:vouchers"
    );
    assert_eq!(response.value["structuredContent"]["evidence"]["bytes"], 0);
}

#[test]
fn the_result_is_pageable_so_an_over_large_report_is_not_discarded() {
    // `page_shape` recognises `items` with an `offset`; without that this
    // shape is untrimmable and a complete report is replaced wholesale by
    // `agent_response_too_large` after every Tally read has been paid for.
    let mut structured = json!({"result":{"offset":0,"total":3,"items":
        (0..3).map(|id| json!({"position":id,"padding":"x".repeat(256)})).collect::<Vec<_>>()}});
    let (bounded, trimmed, _) =
        enforce_response_byte_cap(structured.clone(), 400).expect("trims rather than refusing");
    assert!(trimmed);
    let kept = bounded["result"]["items"].as_array().expect("items");
    assert!(!kept.is_empty() && kept.len() < 3);
    assert_eq!(bounded["result"]["next_offset"], kept.len());
    // And an untrimmed report keeps every row and offers no cursor.
    structured["result"]["items"] = json!([{"position":0}]);
    let (complete, trimmed, _) = enforce_response_byte_cap(structured, 10_000).expect("fits");
    assert!(!trimmed);
    assert!(complete["result"].get("next_offset").is_none());
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

/// The bound is not restated anywhere, so the test must not restate it either:
/// it reads `maxLength` out of the published schema and proves the boundary
/// tracks it. If the schema moves, this moves with it; if the enforcement stops
/// following the schema, this fails.
#[tokio::test]
async fn nested_bounds_are_read_from_the_schema_rather_than_duplicated() {
    let definitions = tool_definitions(true, false);
    let schema = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .map(|tool| tool["inputSchema"].clone())
        .expect("voucher_presence schema");
    let limit = schema["properties"]["vouchers"]["items"]["properties"]["voucher_number"]
        ["maxLength"]
        .as_u64()
        .expect("a published maxLength") as usize;
    let entries =
        json!([{"ledger":"Cash","amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]);
    let numbering = json!([{"voucher_type":"Journal","numbering_method":"manual"}]);
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    for (length, refused) in [(limit, false), (limit + 1, true)] {
        let response = server
            .call_tool_response(
                "voucher_presence",
                json!({"company_guid":GUID,"from":"20260901","to":"20260930","numbering":numbering,
                    "vouchers":[{"date":"20260901","voucher_type":"Journal",
                        "voucher_number":"x".repeat(length),"entries":entries}]}),
            )
            .await;
        let code = &response.value["structuredContent"]["result"]["error"]["code"];
        assert_eq!(
            code == "argument_invalid:vouchers",
            refused,
            "length {length} against a published limit of {limit}"
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

/// Turns the step list into simulator plans. Separated from `presence_plans`
/// so a test that varies one payload does not restate the framing of all six.
fn plans(steps: Vec<Step>) -> Vec<ScenarioPlan> {
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

fn presence_plans() -> Vec<ScenarioPlan> {
    let catalogue = catalogue_xml();
    let mut steps = vec![Step::Company, Step::Status, Step::Company, Step::Status];
    // Catalogue, then the voucher window, then the catalogue again: the
    // verdict is built from two observations and the second read proves the
    // first still holds.
    steps.extend(paired_read(&catalogue));
    steps.extend(paired_read(&window_xml()));
    steps.extend(paired_read(&catalogue));
    plans(steps)
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
    assert_eq!(result["total"], 3);
    assert_eq!(result["offset"], 0);
    assert_eq!(result["totals"]["requested"], 3);
    assert_eq!(result["totals"]["present"], 2);
    assert_eq!(result["totals"]["absent"], 1);
    assert_eq!(result["totals"]["possibly_present"], 0);

    let vouchers = result["items"].as_array().expect("items");
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

/// The admission contract this tool enforces lives in `agent_catalog.rs`, and
/// that file is **not** in the compatibility surface — so an edit confined to
/// it could loosen what a caller may send while the sealed digest and the
/// evidence beneath it stayed unchanged.
///
/// The numeric bounds are safe already: the schema references constants that
/// live in pinned files. What an unpinned edit could change is the *structure*
/// — dropping `additionalProperties`, widening the numbering enum, removing a
/// required field. So the structure is asserted here, in a pinned file, which
/// makes a silent loosening fail a test rather than pass a seal.
///
/// Pinning `agent_catalog.rs` instead would also work and is strictly
/// stronger, but it is a shared decision rather than this lane's: that file is
/// edited by every tool change, so pinning it makes every such change reseal,
/// and it would move this PR's `MAX_SURFACE_FILES` arithmetic that the merge
/// order already depends on.
#[test]
fn every_admission_leaf_is_pinned_by_this_digest() {
    // The assertions below this one say what the contract *means*, and they
    // are worth reading. They cannot be complete: the parser drives itself
    // from the published schema, so every leaf in it is admission-relevant,
    // and a review found the previous version silently omitting
    // `entries.items.properties.amount.maxLength` among others. Enumerating
    // leaves is a list that goes stale; a digest over the whole schema cannot.
    //
    // This file is pinned into the compatibility surface, so changing the
    // schema now forces this constant to change, which moves the surface
    // digest, which is exactly the visibility the seal is for. If this fails
    // and the schema change was deliberate, update the constant *and* reseal
    // — that pairing is the point, not an inconvenience.
    const PINNED: &str = "785b14835f3235ec009a248ac2b443316e764c584b31f2532aa1335365c5fb40";
    let definitions = tool_definitions(true, false);
    let schema = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .expect("voucher_presence tool")["inputSchema"]
        .clone();
    // `serde_json::Value` orders object keys, so this is canonical already,
    // and `sha256_json` is the digest this module already uses for evidence.
    let digest = sha256_json(&schema);
    assert_eq!(
        digest, PINNED,
        "the published admission contract changed; update this digest in the same commit that \
         reseals the compatibility surface"
    );
}

#[test]
fn the_admission_contract_cannot_be_loosened_without_failing_something() {
    let definitions = tool_definitions(true, false);
    let schema = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .expect("voucher_presence tool")["inputSchema"]
        .clone();
    let voucher = &schema["properties"]["vouchers"]["items"];
    let numbering = &schema["properties"]["numbering"]["items"];

    // Nothing undeclared may be sent, at any level.
    for object in [
        &schema,
        voucher,
        numbering,
        &voucher["properties"]["entries"]["items"],
    ] {
        assert_eq!(
            object["additionalProperties"],
            json!(false),
            "an undeclared property would be accepted here"
        );
    }
    // The three numbering methods are the vocabulary; a fourth would mean the
    // crate's `Unknown` fallback silently absorbed it.
    assert_eq!(
        numbering["properties"]["numbering_method"]["enum"],
        json!(["manual", "automatic", "unknown"])
    );
    // A proposal without entries has no magnitude, and one without a date or
    // type cannot be placed in a window.
    assert_eq!(
        voucher["required"],
        json!(["date", "voucher_type", "entries"])
    );
    assert_eq!(
        numbering["required"],
        json!(["voucher_type", "numbering_method"])
    );
    assert_eq!(
        voucher["properties"]["entries"]["items"]["required"],
        json!(["ledger", "amount"])
    );
    // REMOTEID matching is unreachable from the shipped read, so the input
    // stays absent rather than accepted-and-degraded.
    assert!(voucher["properties"].get("remote_id").is_none());
    // Every bound the parser relies on is still stated, since the parser reads
    // them from here rather than restating them.
    for (path, expected) in [
        (
            &voucher["properties"]["voucher_type"]["maxLength"],
            agent_import::MAX_MASTER_NAME_CHARS,
        ),
        (
            &voucher["properties"]["voucher_number"]["maxLength"],
            agent_import::MAX_MASTER_NAME_CHARS,
        ),
        (
            &voucher["properties"]["party"]["maxLength"],
            agent_import::MAX_MASTER_NAME_CHARS,
        ),
    ] {
        assert_eq!(path.as_u64(), Some(expected as u64));
    }
    assert_eq!(
        voucher["properties"]["entries"]["maxItems"].as_u64(),
        Some(MAX_PRESENCE_ENTRIES as u64)
    );
}

/// A ledger the book posts to that the catalogue never listed proves the
/// catalogue short. Both catalogue reads agreeing only proves they agree.
///
/// Left unchecked this is the quiet failure: a proposal naming that ledger
/// binds `Unmatched`, so every party rule declines to run, and an `Absent`
/// gets authorised off a comparison that was never possible — which is the
/// duplicate this whole contract exists to prevent.
#[tokio::test]
async fn a_ledger_missing_from_the_catalogue_fails_closed() {
    let unlisted = window_xml().replace("WR2 Sales", "WR2 Sales Not In Catalogue");
    let catalogue = catalogue_xml();
    let mut steps = vec![Step::Company, Step::Status, Step::Company, Step::Status];
    steps.extend(paired_read(&catalogue));
    steps.extend(paired_read(&unlisted));
    steps.extend(paired_read(&catalogue));
    let simulator = SequenceSimulator::spawn(plans(steps)).expect("simulator");
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
        .call_tool_response(
            "voucher_presence",
            json!({
                "company_guid": CAPTURED_GUID,
                "from": "20260901",
                "to": "20260930",
                "numbering": [{"voucher_type":"Journal","numbering_method":"manual"}],
                // Dated and numbered away from both book rows, so nothing but
                // the party could have surfaced them. Without the guard this
                // returns `absent` and a caller imports a second copy.
                "vouchers": [proposal("JV-77", "WR2 Sales Not In Catalogue", "12.50")],
            }),
        )
        .await;
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "ledger_catalogue_incomplete"
    );
}

/// A party's entity shape is decided entirely by the caller's text, so the
/// tool must refuse it before spending a read — the promise every other
/// argument refusal on this tool already keeps.
///
/// The endpoint here is a *live simulator*, deliberately. Against an offline
/// server this assertion passes whether or not the guard exists, because a
/// failed connection also spends no bytes: the test could not tell "refused
/// before reading" from "the read did not work". With reads available, zero
/// bytes means the refusal really did come first.
#[tokio::test]
async fn a_party_with_too_many_identifiers_costs_no_read() {
    let party = (1..=33)
        .map(|index| format!("{:08}", 10_000_000 + index))
        .collect::<Vec<_>>()
        .join(" ");
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
        .call_tool_response(
            "voucher_presence",
            json!({
                "company_guid": CAPTURED_GUID,
                "from": "20260901",
                "to": "20260930",
                "numbering": [{"voucher_type":"Journal","numbering_method":"manual"}],
                "vouchers": [proposal("JV-1", &party, "12.50")],
            }),
        )
        .await;
    assert_eq!(
        response.value["structuredContent"]["result"]["error"]["code"],
        "master_identifiers_too_many"
    );
    assert_eq!(
        response.value["structuredContent"]["evidence"]["bytes"], 0,
        "an input that was always going to be refused must cost no read"
    );
}
/// The observations can outgrow the byte cap on their own, and `fit_response`
/// cannot reach them - it trims `items`, and `book` is a sibling. Worse, the
/// final framing serializes the payload twice, so the real cost is doubled.
///
/// The premise is asserted first: a `book` at the crate's documented maxima
/// really does exceed the budget. Without that, the degradation below would be
/// a control whose branch never fires.
#[test]
fn a_maximal_book_degrades_to_its_counts_rather_than_costing_the_report() {
    // Both bounds count *characters*, so the widest value they admit is a
    // four-byte one. A real Tally GUID is 36 ASCII bytes and nowhere near
    // this; the guard exists because the contract permits this, not because
    // the common case needs it.
    let key = "\u{1f600}".repeat(book_presence::MAX_BOOK_KEY_CHARS);
    let label = "\u{1f600}".repeat(book_presence::MAX_OBSERVATION_LABEL_CHARS);
    let groups = (0..book_presence::MAX_DUPLICATE_NUMBER_GROUPS)
        .map(|_| {
            json!({
                "voucher_type": label, "voucher_number": label,
                "book_keys": (0..book_presence::MAX_KEYS_PER_DUPLICATE_GROUP)
                    .map(|_| key.clone()).collect::<Vec<_>>(),
                "book_voucher_count": 10,
            })
        })
        .collect::<Vec<_>>();
    let book = json!({
        "duplicate_numbers": groups,
        "duplicate_number_group_count": book_presence::MAX_DUPLICATE_NUMBER_GROUPS,
        "duplicate_numbers_truncated": false,
        "unbalanced_vouchers": (0..book_presence::MAX_UNBALANCED_LISTED)
            .map(|_| key.clone()).collect::<Vec<_>>(),
        "unbalanced_voucher_count": book_presence::MAX_UNBALANCED_LISTED,
        "unmatched_book_vouchers": 0, "window_voucher_count": 20_000,
        "remote_id_observed": false,
    });
    let budget = 200_000 / OBSERVATION_BUDGET_DIVISOR;
    let full = book.to_string().len();
    assert!(
        full > budget,
        "the premise fails: a maximal book is {full} bytes against a budget of {budget}"
    );
    assert!(
        full * 2 > 200_000,
        "the doubled envelope should clear the default cap on observations alone"
    );

    let bounded = bounded_observations(book, budget);
    assert!(bounded.to_string().len() <= budget);
    // Every count survives, and the listing keeps as many rows as fit rather
    // than emptying: dropping the lot would satisfy a laxer assertion than
    // this one, so the retained count is bounded on both sides.
    let listed = bounded["duplicate_numbers"]
        .as_array()
        .expect("listed")
        .len();
    assert!(
        listed < book_presence::MAX_DUPLICATE_NUMBER_GROUPS,
        "nothing was trimmed"
    );
    assert!(
        listed > 0,
        "the whole listing was dropped rather than trimmed"
    );
    assert_eq!(bounded["listings_withheld_for_size"], json!(true));
    assert_eq!(bounded["duplicate_numbers_truncated"], json!(true));
    assert_eq!(
        bounded["duplicate_number_group_count"],
        json!(book_presence::MAX_DUPLICATE_NUMBER_GROUPS)
    );
    assert_eq!(bounded["window_voucher_count"], json!(20_000));

    // A book that fits comes back untouched, with no marker added.
    let small = json!({"duplicate_numbers": [], "window_voucher_count": 3});
    assert_eq!(bounded_observations(small.clone(), budget), small);

    // And a book that is over by a little keeps most of its listing rather
    // than losing all of it -- the row-by-row part, which a wholesale drop
    // would pass the assertions above without ever doing.
    let rows = (0..40).map(|_| json!(key)).collect::<Vec<_>>();
    let large = json!({"duplicate_numbers": [], "unbalanced_vouchers": rows,
        "unbalanced_voucher_count": 40, "window_voucher_count": 40});
    let kept = bounded_observations(large, 12_000);
    let listed = kept["unbalanced_vouchers"]
        .as_array()
        .expect("listed")
        .len();
    assert!(
        (1..40).contains(&listed),
        "expected a partial listing, kept {listed} of 40"
    );
    assert_eq!(kept["unbalanced_voucher_count"], json!(40));
    assert_eq!(kept["listings_withheld_for_size"], json!(true));
}

// ---------------------------------------------------------------------------
// Live replay — manual, owner-authorized, and read-only.
//
// The synthetic cycle above verifies the rules against data this repository
// invented. This replays the shape of the engagement that motivated the
// capability against a real book: twenty proposed invoices, most of which the
// book already holds, one of them differing in amount.
//
// Two properties make it safe to keep in a public repository:
//
//   * It **never writes.** The proposals are built from the book's own rows,
//     so the "already present" ones are present by construction and no voucher
//     is posted to produce them. That inverts one detail of the original
//     engagement and the assertion says so.
//   * It **emits no book content** — counts, bases and reason codes only. A
//     failure prints what went wrong, never a party name, number or amount.
//     The observation object is filtered to its scalar fields to keep that
//     true: its listed groups carry real voucher numbers, types and GUIDs,
//     and `--nocapture` output reaches terminals and CI logs.
// ---------------------------------------------------------------------------

/// How many faithful copies to propose, how many to perturb, how many to invent.
const REPLAY_PRESENT: usize = 15;
const REPLAY_DIFFERING: usize = 1;
const REPLAY_ABSENT: usize = 4;
/// The engagement's invoice was posted 36.13 short of its source document.
const SHORT_BY_PAISE: i64 = 3_613;
/// A party the book has never seen, so nothing it proposes can resemble a row
/// by party. Fabricated, and it must stay that way.
const REPLAY_UNKNOWN_PARTY: &str = "Bridge Replay Unknown Party";

fn live_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set for the live replay"))
}

/// Replays the twenty-invoice engagement against the live lab.
///
/// ```text
/// BRIDGE_TALLY_LIVE_PORT=9001 \
/// BRIDGE_TALLY_LIVE_COMPANY_GUID=<guid> \
/// BRIDGE_PRESENCE_LIVE_FROM=YYYYMMDD BRIDGE_PRESENCE_LIVE_TO=YYYYMMDD \
/// BRIDGE_PRESENCE_LIVE_MANUAL_TYPES=<comma-separated voucher type names, e.g. "Sales,Purchase"> \
/// cargo test -p bridge --lib replay_the_twenty_invoice_engagement -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "manual owner-authorized live read; needs the lab reachable on the given port"]
async fn replay_the_twenty_invoice_engagement() {
    let port = live_env("BRIDGE_TALLY_LIVE_PORT")
        .parse::<u16>()
        .expect("numeric port");
    let guid = live_env("BRIDGE_TALLY_LIVE_COMPANY_GUID");
    let from = live_env("BRIDGE_PRESENCE_LIVE_FROM");
    let to = live_env("BRIDGE_PRESENCE_LIVE_TO");
    let directory = tempfile::tempdir().expect("directory");
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port,
        },
        data_dir: directory.path().to_path_buf(),
        max_rows: 500,
        max_bytes: 4_000_000,
        redaction: Redaction::None,
        import_enabled: false,
        // Read-only, and stated in the settings rather than only in a comment.
        writes_enabled: false,
    });

    let read = server
        .call_tool(
            "vouchers",
            json!({"company_guid": guid, "from": from, "to": to}),
        )
        .await;
    assert_eq!(read["isError"], false, "the window read failed");
    let rows = read["structuredContent"]["result"]["items"]
        .as_array()
        .expect("items")
        .clone();
    let posted = rows
        .iter()
        .filter(|row| {
            row["cancelled"] != json!(true)
                && row["optional"] != json!(true)
                && row["voucher_number"].is_string()
                && row["party"].is_string()
                && row["amounts"].as_array().is_some_and(|rows| rows.len() > 1)
        })
        .collect::<Vec<_>>();
    let needed = REPLAY_PRESENT + REPLAY_DIFFERING;
    assert!(
        posted.len() >= needed,
        "the window holds {} usable vouchers and the replay needs {needed}; widen the dates",
        posted.len()
    );

    // Paise, so the shortfall is exact. The read carries `bill_allocations`
    // and `is_deemed_positive` that the proposal schema does not declare, so
    // each entry is projected down to what a source document actually offers.
    let paise = |amount: &str| -> i64 {
        let (sign, digits) = match amount.strip_prefix('-') {
            Some(rest) => (-1, rest),
            None => (1, amount),
        };
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
        let fraction = format!("{fraction:0<2}");
        sign * (whole.parse::<i64>().expect("whole") * 100
            + fraction[..2].parse::<i64>().expect("fraction"))
    };
    let rupees = |value: i64| {
        format!(
            "{}{}.{:02}",
            if value < 0 { "-" } else { "" },
            value.abs() / 100,
            value.abs() % 100
        )
    };

    // Magnitude is the sum of the non-negative entries, so a shortfall must
    // come off that side to be a difference at all — and it must not push the
    // entry through zero, or the entry leaves the sum entirely and the
    // magnitude moves by its whole value rather than by the shortfall. The
    // test would still see *a* difference and still pass, measuring something
    // other than what it says it measures. So it is taken off the largest
    // positive entry, and only where that entry can absorb it.
    let widest_positive = |row: &Value| -> Option<usize> {
        row["amounts"]
            .as_array()
            .expect("amounts")
            .iter()
            .enumerate()
            .map(|(at, entry)| (at, paise(entry["amount"].as_str().expect("amount"))))
            .filter(|(_, value)| *value > SHORT_BY_PAISE)
            .max_by_key(|(_, value)| *value)
            .map(|(at, _)| at)
    };

    let proposal_from = |row: &Value, short_by: i64| {
        let target = (short_by > 0).then(|| widest_positive(row).expect("an entry to shorten"));
        let entries = row["amounts"]
            .as_array()
            .expect("amounts")
            .iter()
            .enumerate()
            .map(|(at, entry)| {
                let value = paise(entry["amount"].as_str().expect("amount"));
                let adjusted = if target == Some(at) {
                    value - short_by
                } else {
                    value
                };
                json!({"ledger": entry["ledger"], "amount": rupees(adjusted)})
            })
            .collect::<Vec<_>>();
        json!({
            "date": row["date"],
            "voucher_type": row["voucher_type"],
            "voucher_number": row["voucher_number"],
            "party": row["party"],
            "entries": entries,
        })
    };

    let mut proposals = posted
        .iter()
        .take(REPLAY_PRESENT)
        .map(|row| proposal_from(row, 0))
        .collect::<Vec<_>>();
    // The engagement's short-posted invoice was short in the *book*. This
    // harness may not write, so the shortfall is introduced on the proposal
    // side instead. The difference the report must find is the same one; only
    // which side is missing the GST head is reversed.
    // The row to shorten has to be able to absorb the shortfall. Picking
    // blindly is how the perturbation silently becomes a different one.
    let shortened = posted
        .iter()
        .skip(REPLAY_PRESENT)
        .find(|row| widest_positive(row).is_some())
        .expect("a voucher whose invoice line exceeds the shortfall");
    proposals.push(proposal_from(shortened, SHORT_BY_PAISE));
    // A new customer's invoice, which the engagement also had. It must differ
    // from every book row in *party and amount*, not just in number: in a
    // one-day window every row shares the date, so a known party alone would
    // resemble something on date-and-party and withhold `absent` -- correctly,
    // and that is a property of the window rather than of the proposal.
    for index in 0..REPLAY_ABSENT {
        let mut invented = proposal_from(posted[0], (index as i64 + 1) * 7_777);
        invented["voucher_number"] = json!(format!("BRIDGE-REPLAY-ABSENT-{index:02}"));
        invented["party"] = json!(REPLAY_UNKNOWN_PARTY);
        invented["entries"][0]["ledger"] = json!(REPLAY_UNKNOWN_PARTY);
        proposals.push(invented);
    }

    // The numbering method is an *assertion about the book*, and the harness
    // is not entitled to make it. Declaring an automatically numbered type
    // `manual` would let Tally's own numbers produce `present` and the replay
    // would pass on verdicts the contract says are not identity — evidence
    // manufactured by the test rather than found in the book. So the operator
    // names the manually numbered types and the replay refuses any other.
    let declared = live_env("BRIDGE_PRESENCE_LIVE_MANUAL_TYPES");
    let declared = declared
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .collect::<BTreeSet<_>>();
    // `shortened` is whichever row past the faithful slice could absorb the
    // perturbation, not necessarily `posted[REPLAY_PRESENT]` -- so the types
    // in the declaration have to be read off the rows that actually became
    // proposals (the faithful fifteen plus `shortened`) rather than off the
    // first `needed` rows of `posted`, or a shortfall landing on a later type
    // leaves that type's proposal without a numbering declaration at all.
    let mut types = posted
        .iter()
        .take(REPLAY_PRESENT)
        .chain(std::iter::once(shortened))
        .filter_map(|row| row["voucher_type"].as_str())
        .collect::<Vec<_>>();
    types.sort_unstable();
    types.dedup();
    for kind in &types {
        assert!(
            declared.contains(kind),
            "a voucher type in this window was not declared manually numbered; \
             set BRIDGE_PRESENCE_LIVE_MANUAL_TYPES or narrow the window"
        );
    }
    let numbering = types
        .iter()
        .map(|kind| json!({"voucher_type": kind, "numbering_method": "manual"}))
        .collect::<Vec<_>>();

    let response = server
        .call_tool(
            "voucher_presence",
            json!({"company_guid": guid, "from": from, "to": to,
                "numbering": numbering, "vouchers": proposals}),
        )
        .await;
    let result = &response["structuredContent"]["result"];
    assert_eq!(
        response["isError"], false,
        "presence refused the replay: {}",
        result["error"]["code"]
    );

    let items = result["items"].as_array().expect("items");
    // Counts, bases and reason codes only: never a name, number or amount.
    let summarise = |entry: &Value| {
        format!(
            "{}/{}/{}",
            entry["presence"].as_str().unwrap_or("?"),
            entry["basis"].as_str().unwrap_or("-"),
            entry["reason"].as_str().unwrap_or("-")
        )
    };
    println!(
        "replay totals: {} | verdicts: {:?}",
        result["totals"],
        items.iter().map(summarise).collect::<Vec<_>>()
    );
    // Scalars only, by construction rather than by intention. `book` also
    // carries `duplicate_numbers` and `unbalanced_vouchers`, and those hold
    // real voucher numbers, voucher types and GUIDs -- printing the object
    // whole would put customer accounting data into a terminal or a CI log,
    // which is exactly what the header above promises this does not do. A
    // promise a reader has to check the code to trust is not a promise.
    let counts = result["book"]
        .as_object()
        .expect("book")
        .iter()
        .filter(|(_, value)| !value.is_array())
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    println!("book observations: {}", counts.join(" "));

    for (index, entry) in items.iter().take(REPLAY_PRESENT).enumerate() {
        assert_eq!(
            entry["presence"],
            "present",
            "faithful copy {index} came back {}",
            summarise(entry)
        );
        assert!(
            entry["differences"]
                .as_array()
                .is_some_and(|rows| rows.is_empty()),
            "a faithful copy reported a difference at {index}"
        );
    }
    let differing = &items[REPLAY_PRESENT];
    assert_eq!(
        differing["presence"],
        "present",
        "the short proposal came back {}",
        summarise(differing)
    );
    // The *exact* shortfall, not merely "some difference". Asserting only
    // that a difference exists is what let the perturbation drift into
    // something else while the test went on passing.
    let amount = differing["differences"]
        .as_array()
        .expect("differences")
        .iter()
        .find(|difference| difference["field"] == "amount")
        .expect("an amount difference");
    let proposed = paise(amount["proposed"].as_str().expect("proposed"));
    let observed = paise(amount["observed"].as_str().expect("observed"));
    assert_eq!(
        observed - proposed,
        SHORT_BY_PAISE,
        "the reported shortfall is not the one the proposal applied"
    );
    for (index, entry) in items.iter().skip(needed).enumerate() {
        assert_eq!(
            entry["presence"],
            "absent",
            "invented voucher {index} came back {}",
            summarise(entry)
        );
    }
}

// ---------------------------------------------------------------------------
// ADR 0018 — reading the narration marker.
//
// These are the adapter's half of the basis. The crate never parses a marker;
// everything that decides what counts as one is here, so this is where it has
// to be pinned down.
// ---------------------------------------------------------------------------

/// A batch id has the shape `render_import_xml` generates for one.
const BATCH: &str = "bridge-2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01";

fn narration_with(marker: &str) -> String {
    format!(
        "Invoice for the month {}{marker}]",
        agent_import::NARRATION_MARKER_PREFIX
    )
}

/// The one property the whole basis rests on: what the reader accepts is
/// exactly what the writer writes. If these two ever disagree, presence reports
/// every voucher Bridge imported as absent and a caller duplicates all of them.
#[test]
fn the_reader_accepts_exactly_what_the_writer_derives() {
    let identity = agent_import::import_identity(BATCH, "txn-001").to_string();
    let narration = narration_with(&identity);
    assert_eq!(
        observed_marker(Some(&narration)),
        ObservedMarker::Identifying(identity.as_str())
    );
    // And the derivation is a function of both halves, not of the label alone.
    assert_ne!(
        identity,
        agent_import::import_identity("bridge-other", "txn-001").to_string(),
        "the batch is what makes a reused caller label distinct"
    );
}

/// The safety property of ADR 0018 §3. An older scheme wrote the caller's
/// transaction label into the narration, and those labels are reused across
/// batches; matching one would pair a proposal with an unrelated voucher from
/// an unrelated import and drop an invoice without a trace.
#[test]
fn a_legacy_caller_label_is_never_an_identity() {
    for label in ["txn-001", "INV-2026-0001", "batch1_txn1"] {
        assert_eq!(
            observed_marker(Some(&narration_with(label))),
            ObservedMarker::Unidentified(&[]),
            "{label} is a caller label, not a batch-derived identity"
        );
    }
    // Nor is a canonical UUID of some *other* version. `valid_txn_id` admits
    // hex and hyphens, so a legacy-scheme write could legally have put a v4
    // UUID in a narration; only the version the writer stamps can have come
    // from the writer.
    assert_eq!(
        observed_marker(Some(&narration_with(
            "550e8400-e29b-41d4-a716-446655440000"
        ))),
        ObservedMarker::Unidentified(&[]),
        "a canonical v4 UUID is not something import_identity can emit"
    );

    // Nor is a UUID spelled some other way than the writer spells it.
    let identity = agent_import::import_identity(BATCH, "txn-001").to_string();
    for spelling in [
        identity.replace('-', ""),
        identity.to_ascii_uppercase(),
        format!("urn:uuid:{identity}"),
    ] {
        assert_eq!(
            observed_marker(Some(&narration_with(&spelling))),
            ObservedMarker::Unidentified(&[]),
            "only the canonical form can have come from the writer"
        );
    }
}

/// Two markers mean the voucher claims two imports, and a malformed one cannot
/// name any. Both are still Bridge writes, so neither reads as `Absent`.
#[test]
fn an_ambiguous_or_malformed_marker_is_a_bridge_write_without_a_name() {
    let identity = agent_import::import_identity(BATCH, "txn-001").to_string();
    let other = agent_import::import_identity(BATCH, "txn-002").to_string();
    let prefix = agent_import::NARRATION_MARKER_PREFIX;
    for narration in [
        format!("{prefix}{identity}] {prefix}{other}]"),
        format!("{prefix}{identity}"),
        format!("{prefix}]"),
        format!("{prefix}{identity} with a space]"),
    ] {
        assert_eq!(
            observed_marker(Some(&narration)),
            ObservedMarker::Unidentified(&[]),
            "narration {narration:?}"
        );
    }
    // A narration Bridge never touched is a different fact from one it did.
    assert_eq!(
        observed_marker(Some("Cheque deposited at the branch")),
        ObservedMarker::Absent
    );
    assert_eq!(observed_marker(None), ObservedMarker::Absent);
}

/// Half an import identity is a caller error, not something to quietly drop:
/// ignoring it would skip the strongest key this proposal has and let an
/// `absent` stand on a comparison that never ran.
#[tokio::test]
async fn an_import_identity_must_be_supplied_whole() {
    let entries =
        json!([{"ledger":"Cash","amount":"-1.00"},{"ledger":"WR2 Sales","amount":"1.00"}]);
    let numbering = json!([{"voucher_type":"Journal","numbering_method":"manual"}]);
    let directory = tempfile::tempdir().expect("directory");
    let server = offline_server(directory.path());
    for (half, refused) in [
        (json!({"batch_id": BATCH}), true),
        (json!({"bridge_txn_id": "txn-001"}), true),
        (
            json!({"batch_id": BATCH, "bridge_txn_id": "txn-001"}),
            false,
        ),
        (json!({}), false),
    ] {
        let mut voucher = json!({"date":"20260901","voucher_type":"Journal","entries":entries});
        for (key, value) in half.as_object().expect("object") {
            voucher[key] = value.clone();
        }
        let response = server
            .call_tool_response(
                "voucher_presence",
                json!({"company_guid":GUID,"from":"20260901","to":"20260930",
                    "numbering":numbering,"vouchers":[voucher]}),
            )
            .await;
        let code = response.value["structuredContent"]["result"]["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            code == "presence_import_identity_incomplete",
            refused,
            "half {half} was not treated as {}",
            if refused { "an error" } else { "acceptable" }
        );
    }
}

/// The admission contract grew two properties, and both have to stay bounded.
/// Widening either is how a caller reaches a marker it chose rather than one
/// the writer derived.
#[test]
fn the_import_identity_inputs_are_bounded_where_they_are_published() {
    let definitions = tool_definitions(true, false);
    let voucher = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .expect("voucher_presence tool")["inputSchema"]["properties"]["vouchers"]["items"]
        .clone();
    assert_eq!(voucher["additionalProperties"], json!(false));
    for key in ["batch_id", "bridge_txn_id"] {
        assert_eq!(
            voucher["properties"][key]["maxLength"],
            json!(64),
            "{key} is unbounded"
        );
        assert_eq!(voucher["properties"][key]["minLength"], json!(1));
    }
    // The transaction label's alphabet is the writer's, so a caller cannot
    // smuggle a shape the derivation never produces. Declaring it is not
    // enforcing it -- see the test below, which is the one that matters.
    assert_eq!(
        voucher["properties"]["bridge_txn_id"]["pattern"],
        json!("^[A-Za-z0-9_-]+$")
    );
    // Neither is required: a proposal that supplies no import identity behaves
    // exactly as it did before ADR 0018.
    assert_eq!(
        voucher["required"],
        json!(["date", "voucher_type", "entries"])
    );
    // And a marker still cannot be handed over directly.
    assert!(voucher["properties"].get("narration_marker").is_none());
    assert!(voucher["properties"].get("remote_id").is_none());
}

/// A batch id the writer could not have generated cannot have written a
/// marker, so deriving one from it yields an identity no book holds. Left
/// unchecked that is not a harmless miss: under automatic numbering, with
/// nothing resembling the proposal, the window reports `absent` and a caller
/// imports a second copy. A mistyped argument must say it is a mistyped
/// argument. Against the live simulator, so zero bytes means the refusal came
/// before the reads.
#[tokio::test]
async fn a_batch_id_the_writer_could_not_have_made_is_refused() {
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
    for (batch, refused) in [
        (BATCH, false),
        // Plausible, nonblank, within bounds, and not a shape the writer emits.
        ("bridge-not-a-uuid", true),
        ("2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01", true),
        ("bridge-2B1C9F4E-9D3A-4F71-8C2E-5A6B7C8D9E01", true),
        // Canonically spelled and RFC 4122 variant, but the wrong version:
        // `Uuid::new_v4()` never emits a nil or a v7 UUID, so hashing either
        // would derive an identity no book holds and read as `absent`.
        ("bridge-00000000-0000-0000-0000-000000000000", true),
        ("bridge-017f22e2-79b0-7cc3-98c4-dc0c0c07398f", true),
    ] {
        let mut voucher = proposal("JV-1", "Bridge Nested Debtor WR4", "12.50");
        voucher["batch_id"] = json!(batch);
        voucher["bridge_txn_id"] = json!("txn-001");
        let response = server
            .call_tool_response(
                "voucher_presence",
                json!({"company_guid": CAPTURED_GUID, "from":"20260901", "to":"20260930",
                    "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                    "vouchers":[voucher]}),
            )
            .await;
        let code = response.value["structuredContent"]["result"]["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            code == "argument_invalid:batch_id",
            refused,
            "batch id {batch:?} produced {code:?}"
        );
        if refused {
            assert_eq!(
                response.value["structuredContent"]["evidence"]["bytes"], 0,
                "a batch id the writer could not have made must cost no read"
            );
        }
    }
}

/// A declared pattern is documentation until something evaluates it, and the
/// shared validator evaluates exactly one: the `\S` special case. Every other
/// regular expression in a published schema is inert.
///
/// So the transaction label's alphabet is enforced with the writer's own
/// `valid_txn_id`. A label `build_import_xml` would refuse cannot have
/// produced a narration marker; deriving one anyway yields an identity no book
/// can hold, and on an empty window that reads as `absent` rather than as the
/// input error it is. Against the live simulator, so zero bytes means the
/// refusal really did come before the reads.
#[tokio::test]
async fn a_transaction_label_the_writer_would_refuse_is_refused_here() {
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
    // A space is the case the writer rejects and the declared pattern names.
    for (label, refused) in [("txn 001", true), ("txn-001", false)] {
        let mut voucher = proposal("JV-1", "Bridge Nested Debtor WR4", "12.50");
        voucher["batch_id"] = json!("bridge-2b1c9f4e-9d3a-4f71-8c2e-5a6b7c8d9e01");
        voucher["bridge_txn_id"] = json!(label);
        let response = server
            .call_tool_response(
                "voucher_presence",
                json!({"company_guid": CAPTURED_GUID, "from":"20260901", "to":"20260930",
                    "numbering":[{"voucher_type":"Journal","numbering_method":"manual"}],
                    "vouchers":[voucher]}),
            )
            .await;
        let code = response.value["structuredContent"]["result"]["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            code == "argument_invalid:bridge_txn_id",
            refused,
            "label {label:?} produced {code:?}"
        );
        if refused {
            assert_eq!(
                response.value["structuredContent"]["evidence"]["bytes"], 0,
                "a label the writer would refuse must cost no read"
            );
        }
    }
}
