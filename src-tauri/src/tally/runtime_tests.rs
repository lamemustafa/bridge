use super::*;
use crate::commands::VerifiedCompanyIdentity;
use crate::tally::TallyProduct;
use anyhow::Context;
use bridge_tally_core::CapabilityProfile;
use std::collections::BTreeMap;
use tally_protocol_simulator::Fixture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn verified_identity(name: &str, guid: &str) -> VerifiedCompanyIdentity {
    VerifiedCompanyIdentity::test_fixture(name, guid)
}

fn captured_aarav_company_list_and_identity() -> (String, VerifiedCompanyIdentity) {
    let bytes = include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml");
    let xml = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
        bytes,
        "text/xml; charset=utf-16",
        bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
        bytes.len(),
    )
    .expect("captured Company collection decodes")
    .text;
    let rows = parse_companies_from_collection(&xml).unwrap();
    let row = rows
        .iter()
        .find(|row| row.name == "Aarav Trading Company Demo")
        .unwrap();
    let identity = VerifiedCompanyIdentity::from_observed_companies(
        row.name.clone(),
        row.guid.clone().unwrap(),
        row.company_number.clone().unwrap(),
        row.books_from.clone().unwrap(),
        &rows,
    )
    .unwrap();
    (xml, identity)
}

#[test]
fn agent_company_list_evidence_hashes_the_encoded_utf16_response_bytes() {
    let response = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"Book\"><GUID>g-1</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>".to_string();
    let encoded = bridge_tally_protocol::encode_tally_xml_request_utf16le(&response);
    let expected_bytes = encoded.len();
    let expected_sha = Sha256::digest(&encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let listed = agent_company_list_from_response(response, expected_bytes, expected_sha.clone())
        .expect("company list");
    assert_eq!(listed.response_bytes, expected_bytes);
    assert_eq!(listed.response_sha256, expected_sha);
    assert_eq!(listed.companies.len(), 1);
}

#[test]
fn party_ledger_master_evidence_includes_currency_probe_and_source_responses() {
    let source = |master_response_sha256: &str| PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").expect("source from"),
        to: TallyDate::parse("20260731").expect("source to"),
        rows: Vec::new(),
        request_sha256: "0".repeat(64),
        master_response_sha256: master_response_sha256.to_string(),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 11,
        balance_response_bytes: 13,
        group_response_bytes: 17,
        groups: Vec::new(),
    };
    let currency = RuntimeReadEvidence::paired("<currency/>", sha256_hex(b"currency-response"), 19);
    let baseline = TallyRuntime::party_ledger_master_source_evidence(
        &source(&"a".repeat(64)),
        currency.clone(),
    );
    let changed = TallyRuntime::party_ledger_master_source_evidence(
        &source(&"d".repeat(64)),
        currency.clone(),
    );
    let changed_currency = TallyRuntime::party_ledger_master_source_evidence(
        &source(&"a".repeat(64)),
        RuntimeReadEvidence::paired("<currency/>", sha256_hex(b"changed-currency-response"), 23),
    );
    let source_scope = "0".repeat(64);
    let source_responses =
        sha256_hex(format!("{}:{}:{}", "a".repeat(64), "b".repeat(64), "c".repeat(64)).as_bytes());
    assert_eq!(
        baseline.request_sha256,
        sha256_hex(format!("{}:{source_scope}", currency.request_sha256).as_bytes())
    );
    assert_eq!(
        baseline.response_sha256,
        sha256_hex(format!("{}:{source_responses}", currency.response_sha256).as_bytes())
    );
    assert_eq!(baseline.request_sha256, changed_currency.request_sha256);
    assert_ne!(baseline.response_sha256, changed_currency.response_sha256);
    assert_eq!(changed_currency.bytes, (11 + 13 + 17 + 23) * 2);

    assert_eq!(baseline.request_sha256, changed.request_sha256);
    assert_ne!(baseline.response_sha256, changed.response_sha256);
    assert_eq!(baseline.bytes, (11 + 13 + 17 + 19) * 2);
}

#[test]
fn paired_evidence_commits_to_utf16le_request_body_not_utf8_source() {
    let request = "<ENVELOPE>₹ &amp; Co</ENVELOPE>";
    let mut encoded = vec![0xff, 0xfe];
    encoded.extend(request.encode_utf16().flat_map(u16::to_le_bytes));
    let evidence = RuntimeReadEvidence::paired(request, "r".repeat(64), 7);
    assert_eq!(evidence.request_sha256, sha256_hex(&encoded));
    assert_ne!(evidence.request_sha256, sha256_hex(request.as_bytes()));
    assert_eq!(evidence.bytes, 14);
}

#[test]
fn outstandings_wire_evidence_changes_with_any_native_report_response() {
    let receivable = RuntimeReadEvidence::paired("<receivable/>", sha256_hex(b"r1"), 3);
    let payable = RuntimeReadEvidence::paired("<payable/>", sha256_hex(b"p1"), 5);
    let baseline = receivable.clone().combine(payable.clone());
    let changed = receivable.combine(RuntimeReadEvidence::paired(
        "<payable/>",
        sha256_hex(b"p2"),
        5,
    ));

    assert_eq!(baseline.bytes, 16, "both paired responses are counted");
    assert_ne!(baseline.response_sha256, changed.response_sha256);
    assert_ne!(baseline.request_sha256, String::new());
}

fn utf16_xml_response(body: impl AsRef<str>) -> Vec<u8> {
    let body = bridge_tally_protocol::encode_tally_xml_request_utf16le(body.as_ref());
    utf16_xml_response_bytes(&body)
}

fn utf16_xml_response_bytes(body: &[u8]) -> Vec<u8> {
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-16\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), body].concat()
}

fn utf8_status_response(body: impl AsRef<str>) -> Vec<u8> {
    let body = body.as_ref().as_bytes();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), body].concat()
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let (header_end, content_length) = loop {
        let read = socket.read(&mut buffer).await.expect("read request");
        assert!(read > 0, "request ended before headers completed");
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let header =
            std::str::from_utf8(&request[..header_end]).expect("request headers are valid UTF-8");
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then_some(value.trim())
            })
            .map(|value| {
                value
                    .parse::<usize>()
                    .expect("request Content-Length is numeric")
            })
            .unwrap_or(0);
        break (header_end + 4, content_length);
    };
    while request.len() < header_end + content_length {
        let read = socket.read(&mut buffer).await.expect("read request body");
        assert!(read > 0, "request ended before declared body completed");
        request.extend_from_slice(&buffer[..read]);
    }
    request
}

#[test]
fn ageing_anchor_serializes_as_an_explicit_wire_contract() {
    assert_eq!(
        serde_json::to_value(OutstandingsAgeingAnchor::DueDate).unwrap(),
        serde_json::json!("due_date")
    );
    assert_eq!(
        serde_json::to_value(OutstandingsAgeingAnchor::BillDate).unwrap(),
        serde_json::json!("bill_date")
    );
}

#[test]
fn outstandings_read_strategy_serializes_as_an_explicit_wire_contract() {
    assert_eq!(
        serde_json::to_value(OutstandingsReadStrategy::NativeBills).unwrap(),
        serde_json::json!("native_bills")
    );
    assert_eq!(
        serde_json::to_value(OutstandingsReadStrategy::VoucherScan).unwrap(),
        serde_json::json!("voucher_scan")
    );
}

#[test]
fn single_company_forex_ledger_capture_returns_a_typed_partial() {
    const FOREX_LEDGER_CAPTURE: &[u8] = include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/ledgers_forex_composite_live.utf16le.xml"
    );
    let ledger_body = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
        FOREX_LEDGER_CAPTURE,
        "text/xml; charset=utf-16",
        bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
        FOREX_LEDGER_CAPTURE.len(),
    )
    .expect("captured forex ledger response decodes")
    .text;
    let admitted = admit_native_ledger_snapshot(
        parse_native_ledger_snapshot(&ledger_body)
            .map_err(anyhow::Error::from)
            .context("stable native ledger snapshot"),
    )
    .expect("a foreign-currency ledger is an in-band partial");

    assert!(matches!(
        admitted,
        NativeLedgerSnapshotAdmission::Partial(OutstandingsLoadResult::Partial { reason, .. })
            if reason.reason_code == "company_foreign_currency_ledger_balance"
                && reason.foreign_currency_ledger_name.as_deref() == Some("FX USD Debtor 02")
    ));
}

#[tokio::test]
async fn single_company_read_returns_the_forex_capture_partial() {
    const EXTENT: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    );
    const FOREX_LEDGER_CAPTURE: &[u8] = include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/ledgers_forex_composite_live.utf16le.xml"
    );
    const RECEIVABLE: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_aarav.xml"
    );
    const PAYABLE: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_payable_aarav.xml"
    );
    const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    let (company_list, identity) = captured_aarav_company_list_and_identity();

    let extent = EXTENT;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic outstandings server");
    let address = listener.local_addr().expect("synthetic server address");
    let server = tokio::spawn(async move {
        let mut source_post_index = 0;
        let mut company_post_index = 0;
        for index in 0..30 {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                    .await
                    .expect("outstandings request timed out")
                    .expect("accept outstandings request");
            let request = read_http_request(&mut socket).await;
            let response = if request.starts_with(b"GET /status") {
                utf8_status_response(STATUS)
            } else {
                assert!(request.starts_with(b"POST /"), "request {index}");
                let body_start = request
                    .windows(4)
                    .position(|bytes| bytes == b"\r\n\r\n")
                    .expect("complete request headers")
                    + 4;
                let xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
                    &request[body_start..],
                    request.len() - body_start,
                )
                .expect("decode request")
                .text;
                if xml.contains("<ID>BridgeCompanyExtent</ID>")
                    && !xml.contains("<SVCURRENTCOMPANY>")
                {
                    company_post_index += 1;
                    utf16_xml_response(&company_list)
                } else {
                    let response = match source_post_index {
                        0 | 1 | 10 | 11 => utf16_xml_response(extent),
                        2 | 3 => utf16_xml_response(RECEIVABLE),
                        6 | 7 => utf16_xml_response(PAYABLE),
                        _ => utf16_xml_response_bytes(FOREX_LEDGER_CAPTURE),
                    };
                    source_post_index += 1;
                    response
                }
            };
            socket.write_all(&response).await.expect("write response");
        }
        assert_eq!(company_post_index, 4);
        assert_eq!(source_post_index, 12);
    });

    let result = TallyRuntime::default()
        .fetch_outstandings(
            TallyConfig {
                host: address.ip().to_string(),
                port: address.port(),
            },
            &identity,
            TallyDate::parse("20260401").expect("captured book as-of"),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await
        .expect("foreign-currency capture returns an in-band partial");

    assert!(matches!(
        result,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "company_foreign_currency_ledger_balance"
                && reason.foreign_currency_ledger_name.as_deref() == Some("FX USD Debtor 02")
    ));
    server.await.expect("synthetic outstandings server task");
}

#[test]
fn validation_lab_future_bill_stays_unaged_in_open_bill_output() {
    let receivable = parse_native_bill_rows(
        include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_validation_lab.xml"
        ),
        &TallyDate::parse("20250401").expect("captured BooksFrom"),
        &TallyDate::parse("20260817").expect("capture as-of"),
    )
    .expect("captured validation-book rows parse");
    let rows = all_open_bill_rows(
        &receivable,
        &[],
        OutstandingsAgeingAnchor::DueDate,
        &TallyDate::parse("20260817").expect("capture as-of"),
    );
    let future = rows
        .iter()
        .find(|row| row.reference == "ALPHA-FUTURE")
        .expect("captured future-due bill remains present");
    assert_eq!(future.amount.as_str(), "22222.00");
    assert_eq!(future.age_days, None);

    let bill_date_rows = all_open_bill_rows(
        &receivable,
        &[],
        OutstandingsAgeingAnchor::BillDate,
        &TallyDate::parse("20260817").expect("capture as-of"),
    );
    let bill_date_future = bill_date_rows
        .iter()
        .find(|row| row.reference == "ALPHA-FUTURE")
        .expect("captured future-due bill remains present for bill-date ageing");
    assert!(
        bill_date_future.age_days.is_some(),
        "the selected bill-date basis must not reuse the future due date"
    );
}

#[test]
fn settled_native_bill_rows_do_not_reach_statement_or_export_sources() {
    let books_from = TallyDate::parse("20250401").expect("synthetic BooksFrom");
    let as_of = TallyDate::parse("20260817").expect("synthetic as-of");
    let parsed = parse_native_bill_rows(
        "<ENVELOPE><BILLFIXED><BILLDATE>1-Aug-26</BILLDATE><BILLREF>SETTLED</BILLREF>\
             <BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED><BILLCL>0.00</BILLCL>\
             <BILLDUE>1-Aug-26</BILLDUE><BILLOVERDUE>16</BILLOVERDUE></ENVELOPE>",
        &books_from,
        &as_of,
    )
    .expect("synthetic settled row parses at the wire boundary");

    assert_eq!(parsed.len(), 1, "the fixture proves the native row existed");
    assert!(
        all_open_bill_rows(&parsed, &[], OutstandingsAgeingAnchor::DueDate, &as_of).is_empty(),
        "a zero closing balance is not an open bill"
    );
}

#[test]
fn captured_validation_lab_bytes_render_a_complete_working_paper_end_to_end() {
    const COMPANY: &str = "Bridge Validation Lab";
    const COMPANY_GUID: &str = "c6afd306-00e1-4f51-802a-babe44daddd3";
    let books_from = TallyDate::parse("20250401").expect("captured BooksFrom");
    let as_of = TallyDate::parse("20260801").expect("captured as-of date");
    let receivable_xml = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_validation_lab.xml"
    );
    let payable_xml = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_payable_validation_lab.xml"
    );
    let groups_xml = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_validation_lab.xml"
    );
    let ledgers_xml = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/ledger_snapshot_validation_lab.xml"
    );

    let receivable = parse_native_bill_rows(receivable_xml, &books_from, &as_of)
        .expect("captured receivable rows parse");
    let payable = parse_native_bill_rows(payable_xml, &books_from, &as_of)
        .expect("captured payable rows parse");
    // This captured Group body predates the request's response-bound
    // compute. Add only that collection context for the parser contract;
    // source-byte accounting below remains the exact captured response.
    let groups_with_response_company_guid = groups_xml.replace(
        "</GUID>",
        "</GUID><BRIDGECOMPANYGUID>c6afd306-00e1-4f51-802a-babe44daddd3</BRIDGECOMPANYGUID>",
    );
    let groups = parse_native_group_snapshot(&groups_with_response_company_guid, COMPANY_GUID)
        .expect("captured group identity and ancestry parse");
    let ledgers =
        parse_native_ledger_snapshot(ledgers_xml).expect("captured ledger controls parse");
    let source_bytes = receivable_xml
        .len()
        .checked_add(payable_xml.len())
        .and_then(|value| value.checked_add(groups_xml.len()))
        .and_then(|value| value.checked_add(ledgers_xml.len()))
        .expect("captured source byte count fits usize");
    let computed = compute_native_outstandings(
        COMPANY,
        &receivable,
        &payable,
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        NativeAgeingAnchor::DueDate,
        &as_of,
        source_bytes,
    )
    .expect("captured native controls compute");
    assert_eq!(
        native_crosscheck_partial_reason(&computed, &as_of),
        None,
        "captured overdue counters must positively prove the requested date"
    );

    let statement_open_bills = all_open_bill_rows(
        &receivable,
        &payable,
        OutstandingsAgeingAnchor::DueDate,
        &as_of,
    );
    assert_eq!(statement_open_bills.len(), 6);
    assert!(statement_open_bills.iter().all(|row| !row.amount.is_zero()));
    let statement_unallocated_by_party = all_unallocated_parties(&computed.residuals);
    let result = OutstandingsLoadResult::Complete {
        report: Box::new(computed.report),
        read_strategy: OutstandingsReadStrategy::NativeBills,
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        ageing_anchor: OutstandingsAgeingAnchor::DueDate,
        synced_at_unix_ms: 1_777_000_000_000,
        unallocated_total: Some(computed.residual_total),
        statement_unallocated_by_party,
        statement_open_bills,
    };
    let source = crate::reports::outstandings_working_paper_store::source_from_complete_result(
        &result,
        COMPANY_GUID,
    )
    .expect("captured source stays inside export budgets")
    .expect("captured complete result substantiates a source");
    assert_eq!(source.source_bytes, 33_575);
    assert_eq!(source.open_bills.len(), 6);
    let paper =
        crate::reports::outstandings_working_paper::build_outstandings_working_paper(source)
            .expect("captured exact controls reconcile into a working paper");
    let workbook =
        crate::reports::outstandings_working_paper_xlsx::render_outstandings_working_paper_xlsx(
            &paper,
        )
        .expect("captured working paper renders");
    assert!(workbook.len() > 200);
    assert_eq!(&workbook[0..2], b"PK");
}

#[test]
fn raw_overdue_disagreement_withholds_native_complete() {
    let rows = parse_native_bill_rows(
        "<ENVELOPE><BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>MISMATCH</BILLREF>\
             <BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED><BILLCL>-100.00</BILLCL>\
             <BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>31</BILLOVERDUE></ENVELOPE>",
        &TallyDate::parse("20250401").expect("synthetic BooksFrom"),
        &TallyDate::parse("20260817").expect("synthetic as-of"),
    )
    .expect("raw bill parses");
    let computed = compute_native_outstandings(
        "Synthetic Company",
        &rows,
        &[],
        NativeMasterSnapshot {
            ledgers: &[],
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        NativeAgeingAnchor::DueDate,
        &TallyDate::parse("20260817").expect("synthetic as-of"),
        0,
    )
    .expect("arithmetic still computes for diagnostic comparison");
    assert_eq!(
        computed.overdue_crosscheck,
        NativeOverdueCrosscheck::Inconsistent
    );
    assert_eq!(
        native_crosscheck_partial_reason(&computed, &TallyDate::parse("20260817").unwrap()),
        Some(OutstandingsPartialReason::code(
            "native_overdue_crosscheck_mismatch"
        ))
    );
}

#[test]
fn zero_bill_rows_with_a_ledger_residual_withhold_native_complete() {
    let groups = parse_native_group_snapshot(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
            <GROUP NAME="Sundry Debtors" RESERVEDNAME="Sundry Debtors"><GUID>11111111-1111-1111-1111-111111111111-00000001</GUID><BRIDGECOMPANYGUID>11111111-1111-1111-1111-111111111111</BRIDGECOMPANYGUID><PARENT>Primary</PARENT></GROUP>
            </COLLECTION></DATA></BODY></ENVELOPE>"#,
        "11111111-1111-1111-1111-111111111111",
    )
    .expect("synthetic group snapshot parses");
    let ledgers = parse_native_ledger_snapshot(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>
            <LEDGER NAME="Synthetic Customer"><PARENT>Sundry Debtors</PARENT><CLOSINGBALANCE>-100.00</CLOSINGBALANCE><OPENINGBALANCE>0</OPENINGBALANCE><ISBILLWISEON>No</ISBILLWISEON></LEDGER>
            </COLLECTION></DATA></BODY></ENVELOPE>"#,
    )
    .expect("synthetic ledger snapshot parses");
    let requested_as_of = TallyDate::parse("20260817").expect("synthetic as-of");
    let computed = compute_native_outstandings(
        "Synthetic Company",
        &[],
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::Complete(&groups),
        },
        NativeAgeingAnchor::DueDate,
        &requested_as_of,
        0,
    )
    .expect("the residual is a partial-result diagnostic, not a parser failure");

    assert_eq!(
        computed.overdue_crosscheck,
        NativeOverdueCrosscheck::UnconfirmedAsOfWithoutBillReferences
    );
    assert_eq!(
        native_crosscheck_partial_reason(&computed, &requested_as_of),
        Some(OutstandingsPartialReason::code(
            WarningCode::NativeOutstandingsAsOfUnconfirmedWithoutBillReferences.as_str(),
        ))
    );
}

#[test]
fn raw_empty_bill_references_preserve_each_amount_and_get_explicit_label() {
    let raw_bills = "<ENVELOPE>\
            <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF></BILLREF><BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED>\
            <BILLCL>-40.00</BILLCL><BILLDUE>1-Jul-26</BILLDUE><BILLOVERDUE>30</BILLOVERDUE>\
            <BILLFIXED><BILLDATE>2-Jul-26</BILLDATE><BILLREF> \n\t </BILLREF><BILLPARTY>Synthetic Customer</BILLPARTY></BILLFIXED>\
            <BILLCL>-60.00</BILLCL><BILLDUE>2-Jul-26</BILLDUE><BILLOVERDUE>29</BILLOVERDUE>\
            </ENVELOPE>";
    let as_of = TallyDate::parse("20260731").expect("synthetic as-of");
    let parsed = parse_native_bill_rows(
        raw_bills,
        &TallyDate::parse("20250401").expect("synthetic BooksFrom"),
        &as_of,
    )
    .expect("paired empty BILLREF values remain parseable");
    let rows = all_open_bill_rows(&parsed, &[], OutstandingsAgeingAnchor::DueDate, &as_of);

    assert_eq!(rows.len(), 2, "empty identities must not collapse rows");
    let total = rows
        .iter()
        .try_fold(ExactDecimal::zero(), |sum, row| {
            sum.checked_add(&row.amount)
        })
        .expect("synthetic bill total remains exact");
    assert_eq!(total.as_str(), "100", "neither amount may be lost");
    assert_eq!(
        rows.iter()
            .map(|row| row.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["No reference reported", "No reference reported"],
        "client-facing rows must disclose the missing identity",
    );
}

#[test]
fn party_master_currency_assertion_rejects_a_changed_company_extent() {
    const EXTENT: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_company_extent_live.xml"
    );
    let read_extent_xml = EXTENT.replacen(
        r#"<GUID TYPE=\"String\">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>"#,
        r#"<GUID TYPE=\"String\">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID><ALTMSTID TYPE=\"Number\">1</ALTMSTID>"#,
        1,
    );
    let changed_extent_xml = read_extent_xml.replace(
        "<LASTVOUCHERDATE TYPE=\"Date\">20260401</LASTVOUCHERDATE>",
        "<LASTVOUCHERDATE TYPE=\"Date\">20260402</LASTVOUCHERDATE>",
    );
    let read_extent = bridge_tally_protocol::outstandings_shared::parse_company_book_extent(
        &read_extent_xml,
        "Aarav Trading Company Demo",
        "bb8ad19e-6aef-4239-a917-87fec0c6215e",
    )
    .expect("captured extent parses");
    let changed_extent = bridge_tally_protocol::outstandings_shared::parse_company_book_extent(
        &changed_extent_xml,
        "Aarav Trading Company Demo",
        "bb8ad19e-6aef-4239-a917-87fec0c6215e",
    )
    .expect("changed synthetic extent parses");
    let assertion = CompanyCurrencyRead {
        currency: CompanyCurrency {
            symbol: "₹".to_string(),
            mailing_name: "INR".to_string(),
            currency_count: 1,
            decimal_places: 2,
            is_inr: true,
        },
        extent: read_extent.clone(),
        evidence: RuntimeReadEvidence::empty(),
    }
    .bind_party_ledger_master_assertion(OutstandingsCurrencyAssertion::Inr);

    assert_eq!(
        assertion
            .require_opening_extent(&read_extent)
            .expect("the observed extent releases INR"),
        PartyLedgerMasterCurrency {
            assertion: OutstandingsCurrencyAssertion::Inr,
            decimal_places: 2,
        }
    );
    let error = assertion
        .require_opening_extent(&changed_extent)
        .expect_err("a changed extent must not release INR");
    assert!(matches!(
        error.downcast_ref::<PairedReadValidationError>(),
        Some(PairedReadValidationError::CurrencyToMasterExtent)
    ));
}

#[tokio::test]
async fn detect_base_currency_rejects_book_drift_after_the_currency_read() {
    const EXTENT: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    );
    const CURRENCY: &[u8] = include_bytes!(
        "../../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    );
    const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    let (company_list, identity) = captured_aarav_company_list_and_identity();

    let currency = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
        CURRENCY,
        "text/xml; charset=utf-16",
        bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
        CURRENCY.len(),
    )
    .expect("captured currency response decodes")
    .text;

    let opening_extent = EXTENT;
    let closing_extent = opening_extent.replace(
        "<LASTVOUCHERDATE TYPE=\"Date\">20260401</LASTVOUCHERDATE>",
        "<LASTVOUCHERDATE TYPE=\"Date\">20260402</LASTVOUCHERDATE>",
    );
    assert_ne!(
        closing_extent, opening_extent,
        "the drift mutation must apply"
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic currency server");
    let address = listener.local_addr().expect("synthetic server address");
    let server = tokio::spawn(async move {
        let mut source_post_index = 0;
        for index in 0..13 {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
                    .await
                    .expect("currency request timed out")
                    .expect("accept currency request");
            let request = read_http_request(&mut socket).await;
            let response = if request.starts_with(b"GET /status") {
                utf8_status_response(STATUS)
            } else {
                assert!(request.starts_with(b"POST /"), "request {index}");
                let body_start = request
                    .windows(4)
                    .position(|bytes| bytes == b"\r\n\r\n")
                    .expect("complete request headers")
                    + 4;
                let xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
                    &request[body_start..],
                    request.len() - body_start,
                )
                .expect("decode request")
                .text;
                if xml.contains("<ID>BridgeCompanyExtent</ID>")
                    && !xml.contains("<SVCURRENTCOMPANY>")
                {
                    utf16_xml_response(&company_list)
                } else {
                    let response = match source_post_index {
                        0 | 1 => utf16_xml_response(opening_extent),
                        2 | 3 => utf16_xml_response(&currency),
                        _ => utf16_xml_response(&closing_extent),
                    };
                    source_post_index += 1;
                    response
                }
            };
            socket.write_all(&response).await.expect("write response");
        }
    });

    let runtime = TallyRuntime::default();
    let result = runtime
        .detect_base_currency(
            TallyConfig {
                host: address.ip().to_string(),
                port: address.port(),
            },
            &identity,
        )
        .await;

    let error = result.expect_err("closing extent drift must reject the currency");
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<PairedReadValidationError>(),
            Some(PairedReadValidationError::CurrencyExtent)
        )),
        "unexpected error: {error:#}"
    );
    server.await.expect("synthetic currency server task");
}

fn synthetic_probe_result() -> TallyProbeResult {
    TallyProbeResult {
        connection: ConnectionStatus {
            reachable: true,
            compatible: false,
            server_text: "Synthetic status".to_string(),
            product: TallyProduct::Unknown,
            error: None,
        },
        companies: vec![TallyCompany {
            name: "Synthetic Company".to_string(),
            guid: Some("synthetic-guid".to_string()),
            company_number: None,
            books_from: None,
        }],
        profile: CapabilityProfile {
            profile_version: 2,
            product: "Unknown".to_string(),
            release: None,
            license_tier: None,
            mode: None,
            transports: BTreeMap::new(),
            features: BTreeMap::new(),
            packs: BTreeMap::new(),
        },
        selected_read_scope: None,
        passport_snapshot_id: None,
    }
}

#[test]
fn future_due_bill_from_raw_native_bytes_reaches_the_statement_source() {
    let books_from = TallyDate::parse("20260401").expect("synthetic book start");
    let as_of = TallyDate::parse("20260731").expect("synthetic as-of date");
    let bills_xml = "<ENVELOPE>\
            <BILLFIXED><BILLDATE>1-Jul-26</BILLDATE><BILLREF>FUTURE-1</BILLREF><BILLPARTY>Synthetic Party</BILLPARTY></BILLFIXED>\
            <BILLCL>-100.00</BILLCL><BILLDUE>1-Aug-26</BILLDUE><BILLOVERDUE>0</BILLOVERDUE>\
            </ENVELOPE>";
    let ledger_xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION></COLLECTION></DATA></BODY></ENVELOPE>";
    let receivable = parse_native_bill_rows(bills_xml, &books_from, &as_of)
        .expect("a future due date in raw native bytes must parse");
    let ledgers = parse_native_ledger_snapshot(ledger_xml)
        .expect("the synthetic raw ledger response must parse");
    let computed = compute_native_outstandings(
        "Synthetic Company",
        &receivable,
        &[],
        NativeMasterSnapshot {
            ledgers: &ledgers,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        NativeAgeingAnchor::DueDate,
        &as_of,
        bills_xml.len() + ledger_xml.len(),
    )
    .expect("a future-due bill must not abort the native computation");
    assert_eq!(computed.report.receivable_total.as_str(), "100");
    assert_eq!(computed.report.top_parties[0].oldest_bill_age_days, None);
    assert_eq!(computed.report.ageing.days_0_30.as_str(), "100");
    assert_eq!(computed.report.ageing.days_31_60, ExactDecimal::zero());
    assert_eq!(computed.report.ageing.days_61_90, ExactDecimal::zero());
    assert_eq!(computed.report.ageing.days_90_plus, ExactDecimal::zero());
    assert_eq!(computed.report.open_receivable_bill_count, 1);

    let statement_rows =
        all_open_bill_rows(&receivable, &[], OutstandingsAgeingAnchor::DueDate, &as_of);
    assert_eq!(
        statement_rows.len(),
        1,
        "a future-due bill must remain available to the statement source"
    );
    assert_eq!(statement_rows[0].amount.as_str(), "100.00");
    assert_eq!(statement_rows[0].age_days, None);
    let statement = crate::reports::party_statement::build_party_statement(
        "Synthetic Company",
        as_of.as_str(),
        "Synthetic Party",
        &statement_rows,
        &[],
    )
    .expect("the future-due bill must build a party statement");
    assert_eq!(statement.bills.len(), 1);
    assert_eq!(statement.bills[0].reference, "FUTURE-1");
    assert_eq!(statement.bills[0].age_days, None);
    assert_eq!(statement.bills[0].bucket, None);
    assert_eq!(statement.bill_total.as_str(), "100");

    let destination = tempfile::tempdir().expect("synthetic destination");
    let approvals =
        crate::reports::bulk_party_statement::PartyStatementDestinationApprovals::default();
    let approval_id = approvals
        .issue(destination.path().to_path_buf())
        .expect("approve synthetic destination");
    let approved_destination = approvals
        .consume(&approval_id, destination.path())
        .expect("consume synthetic approval");
    let bulk = crate::reports::bulk_party_statement::write_bulk_party_statements(
        &approved_destination,
        "Synthetic Company",
        as_of.as_str(),
        "xlsx",
        &statement_rows,
        &[],
        |party_statement| {
            crate::reports::party_statement_xlsx::render_party_statement_xlsx(party_statement)
                .map_err(|error| error.to_string())
        },
    )
    .expect("a bulk run must write the future-due party statement");
    assert_eq!(bulk.written.len(), 1);
    let workbook = std::fs::File::open(destination.path().join(&bulk.written[0].file_name))
        .expect("the bulk statement file exists");
    let mut archive = zip::ZipArchive::new(workbook).expect("bulk output is an XLSX archive");
    let mut workbook_text = String::new();
    for entry_name in ["xl/worksheets/sheet1.xml", "xl/sharedStrings.xml"] {
        let mut entry = archive
            .by_name(entry_name)
            .expect("the XLSX statement entry exists");
        std::io::Read::read_to_string(&mut entry, &mut workbook_text)
            .expect("the workbook XML is readable");
    }
    assert!(workbook_text.contains("FUTURE-1"));
    assert!(workbook_text.contains("Not due"));
    assert!(workbook_text.contains("Unaged"));
}

#[test]
fn education_snapshot_period_refusal_is_an_in_band_partial() {
    let result = match admit_native_ledger_snapshot_period(
        DateBoundaryProfile::EducationRestricted,
        TallyDate::parse("20260401").expect("Education-valid book start"),
        TallyDate::parse("20260415").expect("ordinary calendar as-of"),
    ) {
        NativeLedgerSnapshotPeriodAdmission::Period(_) => {
            panic!("an Education-refused as-of must not construct a snapshot period")
        }
        NativeLedgerSnapshotPeriodAdmission::Partial(partial) => partial,
    };
    assert!(matches!(
        result,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "as_of_has_no_valid_window_boundary"
    ));
}

#[cfg(feature = "voucher-scan")]
#[test]
fn outstandings_read_failures_are_partial_and_deadlines_recommend_restart() {
    assert_eq!(
        outstandings_read_failure_reason(&anyhow::Error::new(TallyTransportError::RequestTimedOut)),
        "tally_segment_deadline_restart_recommended"
    );
    assert_eq!(
        outstandings_read_failure_reason(&anyhow::Error::new(TallyTransportError::RequestFailed)),
        "segment_request_failed"
    );
    assert_eq!(
        outstandings_read_failure_reason(&anyhow::Error::new(TallyTransportError::HttpStatus {
            status: 500
        })),
        "segment_http_status_failure"
    );
    assert_eq!(
        outstandings_read_failure_reason(&anyhow::anyhow!("connection refused")),
        "segment_read_failed"
    );
}

#[cfg(feature = "voucher-scan")]
#[tokio::test]
async fn outstandings_read_transport_failure_feeds_breaker_and_preserves_typed_partial() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9120,
    };
    let result = runtime
        .execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            |_client| async {
                Err::<OutstandingsLoadResult, _>(anyhow::Error::new(
                    OutstandingsReadTransportFailure {
                        reason_code: "tally_segment_deadline_restart_recommended",
                        source: anyhow::Error::new(TallyTransportError::RequestTimedOut),
                    },
                ))
            },
        )
        .await;
    let caller_value = partial_after_outstandings_read_transport_failure(result)
        .expect("outstandings read transport failures stay in-band for the caller");
    assert!(matches!(
        caller_value,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "tally_segment_deadline_restart_recommended"
    ));
    assert_eq!(
        runtime.snapshots().expect("runtime snapshot")[0].consecutive_failures,
        1,
        "the breaker must observe the transport failure before UI mapping"
    );
}

#[cfg(feature = "voucher-scan")]
#[tokio::test]
async fn verification_partial_stays_partial_without_feeding_breaker() {
    let runtime = TallyRuntime::default();
    let result = runtime
        .execute(
            TallyConfig {
                host: "localhost".to_string(),
                port: 9121,
            },
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            |_client| async { Ok::<_, anyhow::Error>(partial_result("paired_segment_mismatch")) },
        )
        .await;
    let caller_value = partial_after_outstandings_read_transport_failure(result)
        .expect("verification partial remains an in-band result");
    assert!(matches!(
        caller_value,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "paired_segment_mismatch"
    ));
    assert_eq!(
        runtime.snapshots().expect("runtime snapshot")[0].consecutive_failures,
        0,
        "a verification failure reached a responder and must not poison endpoint health"
    );
}

#[cfg(feature = "voucher-scan")]
#[test]
fn closing_coverage_drift_is_not_reported_as_uncovered_opening_bills() {
    assert_eq!(
        closing_coverage_partial_reason(false, true),
        Some("ledger_master_identity_changed_during_scan")
    );
    assert_eq!(
        closing_coverage_partial_reason(true, false),
        Some("ledger_opening_bills_not_covered")
    );
    assert_eq!(closing_coverage_partial_reason(true, true), None);
}

#[cfg(feature = "voucher-scan")]
#[test]
fn intra_pair_ledger_coverage_drift_is_an_in_band_partial() {
    assert_eq!(
        paired_coverage_partial_reason(&LedgerOpeningCoverageRead::Drifted),
        Some("ledger_master_identity_changed_during_scan")
    );
}

#[cfg(feature = "voucher-scan")]
#[tokio::test]
async fn witness_transport_failure_feeds_breaker_and_preserves_typed_partial() {
    let error = fetch_empty_partition_witness(
        VoucherAlterIdHighWater::parse("1").expect("positive high-water"),
        || async { Err(anyhow::Error::new(TallyTransportError::RequestTimedOut)) },
    )
    .await
    .expect_err("a witness transport failure must cross the runtime health boundary");
    assert!(error.downcast_ref::<TallyTransportError>().is_some());

    let runtime = TallyRuntime::default();
    let result = runtime
        .execute(
            TallyConfig {
                host: "localhost".to_string(),
                port: 9122,
            },
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            |_client| async {
                Err::<OutstandingsLoadResult, _>(outstandings_read_transport_failure(
                    anyhow::Error::new(TallyTransportError::RequestTimedOut),
                ))
            },
        )
        .await;
    let caller_value = partial_after_outstandings_read_transport_failure(result)
        .expect("witness transport failure stays in-band only after health accounting");

    assert!(matches!(
        caller_value,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "tally_segment_deadline_restart_recommended"
    ));
    assert_eq!(
        runtime.snapshots().expect("runtime snapshot")[0].consecutive_failures,
        1,
        "the breaker must observe the witness transport failure"
    );
}

#[cfg(feature = "voucher-scan")]
#[tokio::test]
async fn witness_non_transport_failure_remains_an_in_band_partial() {
    let partial = fetch_empty_partition_witness(
        VoucherAlterIdHighWater::parse("1").expect("positive high-water"),
        || async { Err(anyhow::anyhow!("witness response was malformed")) },
    )
    .await
    .expect("non-transport witness failure stays in-band")
    .expect_err("a malformed witness cannot complete a non-empty book");

    assert_eq!(
        partial.reason_code,
        "empty_date_witness_profile_unavailable"
    );
}

#[cfg(feature = "voucher-scan")]
#[test]
fn outstandings_date_boundaries_follow_detected_mode_and_fallback_to_i12() {
    let mut profile = synthetic_probe_result().profile;
    profile.product = "TallyPrime Edit Log".to_string();
    profile.mode = Some("Education".to_string());
    assert_eq!(
        select_date_boundary_profile(Some(&profile)),
        DateBoundaryProfile::EducationRestricted
    );

    profile.mode = Some("Licensed".to_string());
    assert_eq!(
        select_date_boundary_profile(Some(&profile)),
        DateBoundaryProfile::ModeAgnostic
    );
    assert_eq!(
        select_date_boundary_profile(None),
        DateBoundaryProfile::ModeAgnostic
    );

    profile.product = "Unknown".to_string();
    profile.mode = Some("Education".to_string());
    assert_eq!(
        select_date_boundary_profile(Some(&profile)),
        DateBoundaryProfile::ModeAgnostic,
        "inconsistent or incomplete detection must rely on I12 rather than inventing compatibility evidence"
    );
}

/// An uncalibrated segment width no longer refuses the read -- it selects
/// the native bills path, which needs no width. What must NOT change is
/// that a non-loopback endpoint is still refused.
///
/// This test previously asserted `outstandings_segment_sizing_uncalibrated`
/// and, by using a non-loopback host, proved the refusal happened BEFORE
/// endpoint admission. That refusal was the defect: production has no
/// calibrated width by construction, so every shipped build returned it for
/// every company on every book and the screen could only ever say "No Tally
/// data was read". The reason code is gone with it.
///
/// The loopback guard is unchanged and still fails closed; it is simply now
/// the first guard the native path reaches. That is the property worth
/// pinning, so this asserts it directly rather than inferring it from an
/// ordering that no longer exists.
#[tokio::test]
async fn uncalibrated_outstandings_takes_the_native_path_and_still_refuses_a_non_loopback_endpoint()
{
    let runtime = TallyRuntime::default();
    #[cfg(feature = "voucher-scan")]
    assert!(
        runtime.outstandings_segment_policy.is_none(),
        "a default runtime must have no calibrated width -- that is what routes to the native path"
    );

    let error = runtime
        .fetch_outstandings(
            TallyConfig {
                host: "not-a-loopback-endpoint".to_string(),
                port: 9000,
            },
            &verified_identity("Synthetic Company", "synthetic-guid"),
            TallyDate::parse("20260731").unwrap(),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await
        .expect_err("a non-loopback endpoint must never be contacted");
    assert!(
        error.to_string().contains("non_loopback_forbidden"),
        "loopback-only admission must still fail closed on the native path, got: {error}"
    );
}

#[cfg(feature = "live-calibration-harness")]
#[tokio::test]
async fn calibrated_voucher_scan_withholds_totals_before_endpoint_admission_without_residual_coverage(
) {
    let result = TallyRuntime::for_billwise_lab_reconciliation_exit_check()
        .fetch_outstandings(
            TallyConfig {
                host: "not-a-loopback-endpoint".to_string(),
                port: 9000,
            },
            &verified_identity("Synthetic Company", "synthetic-guid"),
            TallyDate::parse("20260731").unwrap(),
            OutstandingsCurrencyAssertion::Inr,
            OutstandingsAgeingAnchor::DueDate,
        )
        .await
        .expect("missing coverage is an in-band partial result");
    assert!(matches!(
        result,
        OutstandingsLoadResult::Partial { reason, .. }
            if reason.reason_code == "unallocated_direct_postings_not_covered"
    ));
}

#[cfg(all(feature = "voucher-scan", not(feature = "live-calibration-harness")))]
#[test]
fn default_build_has_no_outstandings_width_admission() {
    let runtime = TallyRuntime::default();
    assert!(runtime.outstandings_segment_policy.is_none());
    assert!(runtime.outstandings_boundary_profile_override.is_none());
}

#[cfg(feature = "live-calibration-harness")]
#[test]
fn billwise_lab_exit_harness_uses_only_education_valid_boundaries() {
    let runtime = TallyRuntime::for_billwise_lab_reconciliation_exit_check();
    assert_eq!(
        runtime.outstandings_boundary_profile_override,
        Some(DateBoundaryProfile::EducationRestricted)
    );
    let reporting_window = DateWindow::parse(
        runtime
            .outstandings_boundary_profile_override
            .expect("exit profile is fixed"),
        "20240401",
        "20260702",
    )
    .expect("accepted corpus extent uses Education-valid boundaries");
    let partitions = reporting_window
        .narrow_partitions()
        .expect("Education-valid corpus partitions without day-3 synthesis");
    assert!(partitions.iter().all(|partition| {
        matches!(&partition.from().as_str()[6..8], "01" | "02" | "31")
            && matches!(&partition.to().as_str()[6..8], "01" | "02" | "31")
    }));

    let unprofiled = DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20240401", "20260702")
        .unwrap()
        .narrow_partitions()
        .unwrap();
    assert!(unprofiled.iter().any(|partition| {
        !matches!(&partition.from().as_str()[6..8], "01" | "02" | "31")
            || !matches!(&partition.to().as_str()[6..8], "01" | "02" | "31")
    }));
}

#[test]
fn local_rejections_and_client_http_statuses_do_not_poison_endpoint_health() {
    assert_eq!(
        classify_failure(&anyhow::Error::new(
            TallyRuntimeReadError::ApplicationResponseRejected,
        )),
        ReadFailureClass::Application
    );
    for error in [
        TallyTransportError::RequestTooLarge { limit: 1024 },
        TallyTransportError::PolicyInvalid { code: "test" },
        TallyTransportError::EndpointInvalid { code: "test" },
        TallyTransportError::ClientInitializationFailed,
        TallyTransportError::HttpStatus { status: 400 },
    ] {
        assert_eq!(
            classify_error(&anyhow::Error::new(error)),
            HealthOutcome::ApplicationRejected
        );
    }
    for error in [
        TallyTransportError::ConnectionFailed,
        TallyTransportError::RequestTimedOut,
        TallyTransportError::HttpStatus { status: 503 },
    ] {
        assert_eq!(
            classify_error(&anyhow::Error::new(error)),
            HealthOutcome::TransportFailure
        );
    }
}

#[test]
fn endpoint_identity_aliases_only_localhost_to_ipv4_loopback() {
    let runtime = TallyRuntime::default();
    let localhost_identity = EndpointKey::from_config(&TallyConfig {
        host: "localhost".to_string(),
        port: 9000,
    })
    .expect("localhost identity");
    let ipv4_identity = EndpointKey::from_config(&TallyConfig {
        host: "127.0.0.1".to_string(),
        port: 9000,
    })
    .expect("IPv4 loopback identity");
    assert_eq!(localhost_identity, ipv4_identity);
    let first = runtime
        .session(TallyConfig {
            host: "localhost".to_string(),
            port: 9000,
        })
        .expect("localhost session");
    let second = runtime
        .session(TallyConfig {
            host: "127.0.0.1".to_string(),
            port: 9000,
        })
        .expect("IPv4 loopback session");
    let third = runtime
        .session(TallyConfig {
            host: "::1".to_string(),
            port: 9000,
        })
        .expect("IPv6 loopback session");
    let fourth = runtime
        .session(TallyConfig {
            host: "127.0.0.2".to_string(),
            port: 9000,
        })
        .expect("alternate IPv4 loopback session");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &third));
    assert!(!Arc::ptr_eq(&first, &fourth));
    assert!(!Arc::ptr_eq(&third, &fourth));
    let snapshots = runtime.snapshots().expect("runtime snapshots");
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots[0].canonical_endpoint, "http://127.0.0.1:9000");
    assert_eq!(snapshots[1].canonical_endpoint, "http://127.0.0.2:9000");
    assert_eq!(snapshots[2].canonical_endpoint, "http://[::1]:9000");
}

#[test]
fn reviewed_probe_cache_preserves_observation_time_and_is_single_use() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9000,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    let review_id = "review-current";
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: review_id.to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });

    let mut reservation = runtime
        .reserve_cached_probe_fresh(&config, review_id, 300_000)
        .expect("reserve cache")
        .expect("fresh reviewed probe");
    assert_eq!(reservation.observed_at_unix_ms(), observed_at_unix_ms);
    assert_eq!(reservation.result().companies[0].name, "Synthetic Company");
    assert!(runtime
        .reserve_cached_probe_fresh(&config, review_id, 300_000)
        .expect("second reserve")
        .is_none());
    assert!(reservation.consume().expect("consume reservation"));
    assert!(!reservation
        .consume()
        .expect("consuming an already consumed lease is inert"));
    assert!(runtime
        .reserve_cached_probe_fresh(&config, review_id, 300_000)
        .expect("reserve consumed cache")
        .is_none());
}

#[test]
fn stale_review_id_cannot_consume_or_reserve_a_newer_probe() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9002,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-b".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });

    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-a", 300_000)
        .expect("reject stale review")
        .is_none());
    let mut reservation = runtime
        .reserve_cached_probe_fresh(&config, "review-b", 300_000)
        .expect("reserve current review")
        .expect("current review exists");
    assert!(reservation.release().expect("release current review"));
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-b", 300_000)
        .expect("retry current review")
        .is_some());
}

#[test]
fn reviewed_probe_cache_rejects_future_expired_and_invalid_freshness() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9001,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    for observed_at_unix_ms in [
        chrono::Utc::now().timestamp_millis() + 1_000,
        chrono::Utc::now().timestamp_millis() - 301_000,
    ] {
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-expiry".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        assert!(runtime
            .reserve_cached_probe_fresh(&config, "review-expiry", 300_000)
            .expect("reserve cache")
            .is_none());
    }
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-expiry", 0)
        .is_err());
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-expiry", 600_001)
        .is_err());
}

#[test]
fn replacing_a_qualified_review_does_not_renew_its_freshness_origin() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9003,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let freshness_origin_unix_ms = chrono::Utc::now().timestamp_millis() - 299_000;
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-original".to_string(),
        observed_at_unix_ms: freshness_origin_unix_ms,
        freshness_origin_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    let mut reservation = runtime
        .reserve_cached_probe_fresh(&config, "review-original", 300_000)
        .expect("reserve original")
        .expect("original remains barely fresh");
    assert!(reservation
        .replace(
            "review-qualified".to_string(),
            chrono::Utc::now().timestamp_millis(),
            synthetic_probe_result(),
        )
        .expect("replace reservation"));
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-qualified", 298_000)
        .expect("check inherited freshness")
        .is_none());
}

#[test]
fn ordinary_read_admission_and_review_reservation_are_mutually_exclusive() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9004,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-lease".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });

    let read_lease = runtime
        .begin_ordinary_read(&config)
        .expect("admit ordinary read");
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-lease", 300_000)
        .is_err());
    drop(read_lease);

    let reservation = runtime
        .reserve_cached_probe_fresh(&config, "review-lease", 300_000)
        .expect("reserve after read")
        .expect("fresh review");
    assert!(runtime.begin_ordinary_read(&config).is_err());
    assert!(reservation.authorize(&runtime, &config).is_ok());
    assert!(reservation
        .authorize(
            &runtime,
            &TallyConfig {
                host: "127.0.0.2".to_string(),
                port: 9004,
            },
        )
        .is_err());
}

#[tokio::test]
async fn qualification_rejects_a_reservation_from_another_runtime_before_dispatch() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind cross-runtime qualification server");
    let address = listener.local_addr().expect("qualification server address");
    let config = TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    };
    let owner_runtime = TallyRuntime::default();
    let executing_runtime = TallyRuntime::default();
    let session = owner_runtime
        .session(config.clone())
        .expect("owner runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-cross-runtime".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    drop(session);
    let reservation = owner_runtime
        .reserve_cached_probe_fresh(&config, "review-cross-runtime", 300_000)
        .expect("reserve owner review")
        .expect("fresh owner review");

    let error = executing_runtime
        .qualify_selected_ledgers(
            config,
            &reservation,
            &verified_identity("Synthetic Company", "synthetic-guid"),
        )
        .await
        .expect_err("another runtime must not borrow the reservation");
    assert!(error
        .to_string()
        .contains("reviewed setup operation ownership changed"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept(),)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn production_identity_bracket_rechecks_the_tuple_around_selected_ledger_qualification() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind qualification server");
    let address = listener.local_addr().expect("qualification server address");
    let company_list = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="BRIDGE SYNTHETIC BOOK"><GUID TYPE="String">00000000-0000-4000-8000-000000000001</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><BOOKSFROM TYPE="Date">20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
    let ledger_export = Fixture::NormalExport.body().into_owned();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for body in [
            company_list.to_string(),
            company_list.to_string(),
            ledger_export,
            company_list.to_string(),
        ] {
            let (mut socket, _) = listener.accept().await.expect("accept reserved read");
            let request = read_http_request(&mut socket).await;
            assert!(request.starts_with(b"POST /"));
            requests.push(request);
            socket
                .write_all(&utf16_xml_response(body))
                .await
                .expect("write reserved read response");
        }
        requests
    });
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-qualified-tuple".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    drop(session);
    let reservation = runtime
        .reserve_cached_probe_fresh(&config, "review-qualified-tuple", 300_000)
        .expect("reserve reviewed setup")
        .expect("fresh review");

    let ordinary_read = runtime
        .fetch_companies(config.clone())
        .await
        .expect_err("ordinary reads must remain blocked by the reservation");
    assert!(ordinary_read
        .to_string()
        .contains("reviewed setup operation is in progress"));
    let companies = runtime
        .fetch_companies_for_reservation(config.clone(), &reservation)
        .await
        .expect("reservation owner may recheck company tuple");
    assert_eq!(companies.len(), 1);
    let identity = VerifiedCompanyIdentity::from_observed_companies(
        "BRIDGE SYNTHETIC BOOK".to_string(),
        "00000000-0000-4000-8000-000000000001".to_string(),
        "100001".to_string(),
        "20260401".to_string(),
        &[TallyCompany {
            name: "BRIDGE SYNTHETIC BOOK".to_string(),
            guid: Some("00000000-0000-4000-8000-000000000001".to_string()),
            company_number: Some("100001".to_string()),
            books_from: Some("20260401".to_string()),
        }],
    )
    .expect("synthetic qualification identity is complete");
    let observation = runtime
        .qualify_selected_ledgers(config, &reservation, &identity)
        .await
        .expect("qualification must run after the reserved tuple recheck");
    assert_eq!(observation.result_bucket, "non_empty_observed");
    let requests = server.await.expect("finish reserved qualification server");
    assert_eq!(requests.len(), 4);
    let decoded = requests
        .iter()
        .map(|request| {
            let body_start = request
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .expect("request has complete HTTP headers")
                + 4;
            bridge_tally_protocol::decode_tally_text_bytes_limited(
                &request[body_start..],
                request.len() - body_start,
            )
            .expect("decode dispatched Tally request")
            .text
        })
        .collect::<Vec<_>>();
    for (index, request) in [0, 1, 3].into_iter().map(|index| (index, &decoded[index])) {
        assert!(
            request.contains("<TYPE>Collection</TYPE>"),
            "request {index}"
        );
        assert!(request.contains("<TYPE>Company</TYPE>"), "request {index}");
        assert!(!request.contains("<SVCURRENTCOMPANY>"), "request {index}");
    }
    assert!(decoded[2].contains("<SVCURRENTCOMPANY>BRIDGE SYNTHETIC BOOK</SVCURRENTCOMPANY>"));
}

#[test]
fn dropping_a_review_reservation_restores_the_same_fresh_review() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9005,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-drop".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    drop(
        runtime
            .reserve_cached_probe_fresh(&config, "review-drop", 300_000)
            .expect("reserve review")
            .expect("fresh review"),
    );
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-drop", 300_000)
        .expect("reserve after drop")
        .is_some());
}

#[tokio::test]
async fn aborting_a_task_drops_and_releases_its_review_reservation() {
    let runtime = Arc::new(TallyRuntime::default());
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9006,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-abort".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    let task_runtime = Arc::clone(&runtime);
    let task_config = config.clone();
    let task = tokio::spawn(async move {
        let _reservation = task_runtime
            .reserve_cached_probe_fresh(&task_config, "review-abort", 300_000)
            .expect("reserve review")
            .expect("fresh review");
        held_tx.send(()).expect("announce held reservation");
        std::future::pending::<()>().await;
    });
    held_rx.await.expect("reservation was held");
    task.abort();
    let _ = task.await;
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-abort", 300_000)
        .expect("reserve after abort")
        .is_some());
}

#[tokio::test]
async fn aborting_pending_qualification_releases_review_and_active_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind pending qualification server");
    let address = listener.local_addr().expect("pending server address");
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.expect("accept qualification");
        accepted_tx.send(()).expect("announce accepted request");
        std::future::pending::<()>().await;
    });
    let runtime = Arc::new(TallyRuntime::default());
    let config = TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-pending".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    drop(session);
    let task_runtime = Arc::clone(&runtime);
    let task_config = config.clone();
    let task = tokio::spawn(async move {
        let reservation = task_runtime
            .reserve_cached_probe_fresh(&task_config, "review-pending", 300_000)
            .expect("reserve pending review")
            .expect("fresh pending review");
        let _ = task_runtime
            .qualify_selected_ledgers(
                task_config,
                &reservation,
                &verified_identity("Synthetic Company", "synthetic-guid"),
            )
            .await;
    });
    accepted_rx.await.expect("qualification reached server");
    task.abort();
    let _ = task.await;
    server.abort();
    let snapshots = runtime.snapshots().expect("runtime snapshots after abort");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].active_requests, 0);
    assert!(snapshots[0].active_request_ids.is_empty());
    assert!(runtime
        .reserve_cached_probe_fresh(&config, "review-pending", 300_000)
        .expect("reserve after pending abort")
        .is_some());
}

#[test]
fn stale_guard_cannot_release_or_consume_a_newer_reserved_review() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9007,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-old".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    let mut stale = runtime
        .reserve_cached_probe_fresh(&config, "review-old", 300_000)
        .expect("reserve old")
        .expect("old review");
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-new".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: true,
    });
    assert!(!stale.consume().expect("stale consume is inert"));
    drop(stale);
    let cache = session.cached_probe.read().expect("capability cache");
    let current = cache.as_ref().expect("new review remains");
    assert_eq!(current.review_id, "review-new");
    assert!(current.reserved);
}

#[test]
fn stale_guard_cannot_replace_a_newer_reserved_review() {
    let runtime = TallyRuntime::default();
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9008,
    };
    let session = runtime.session(config.clone()).expect("runtime session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-old".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    let mut stale = runtime
        .reserve_cached_probe_fresh(&config, "review-old", 300_000)
        .expect("reserve old")
        .expect("old review");
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-new".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: true,
    });

    assert!(!stale
        .replace(
            "review-illegal-replacement".to_string(),
            observed_at_unix_ms,
            synthetic_probe_result(),
        )
        .expect("stale replace is inert"));
    drop(stale);
    let cache = session.cached_probe.read().expect("capability cache");
    let current = cache.as_ref().expect("new review remains");
    assert_eq!(current.review_id, "review-new");
    assert!(current.reserved);
}

#[test]
fn held_review_reservation_prevents_endpoint_session_eviction() {
    let runtime = TallyRuntime::default();
    let reserved_config = TallyConfig {
        host: "127.0.0.1".to_string(),
        port: 9200,
    };
    let reserved_endpoint = EndpointKey::from_config(&reserved_config).unwrap();
    let session = runtime
        .session(reserved_config.clone())
        .expect("reserved session");
    let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
    *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
        review_id: "review-capacity".to_string(),
        observed_at_unix_ms,
        freshness_origin_unix_ms: observed_at_unix_ms,
        result: synthetic_probe_result(),
        reserved: false,
    });
    drop(session);
    let _reservation = runtime
        .reserve_cached_probe_fresh(&reserved_config, "review-capacity", 300_000)
        .expect("reserve capacity review")
        .expect("fresh capacity review");
    for host_suffix in 2..=MAX_ENDPOINT_SESSIONS {
        runtime
            .session(TallyConfig {
                host: format!("127.0.0.{host_suffix}"),
                port: 9200,
            })
            .expect("fill endpoint capacity");
    }
    runtime
        .session(TallyConfig {
            host: "127.0.0.254".to_string(),
            port: 9200,
        })
        .expect("evict one unreserved session");
    assert!(runtime
        .sessions
        .lock()
        .expect("session registry")
        .contains_key(&reserved_endpoint));
}

#[tokio::test]
async fn cancellation_registry_cancels_and_releases_requests() {
    let runtime = Arc::new(TallyRuntime::default());
    let config = TallyConfig {
        host: "localhost".to_string(),
        port: 9100,
    };
    let runtime_task = Arc::clone(&runtime);
    let task = tokio::spawn(async move {
        runtime_task
            .execute(
                config,
                ReadOperation::OtherRead,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |_client| async {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    Ok::<_, anyhow::Error>(())
                },
            )
            .await
    });
    tokio::task::yield_now().await;
    let snapshot = runtime
        .snapshots()
        .expect("runtime snapshots")
        .pop()
        .expect("active session");
    assert_eq!(snapshot.active_requests, 1);
    let session = runtime
        .sessions
        .lock()
        .expect("sessions lock")
        .values()
        .next()
        .expect("session")
        .session
        .clone();
    let request_id = session
        .active_requests
        .lock()
        .expect("request lock")
        .keys()
        .next()
        .expect("request ID")
        .clone();
    assert!(runtime.cancel_request(&request_id).expect("cancel request"));
    assert!(task.await.expect("request task").is_err());
    assert_eq!(
        runtime.snapshots().expect("runtime snapshots")[0].active_requests,
        0
    );
    assert_eq!(
        runtime.snapshots().expect("runtime snapshots")[0].consecutive_failures,
        0,
        "operator cancellation must not degrade endpoint health"
    );
}

#[test]
fn telemetry_preview_is_privacy_reduced_and_checksummed() {
    let preview = TallyRuntime::default()
        .telemetry_preview()
        .expect("telemetry preview");
    assert_eq!(preview.schema, "bridge.tally.telemetry-preview/2");
    assert_eq!(preview.payload_sha256.len(), 64);
    let preview_value: serde_json::Value =
        serde_json::from_str(&preview.preview_json).expect("valid preview JSON");
    assert_eq!(
        preview_value["privacy_profile"],
        "fixed_dimensions_bucketed_values_v1"
    );
    assert_eq!(preview_value["authenticity_claim"], "none");
}

/// Both selected-read qualifiers send a custom report whose TDL Education
/// answers with a blocking dialog on the Tally screen (bridge#45). When the
/// identity bracket before it reports Education, the qualifier refuses before
/// sending: only that one company-list request reaches the endpoint.
#[tokio::test]
async fn selected_read_qualification_is_refused_before_sending_in_education() {
    for vouchers in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind qualification server");
        let address = listener.local_addr().expect("qualification server address");
        let company_list = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="BRIDGE SYNTHETIC BOOK"><GUID TYPE="String">00000000-0000-4000-8000-000000000001</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><BOOKSFROM TYPE="Date">20260401</BOOKSFROM><EDUMODE TYPE="Logical">Yes</EDUMODE></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept identity bracket");
            let request = read_http_request(&mut socket).await;
            socket
                .write_all(&utf16_xml_response(company_list))
                .await
                .expect("write identity bracket response");
            drop(listener);
            request
        });
        let runtime = TallyRuntime::default();
        let config = TallyConfig {
            host: address.ip().to_string(),
            port: address.port(),
        };
        let session = runtime.session(config.clone()).expect("runtime session");
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        *session.cached_probe.write().expect("capability cache") = Some(CachedProbe {
            review_id: "review-education".to_string(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: synthetic_probe_result(),
            reserved: false,
        });
        drop(session);
        let reservation = runtime
            .reserve_cached_probe_fresh(&config, "review-education", 300_000)
            .expect("reserve reviewed setup")
            .expect("fresh review");
        let identity = VerifiedCompanyIdentity::from_observed_companies(
            "BRIDGE SYNTHETIC BOOK".to_string(),
            "00000000-0000-4000-8000-000000000001".to_string(),
            "100001".to_string(),
            "20260401".to_string(),
            &[TallyCompany {
                name: "BRIDGE SYNTHETIC BOOK".to_string(),
                guid: Some("00000000-0000-4000-8000-000000000001".to_string()),
                company_number: Some("100001".to_string()),
                books_from: Some("20260401".to_string()),
            }],
        )
        .expect("synthetic qualification identity is complete");
        let refused = if vouchers {
            runtime
                .qualify_selected_vouchers(
                    config,
                    &reservation,
                    &identity,
                    "20260401".to_string(),
                    "20260430".to_string(),
                )
                .await
        } else {
            runtime
                .qualify_selected_ledgers(config, &reservation, &identity)
                .await
        }
        .expect_err("Education refuses the report before it is sent");
        assert!(
            refused
                .chain()
                .any(|cause| cause.is::<EducationReportFamilyRefusal>()),
            "{vouchers}: {refused:#}"
        );
        let request = server.await.expect("identity bracket server");
        assert!(request.starts_with(b"POST /"), "{vouchers}");
    }
}
