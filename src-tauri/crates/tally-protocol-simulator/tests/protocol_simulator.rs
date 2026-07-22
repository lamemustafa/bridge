use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

use bridge_tally_core::ExactDecimal;
use quick_xml::{events::Event, Reader};
use tally_protocol_simulator::{
    decode, Delivery, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, Simulator,
    WireEncoding, MAX_SEQUENCE_REQUESTS,
};

fn request(simulator: Simulator, method: &str, path: &str) -> (Vec<u8>, bool) {
    let mut stream = TcpStream::connect(simulator.address()).expect("connect loopback simulator");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .expect("write synthetic request");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    let observed = simulator.finish().expect("finish simulator");
    assert_eq!(observed.method, method);
    assert_eq!(observed.path, path);
    (response, observed.request_processed)
}

fn response_body(response: &[u8]) -> &[u8] {
    let marker = b"\r\n\r\n";
    let offset = response
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("HTTP header terminator");
    &response[offset + marker.len()..]
}

#[test]
fn bounded_sequence_serves_plans_in_exact_request_order() {
    let simulator = SequenceSimulator::spawn(vec![
        ScenarioPlan::new(Fixture::ExportStatusOne).with_http_status(500),
        ScenarioPlan::new(Fixture::ExportStatusOne),
    ])
    .expect("spawn sequence simulator");
    for expected_status in ["500 Internal Server Error", "200 OK"] {
        let mut stream = TcpStream::connect(simulator.address()).expect("connect sequence");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set sequence read timeout");
        write!(
            stream,
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .expect("write sequence request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("read sequence response");
        assert!(String::from_utf8_lossy(&response)
            .starts_with(&format!("HTTP/1.1 {expected_status}\r\n")));
    }
    let observed = simulator.finish().expect("finish sequence simulator");
    assert_eq!(observed.len(), 2);
    assert!(observed.iter().all(|request| {
        request.method == "POST" && request.path == "/" && request.request_processed
    }));
}

#[test]
fn delayed_request_body_within_deadline_is_not_abandoned() {
    let simulator =
        Simulator::spawn(ScenarioPlan::new(Fixture::ExportStatusOne)).expect("spawn simulator");
    let address = simulator.address();
    let response = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\n",
        )?;
        // A native CI runner can pause a request producer after its TCP connection is accepted.
        // The simulator must retain that connection long enough to receive its bounded body.
        std::thread::sleep(Duration::from_millis(2100));
        stream.write_all(b"body")?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response)?;
        Ok(response)
    })
    .join()
    .expect("delayed request thread does not panic");
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            simulator.cancel();
            panic!("delayed request is still processed: {error}");
        }
    };
    let observed = simulator.finish().expect("finish delayed simulator");
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(observed.request_processed);
    assert_eq!(observed.request_body_bytes, 4);
}

#[test]
fn sequence_request_count_is_fail_closed() {
    assert!(SequenceSimulator::spawn(Vec::new()).is_err());
    assert!(SequenceSimulator::spawn(
        (0..=MAX_SEQUENCE_REQUESTS)
            .map(|_| ScenarioPlan::new(Fixture::ExportStatusOne))
            .collect()
    )
    .is_err());
}

fn element_values(xml: &str, element_name: &[u8]) -> Result<Vec<String>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut values = Vec::new();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) if element.name().as_ref() == element_name => {
                values.push(
                    reader
                        .read_text(element.name())
                        .map_err(|error| error.to_string())?
                        .decode()
                        .map_err(|error| error.to_string())?
                        .into_owned(),
                );
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(values)
}

fn assert_well_formed(xml: &str) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    let mut open = Vec::<Vec<u8>>::new();
    loop {
        match reader.read_event().map_err(|error| error.to_string())? {
            Event::Start(element) => open.push(element.name().as_ref().to_vec()),
            Event::End(element) => {
                let expected = open
                    .pop()
                    .ok_or_else(|| "unexpected closing element".to_owned())?;
                if expected != element.name().as_ref() {
                    return Err("mismatched closing element".to_owned());
                }
            }
            Event::Eof if open.is_empty() => return Ok(()),
            Event::Eof => return Err("document ended before all elements closed".to_owned()),
            _ => {}
        }
    }
}

#[test]
fn application_status_matrix_is_explicit_and_independent_of_http_200() {
    let cases = [
        (Fixture::ExportStatusOne, Some("1")),
        (Fixture::ExportStatusZero, Some("0")),
        (Fixture::ExportStatusMissing, None),
        (Fixture::ExportStatusInvalid, Some("-1")),
    ];
    for (fixture, expected) in cases {
        let body = fixture.body();
        let status = element_values(&body, b"STATUS").expect("status fixture must be XML");
        assert_eq!(status.first().map(String::as_str), expected);

        let simulator = Simulator::spawn(ScenarioPlan::new(fixture)).expect("spawn simulator");
        let (response, processed) = request(simulator, "POST", "/");
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
        assert!(processed);
    }

    let simulator =
        Simulator::spawn(ScenarioPlan::new(Fixture::ExportStatusOne).with_http_status(500))
            .expect("spawn HTTP failure simulator");
    let (response, _) = request(simulator, "POST", "/");
    assert!(response.starts_with(b"HTTP/1.1 500 Internal Server Error\r\n"));
    assert_eq!(
        element_values(
            std::str::from_utf8(response_body(&response)).expect("UTF-8 body"),
            b"STATUS",
        )
        .unwrap(),
        ["1"]
    );
}

#[test]
fn product_status_catalog_covers_prime_erp9_and_unknown() {
    let cases = [
        (ProductStatus::TallyPrime, "TallyPrime Server is Running"),
        (ProductStatus::TallyErp9, "Tally ERP 9 Server is Running"),
        (ProductStatus::Unknown, "BRIDGE SYNTHETIC UNKNOWN PRODUCT"),
    ];
    for (product, marker) in cases {
        let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::ProductStatus(product)))
            .expect("spawn status simulator");
        let (response, _) = request(simulator, "GET", "/status");
        let body = std::str::from_utf8(response_body(&response)).expect("status UTF-8");
        assert!(body.contains(marker));
    }
}

#[test]
fn utf8_bom_and_both_utf16_endiannesses_have_identical_text() {
    let source = Fixture::NormalExport.body().into_owned();
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf8Bom,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let plan = ScenarioPlan::new(Fixture::NormalExport).with_encoding(encoding);
        let bytes = plan.response_bytes();
        assert_eq!(decode(&bytes).expect("decode supported encoding"), source);

        let simulator = Simulator::spawn(plan).expect("spawn encoded simulator");
        let (response, _) = request(simulator, "POST", "/");
        assert_eq!(
            decode(response_body(&response)).expect("decode HTTP body"),
            source
        );
    }
    assert!(decode(&[0xFF, 0xFE, 0x00]).is_err());
    assert!(decode(&[0x80]).is_err());
}

#[test]
fn malformed_and_truncated_xml_are_not_well_formed() {
    assert!(assert_well_formed(&Fixture::MalformedXml.body()).is_err());
    assert!(assert_well_formed(&Fixture::TruncatedXml.body()).is_err());
    assert!(assert_well_formed(&Fixture::NormalExport.body()).is_ok());
}

#[test]
fn company_context_and_duplicate_identity_anomalies_are_deterministic() {
    let normal = Fixture::NormalExport.body();
    assert!(normal.contains("BRIDGE SYNTHETIC BOOK"));
    assert!(normal.contains("00000000-0000-4000-8000-000000000001"));

    let wrong = Fixture::WrongCompany.body();
    assert!(wrong.contains("BRIDGE SYNTHETIC OTHER BOOK"));
    assert!(wrong.contains("00000000-0000-4000-8000-000000000002"));
    assert!(!wrong.contains("00000000-0000-4000-8000-000000000001"));

    let duplicate = Fixture::DuplicateIdentity.body();
    let duplicated_guid = "00000000-0000-4000-8000-000000000199";
    assert_eq!(duplicate.matches(duplicated_guid).count(), 2);
}

#[test]
fn exact_decimal_forms_never_use_floating_point() {
    let values = element_values(&Fixture::ExactDecimals.body(), b"AMOUNT")
        .expect("read exact decimal fixtures");
    assert_eq!(values, ["0", "0.00", "-1180.00", "999999999999.9999"]);
    for value in values {
        ExactDecimal::parse(value).expect("valid exact decimal");
    }
    for invalid in ["1e3", "+1.00", "1,000.00", "NaN", ""] {
        assert!(ExactDecimal::parse(invalid).is_err());
    }
}

#[test]
fn import_counter_fixtures_preserve_success_duplicate_and_partial_results() {
    fn counters(fixture: Fixture) -> BTreeMap<&'static str, u64> {
        [
            "CREATED",
            "ALTERED",
            "DELETED",
            "IGNORED",
            "ERRORS",
            "CANCELLED",
            "EXCEPTIONS",
        ]
        .into_iter()
        .map(|name| {
            let values = element_values(&fixture.body(), name.as_bytes()).unwrap();
            (name, values[0].parse::<u64>().unwrap())
        })
        .collect()
    }

    let success = counters(Fixture::ImportCounters);
    assert_eq!(success["CREATED"], 2);
    assert_eq!(success["ALTERED"], 3);
    assert_eq!(success["DELETED"], 1);
    assert_eq!(success["ERRORS"], 0);

    let duplicate = counters(Fixture::ImportDuplicate);
    assert_eq!(duplicate["IGNORED"], 1);
    assert_eq!(duplicate["ERRORS"], 1);
    assert_eq!(
        element_values(&Fixture::ImportDuplicate.body(), b"LINEERROR")
            .unwrap()
            .len(),
        1
    );

    let partial = counters(Fixture::ImportPartial);
    assert_eq!(partial["CREATED"], 1);
    assert_eq!(partial["ALTERED"], 1);
    assert_eq!(partial["IGNORED"], 1);
    assert_eq!(partial["ERRORS"], 1);
    assert_eq!(partial["EXCEPTIONS"], 1);
}

#[test]
fn oversized_slow_body_and_reset_before_body_are_real_http_behaviors() {
    let minimum = 64 * 1024;
    let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::Oversized {
        minimum_bytes: minimum,
    }))
    .unwrap();
    let (response, processed) = request(simulator, "POST", "/");
    assert!(processed);
    assert!(response_body(&response).len() >= minimum);

    let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::ExportStatusOne).with_delivery(
        Delivery::SlowBody {
            chunk_bytes: 7,
            delay: Duration::from_millis(1),
        },
    ))
    .unwrap();
    let (response, processed) = request(simulator, "POST", "/");
    assert!(processed);
    assert_eq!(
        response_body(&response),
        Fixture::ExportStatusOne.body().as_bytes()
    );

    let simulator = Simulator::spawn(
        ScenarioPlan::new(Fixture::ExportStatusOne).with_delivery(Delivery::ResetBeforeBody),
    )
    .unwrap();
    let (response, processed) = request(simulator, "POST", "/");
    assert!(!processed);
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(response_body(&response).is_empty());
}

#[test]
fn cancellation_interrupts_slow_headers_without_deadlocking() {
    let simulator = Simulator::spawn(
        ScenarioPlan::new(Fixture::ExportStatusOne)
            .with_delivery(Delivery::SlowHeaders(Duration::from_secs(2))),
    )
    .unwrap();
    let mut stream = TcpStream::connect(simulator.address()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream
        .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let started = Instant::now();
    simulator.cancel();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let observed = simulator.finish().unwrap();
    assert!(observed.cancelled);
    assert!(!observed.request_processed);
    assert!(response.is_empty());
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn delayed_reset_after_processing_is_explicitly_ambiguous() {
    let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::ImportCounters).with_delivery(
        Delivery::ResetAfterRequestProcessed {
            delay: Duration::from_millis(20),
        },
    ))
    .unwrap();
    let (response, processed) = request(simulator, "POST", "/");
    assert!(response.is_empty());
    assert!(
        processed,
        "the simulator records that the request may have committed"
    );
}

#[test]
fn synthetic_json_reference_contains_selected_xml_fixture_values() {
    let xml = Fixture::NormalExport.body();
    let json: serde_json::Value =
        serde_json::from_str(&Fixture::SyntheticJsonSemanticReference.body())
            .expect("valid synthetic JSON reference");

    assert!(xml.contains(json["company"]["guid"].as_str().unwrap()));
    assert!(xml.contains(json["company"]["name"].as_str().unwrap()));
    let ledger = &json["ledgers"][0];
    for field in ["name", "opening_balance", "parent", "guid"] {
        assert!(xml.contains(ledger[field].as_str().unwrap()));
    }
}

#[test]
fn scoped_export_metadata_scenarios_are_explicit_and_minimized() {
    let empty = Fixture::EmptyExport.body();
    assert!(empty.contains("<RECORDCOUNT>0</RECORDCOUNT>"));
    assert!(!empty.contains("<LEDGER"));

    let mismatch = Fixture::RecordCountMismatch.body();
    assert!(mismatch.contains("<RECORDCOUNT>2</RECORDCOUNT>"));
    assert_eq!(mismatch.matches("<LEDGER ").count(), 1);

    assert!(Fixture::MalformedExportMetadata
        .body()
        .contains("<RECORDCOUNT>-1</RECORDCOUNT>"));
    let duplicate = Fixture::DuplicateExportMetadata.body();
    assert!(duplicate.contains("SCHEMA=\"bridge.tally.ledgers/1\""));
    assert!(duplicate.contains("<SCHEMA>bridge.tally.ledgers/1</SCHEMA>"));

    let voucher = Fixture::VoucherExport.body();
    assert!(voucher.contains("SCHEMA=\"bridge.tally.vouchers/2\""));
    assert!(voucher.contains("OBJECTTYPE=\"VOUCHER\""));
    for forbidden in [
        "NARRATION",
        "ADDRESS",
        "EMAIL",
        "PHONE",
        "MOBILE",
        "PINCODE",
    ] {
        assert!(!voucher.contains(forbidden));
    }
}

#[test]
fn inconsistent_date_filter_fixture_returns_a_row_outside_its_declared_window() {
    let xml = Fixture::InconsistentDateFilter.body();
    assert_eq!(
        element_values(&xml, b"DATE").unwrap(),
        vec!["20260630".to_string()]
    );
    assert!(xml.contains("FROMDATE=\"20260701\""));
    assert!(xml.contains("TODATE=\"20260731\""));
    assert!(assert_well_formed(&xml).is_ok());
}

#[test]
fn entire_corpus_is_synthetic_and_contains_no_local_identity_markers() {
    let fixtures = [
        Fixture::ProductStatus(ProductStatus::TallyPrime),
        Fixture::ProductStatus(ProductStatus::TallyErp9),
        Fixture::ProductStatus(ProductStatus::Unknown),
        Fixture::ExportStatusOne,
        Fixture::ExportStatusZero,
        Fixture::ExportStatusMissing,
        Fixture::ExportStatusInvalid,
        Fixture::NormalExport,
        Fixture::EmptyExport,
        Fixture::DuplicateIdentity,
        Fixture::WrongCompany,
        Fixture::VoucherExport,
        Fixture::InconsistentDateFilter,
        Fixture::RecordCountMismatch,
        Fixture::MalformedExportMetadata,
        Fixture::DuplicateExportMetadata,
        Fixture::ExactDecimals,
        Fixture::ImportCounters,
        Fixture::ImportDuplicate,
        Fixture::ImportPartial,
        Fixture::MalformedXml,
        Fixture::TruncatedXml,
        Fixture::SyntheticJsonSemanticReference,
        Fixture::UnsupportedCapability,
    ];
    for fixture in fixtures {
        let body = fixture.body();
        for forbidden in [
            "C:\\Users\\",
            "/Users/",
            "@example",
            "PARTYGSTIN",
            "PHONE",
            "MOBILE",
        ] {
            assert!(!body.contains(forbidden), "forbidden marker: {forbidden}");
        }
        if body.contains("NAME>") || body.contains("name\"") {
            assert!(body.contains("BRIDGE SYNTHETIC"));
        }
    }
}
