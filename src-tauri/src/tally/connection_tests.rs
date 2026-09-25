#[tokio::test]
async fn capability_probe_preserves_captured_release_and_exclusive_license_tier() {
    let capture = include_bytes!("../../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml");
    let xml = bridge_tally_protocol::decode_tally_xml_response_bytes_limited(
        capture,
        "text/xml; charset=utf-16",
        bridge_tally_protocol::ExpectedTallyTextEncoding::Utf16Le,
        capture.len(),
    )
    .unwrap()
    .text;
    let observation = super::parse_company_gateway_capability_observation(&xml).unwrap();
    for (education, silver, gold, tier) in [
        (false, true, false, Some(super::LicenseTier::Silver)),
        (false, false, true, Some(super::LicenseTier::Gold)),
        (false, true, true, None),
        (false, false, false, None),
        (true, true, false, None),
    ] {
        let mut altered = observation.clone();
        altered.educational_mode = education;
        altered.silver = silver;
        altered.gold = gold;
        assert_eq!(
            super::GatewayProductModeEvidence::from_observation(altered).license_tier,
            tier
        );
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        for response in [
            utf8_status_response("<RESPONSE>Unknown status banner</RESPONSE>"),
            utf16_xml_response(&xml),
        ] {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_complete_http_request(&mut socket).await;
            assert!(!request.is_empty());
            socket.write_all(&response).await.unwrap();
        }
    });
    let (probe, wire) = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .unwrap()
    .probe_with_wire_evidence()
    .await
    .unwrap();
    server.await.unwrap();
    assert!(wire.bytes >= capture.len());
    assert_eq!(probe.companies.len(), 16);
    assert_eq!(probe.profile.profile_version, 4);
    assert_eq!(probe.profile.product, "TallyPrime");
    assert_eq!(probe.profile.mode.as_deref(), Some("Licensed"));
    assert_eq!(probe.profile.release.as_deref(), Some("7.1"));
    assert_eq!(probe.profile.license_tier, Some(super::LicenseTier::Silver));
    assert_eq!(
        probe.profile.transports[&TransportId::JsonEx].state,
        CapabilityState::Unknown
    );
    assert_eq!(
        probe.profile.transports[&TransportId::JsonEx]
            .safe_reason_code
            .as_deref(),
        Some("transport_not_probed")
    );
    let mut old = serde_json::to_value(&probe.profile).unwrap();
    old.as_object_mut().unwrap().remove("license_tier");
    old["profile_version"] = serde_json::json!(3);
    let old: bridge_tally_core::CapabilityProfile = serde_json::from_value(old).unwrap();
    assert_eq!(old.license_tier, None);
    assert_ne!(old.profile_version, probe.profile.profile_version);
}

#[test]
fn party_ledger_commitment_hashes_the_three_encoded_builder_requests() {
    let master = NativeLedgerExportPeriod::new(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20260401").unwrap(),
        TallyDate::parse("20260731").unwrap(),
    )
    .unwrap();
    let balance = party_ledger_master_balance_period(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20260401").unwrap(),
        TallyDate::parse("20260731").unwrap(),
    )
    .unwrap();
    let requests = [
        super::render_party_ledger_master_request("Synthetic ₹ Books", &master),
        super::render_native_ledger_snapshot_request("Synthetic ₹ Books", &balance),
        super::render_native_group_snapshot_request("Synthetic ₹ Books"),
    ];
    let hashes = requests
        .iter()
        .map(|request| {
            let mut bytes = vec![0xff, 0xfe];
            bytes.extend(request.encode_utf16().flat_map(u16::to_le_bytes));
            super::sha256_hex(&bytes)
        })
        .collect::<Vec<_>>();
    let expected = super::sha256_hex(hashes.join(":").as_bytes());
    assert_eq!(super::party_ledger_request_commitment(&requests), expected);
    for index in 0..3 {
        let mut changed = requests.clone();
        changed[index].push(' ');
        assert_ne!(super::party_ledger_request_commitment(&changed), expected);
    }
}

#[cfg(feature = "voucher-scan")]
use super::LedgerOpeningCoverageRead;
use super::{
    canonical_loopback_origin, decode_xml_bytes, detect_product,
    has_presentation_equivalent_guid_siblings, normalize_discovered_companies,
    party_ledger_master_balance_period, party_ledger_master_openings_agree, tally_endpoint,
    unique_company_identities, TallyClient, TallyConfig, TallyProduct, VerifiedCompanyIdentity,
};
use bridge_tally_core::{
    CapabilityFeatureId, CapabilityPackId, CapabilityState, EvidenceConfidence, TallyDate,
    TransportId,
};
use bridge_tally_protocol::native_outstandings::NativeLedgerExportPeriod;
use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;
use std::time::Duration;
use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator, WireEncoding};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const COMPANY_EXTENT_V2: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
);
const CAPTURED_COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";

fn identity_from_company_extent(xml: &str) -> VerifiedCompanyIdentity {
    let companies = bridge_tally_protocol::parse_companies_from_collection(xml)
        .expect("captured company extent must parse as a Company collection");
    let company = companies
        .iter()
        .find(|company| company.guid.as_deref() == Some(CAPTURED_COMPANY_GUID))
        .expect("captured target company must remain present");
    VerifiedCompanyIdentity::from_observed_companies(
        company.name.clone(),
        company.guid.clone().expect("captured target company GUID"),
        company
            .company_number
            .clone()
            .expect("captured target company number"),
        company
            .books_from
            .clone()
            .expect("captured target company books-from"),
        &companies,
    )
    .expect("captured target company tuple must remain uniquely admissible")
}

fn captured_company_identity() -> VerifiedCompanyIdentity {
    identity_from_company_extent(COMPANY_EXTENT_V2)
}

#[test]
fn party_master_snapshot_uses_the_next_common_admissible_boundary() {
    let period = party_ledger_master_balance_period(
        DateBoundaryProfile::ModeAgnostic,
        TallyDate::parse("20260401").unwrap(),
        TallyDate::parse("20260415").unwrap(),
    )
    .expect("the derived common boundary is valid");
    assert_eq!(period.to().as_str(), "20260501");
    assert!(period.to() >= &TallyDate::parse("20260415").unwrap());
}

#[test]
fn party_master_opening_balance_comparison_accepts_equivalent_decimal_scale() {
    assert!(party_ledger_master_openings_agree(
        "0",
        &bridge_tally_core::ExactDecimal::parse("0.00").unwrap(),
    )
    .unwrap());
}

#[test]
fn party_master_opening_balance_comparison_withholds_a_real_numeric_mismatch() {
    assert!(!party_ledger_master_openings_agree(
        "0.01",
        &bridge_tally_core::ExactDecimal::parse("0.00").unwrap(),
    )
    .unwrap());
}

#[test]
fn party_master_opening_balance_comparison_rejects_an_unparseable_master_value() {
    assert!(party_ledger_master_openings_agree(
        "not-an-amount",
        &bridge_tally_core::ExactDecimal::parse("0.00").unwrap(),
    )
    .is_err());
}

fn utf16_xml_response(body: impl AsRef<str>) -> Vec<u8> {
    let body = bridge_tally_protocol::encode_tally_xml_request_utf16le(body.as_ref());
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-16\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), &body].concat()
}

fn utf8_status_response(body: impl AsRef<str>) -> Vec<u8> {
    let body = body.as_ref().as_bytes();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), body].concat()
}

/// Reads one HTTP/1.1 request to its header terminator and then exactly its
/// declared Content-Length, so a raw test responder never answers a request it
/// has only partly read. Shared with the agent's endpoint-failure tests.
pub(crate) async fn read_complete_http_request(
    socket: &mut (impl tokio::io::AsyncRead + Unpin),
) -> Vec<u8> {
    const HEADER_TERMINATOR: &[u8] = b"\r\n\r\n";
    const MAX_TEST_HEADER_BYTES: usize = 64 * 1024;

    let mut request = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        if let Some(offset) = request
            .windows(HEADER_TERMINATOR.len())
            .position(|window| window == HEADER_TERMINATOR)
        {
            break offset + HEADER_TERMINATOR.len();
        }
        let bytes_read = socket
            .read(&mut chunk)
            .await
            .expect("read synthetic HTTP request headers");
        assert!(
            bytes_read > 0,
            "synthetic HTTP request ended before its headers were complete"
        );
        request.extend_from_slice(&chunk[..bytes_read]);
        assert!(
            request.len() <= MAX_TEST_HEADER_BYTES,
            "synthetic HTTP request headers exceeded {MAX_TEST_HEADER_BYTES} bytes"
        );
    };

    let headers = std::str::from_utf8(&request[..header_end - HEADER_TERMINATOR.len()])
        .expect("synthetic HTTP request headers must be UTF-8");
    let mut lines = headers.split("\r\n");
    let request_line = lines
        .next()
        .expect("synthetic HTTP request must contain a request line");
    let mut request_line_parts = request_line.split_ascii_whitespace();
    let method = request_line_parts
        .next()
        .expect("synthetic HTTP request method is missing");
    request_line_parts
        .next()
        .expect("synthetic HTTP request target is missing");
    let version = request_line_parts
        .next()
        .expect("synthetic HTTP request version is missing");
    assert_eq!(version, "HTTP/1.1", "synthetic request must use HTTP/1.1");
    assert!(
        request_line_parts.next().is_none(),
        "synthetic HTTP request line has unexpected fields"
    );

    let mut content_length = None;
    for header in lines {
        let (name, value) = header
            .split_once(':')
            .expect("synthetic HTTP request contains a malformed header");
        assert!(
            !name.eq_ignore_ascii_case("transfer-encoding"),
            "synthetic HTTP request must use Content-Length framing"
        );
        if name.eq_ignore_ascii_case("content-length") {
            assert!(
                content_length.is_none(),
                "synthetic HTTP request contains duplicate Content-Length headers"
            );
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .expect("synthetic HTTP request has an invalid Content-Length"),
            );
        }
    }
    let content_length = match content_length {
        Some(length) => length,
        None if method == "GET" || method == "HEAD" => 0,
        None => panic!("synthetic HTTP request body is missing Content-Length"),
    };
    let request_end = header_end
        .checked_add(content_length)
        .expect("synthetic HTTP request length overflowed usize");
    assert!(
        request.len() <= request_end,
        "synthetic HTTP request contains bytes beyond Content-Length"
    );

    while request.len() < request_end {
        let remaining = request_end - request.len();
        let chunk_length = remaining.min(chunk.len());
        let bytes_read = socket
            .read(&mut chunk[..chunk_length])
            .await
            .expect("read synthetic HTTP request body");
        assert!(
            bytes_read > 0,
            "synthetic HTTP request ended before its Content-Length body was complete"
        );
        request.extend_from_slice(&chunk[..bytes_read]);
    }

    request
}

fn assert_company_collection_request_shape(request: &str) {
    assert!(request.contains("<TYPE>Collection</TYPE>"));
    for method in ["NAME", "GUID", "PRODUCTNAME"] {
        assert!(request.contains(&format!("<NATIVEMETHOD>{method}</NATIVEMETHOD>")));
    }
    for expression in [
        "<COMPUTE>EduMode : $$LicenseInfo:IsEducationalMode</COMPUTE>",
        "<COMPUTE>Silver : $$LicenseInfo:IsSilver</COMPUTE>",
        "<COMPUTE>Gold : $$LicenseInfo:IsGold</COMPUTE>",
    ] {
        assert!(request.contains(expression));
    }
}

#[test]
fn detects_tallyprime_status() {
    assert!(matches!(
        detect_product("TallyPrime Server is Running"),
        TallyProduct::TallyPrime
    ));
}

#[test]
fn detects_erp9_status() {
    assert!(matches!(
        detect_product("Tally ERP 9 Server is Running"),
        TallyProduct::TallyErp9
    ));
}

#[test]
fn product_marker_is_not_accepted_inside_unrelated_content() {
    assert!(matches!(
        detect_product("<html><body>TallyPrime Server is Running</body></html>"),
        TallyProduct::Unknown
    ));
    assert!(matches!(
        detect_product("prefix Tally ERP 9 Server is Running suffix"),
        TallyProduct::Unknown
    ));
}

#[tokio::test]
async fn selected_read_request_digest_matches_the_dispatched_wire_entity() {
    let plan = ScenarioPlan::new(Fixture::NormalExport).with_encoding(WireEncoding::Utf16Le);
    let simulator = Simulator::spawn(plan).expect("spawn synthetic Tally endpoint");
    let client = TallyClient::new(TallyConfig {
        host: simulator.address().ip().to_string(),
        port: simulator.address().port(),
    })
    .expect("build synthetic Tally client");

    let observation = client
        .qualify_selected_ledgers(
            "BRIDGE SYNTHETIC BOOK",
            "00000000-0000-4000-8000-000000000001",
        )
        .await
        .expect("qualify synthetic selected-ledger read");
    let dispatched = simulator.finish().expect("finish synthetic Tally exchange");

    assert_eq!(observation.request_sha256, dispatched.request_body_sha256);
}

#[test]
fn company_identity_normalization_rejects_invalid_and_ambiguous_guids() {
    let normalized = normalize_discovered_companies(vec![
        crate::tally::TallyCompany {
            name: "  Synthetic A  ".to_string(),
            guid: Some("  GUID-1  ".to_string()),
            company_number: Some("100005".to_string()),
            books_from: Some("20250401".to_string()),
        },
        crate::tally::TallyCompany {
            name: "Synthetic B".to_string(),
            guid: Some("guid-1".to_string()),
            company_number: Some("100014".to_string()),
            books_from: Some("20260401".to_string()),
        },
    ])
    .expect("normalize company identities");
    assert_eq!(normalized[0].name, "Synthetic A");
    assert_eq!(normalized[0].guid.as_deref(), Some("GUID-1"));
    assert!(unique_company_identities(&normalized));
    assert!(!has_presentation_equivalent_guid_siblings(&normalized));

    assert!(
        normalize_discovered_companies(vec![crate::tally::TallyCompany {
            name: "Synthetic\nCompany".to_string(),
            guid: Some("guid-2".to_string()),
            company_number: None,
            books_from: None,
        }])
        .is_err()
    );
    assert!(
        normalize_discovered_companies(vec![crate::tally::TallyCompany {
            name: "Synthetic Company".to_string(),
            guid: Some("guid\n2".to_string()),
            company_number: None,
            books_from: None,
        }])
        .is_err()
    );
}

#[test]
fn presentation_equivalent_guid_siblings_are_not_stable_company_identities() {
    let companies = vec![
        crate::tally::TallyCompany {
            name: "Synthetic Company".to_string(),
            guid: Some("guid-3".to_string()),
            company_number: Some("100005".to_string()),
            books_from: Some("20250401".to_string()),
        },
        crate::tally::TallyCompany {
            name: " synthetic company ".to_string(),
            guid: Some("GUID-3".to_string()),
            company_number: Some("100014".to_string()),
            books_from: Some("20260401".to_string()),
        },
    ];

    assert!(unique_company_identities(&companies));
    assert!(has_presentation_equivalent_guid_siblings(&companies));
}

#[test]
fn identical_name_same_guid_distinct_books_are_not_stable_company_identities() {
    let companies = vec![
        crate::tally::TallyCompany {
            name: "Synthetic Company".to_string(),
            guid: Some("guid-3".to_string()),
            company_number: Some("100005".to_string()),
            books_from: Some("20250401".to_string()),
        },
        crate::tally::TallyCompany {
            name: "Synthetic Company".to_string(),
            guid: Some("GUID-3".to_string()),
            company_number: Some("100014".to_string()),
            books_from: Some("20260401".to_string()),
        },
    ];

    assert!(unique_company_identities(&companies));
    assert!(has_presentation_equivalent_guid_siblings(&companies));
}

#[test]
fn validates_tally_endpoint_components() {
    assert_eq!(
        tally_endpoint(&TallyConfig::default(), "/status")
            .expect("localhost endpoint")
            .as_str(),
        "http://127.0.0.1:9000/status"
    );
    let config = TallyConfig {
        host: "::1".to_string(),
        port: 9000,
    };
    assert_eq!(
        tally_endpoint(&config, "/status")
            .expect("IPv6 endpoint")
            .as_str(),
        "http://[::1]:9000/status"
    );
    for host in ["localhost", "127.0.0.1"] {
        assert_eq!(
            canonical_loopback_origin(&TallyConfig {
                host: host.to_string(),
                port: 9000,
            })
            .expect("canonical loopback origin"),
            "http://127.0.0.1:9000"
        );
    }
    assert_eq!(
        canonical_loopback_origin(&TallyConfig {
            host: "::1".to_string(),
            port: 9000,
        })
        .expect("canonical IPv6 loopback origin"),
        "http://[::1]:9000"
    );

    for host in ["http://localhost", "localhost/path", "user@localhost", ""] {
        let invalid = TallyConfig {
            host: host.to_string(),
            port: 9000,
        };
        assert!(tally_endpoint(&invalid, "/status").is_err());
    }

    for host in [
        "192.168.1.10",
        "10.0.0.5",
        "169.254.1.1",
        "224.0.0.1",
        "8.8.8.8",
        "tally.internal",
    ] {
        let remote = TallyConfig {
            host: host.to_string(),
            port: 9000,
        };
        assert!(tally_endpoint(&remote, "/status").is_err());
    }
}

#[test]
fn decodes_supported_xml_byte_order_marks_and_rejects_invalid_sequences() {
    let utf8 = [b"\xEF\xBB\xBF".as_slice(), b"<ENVELOPE />"].concat();
    assert_eq!(decode_xml_bytes(utf8).expect("UTF-8 BOM"), "<ENVELOPE />");

    let document = "<ENVELOPE><NAME>नमस्ते</NAME></ENVELOPE>";
    let mut utf16le = vec![0xFF, 0xFE];
    utf16le.extend(document.encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(decode_xml_bytes(utf16le).expect("UTF-16LE"), document);

    let mut utf16be = vec![0xFE, 0xFF];
    utf16be.extend(document.encode_utf16().flat_map(u16::to_be_bytes));
    assert_eq!(decode_xml_bytes(utf16be).expect("UTF-16BE"), document);

    assert!(decode_xml_bytes(vec![0xFF, 0xFE, 0x00]).is_err());
    assert!(decode_xml_bytes(vec![0x80]).is_err());
}

#[tokio::test]
async fn tally_requests_ignore_configured_proxy() {
    let tally_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let tally_address = tally_listener.local_addr().expect("Tally address");
    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic proxy");
    let proxy_address = proxy_listener.local_addr().expect("proxy address");

    let tally_server = tokio::spawn(async move {
        let accepted = tokio::time::timeout(Duration::from_secs(2), tally_listener.accept())
            .await
            .expect("Tally request timed out")
            .expect("accept Tally request");
        let (mut socket, _) = accepted;
        let request = read_complete_http_request(&mut socket).await;
        assert!(
            String::from_utf8_lossy(&request).starts_with("GET /status HTTP/1.1"),
            "request should go directly to the Tally endpoint"
        );
        let body = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
        socket
            .write_all(&utf8_status_response(body))
            .await
            .expect("write Tally response");
    });

    let proxy_server = tokio::spawn(async move {
        match tokio::time::timeout(Duration::from_millis(750), proxy_listener.accept()).await {
            Ok(Ok((mut socket, _))) => {
                let response =
                    "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write proxy response");
                true
            }
            Ok(Err(error)) => panic!("accept proxy request: {error}"),
            Err(_) => false,
        }
    });

    let client = TallyClient::with_http_builder(
        TallyConfig {
            host: tally_address.ip().to_string(),
            port: tally_address.port(),
        },
        reqwest::Client::builder().proxy(
            reqwest::Proxy::all(format!("http://{proxy_address}")).expect("synthetic proxy URL"),
        ),
    );

    let status = client
        .check_connection()
        .await
        .expect("check synthetic Tally connection");
    tally_server.await.expect("synthetic Tally server task");
    let proxy_received_request = proxy_server.await.expect("synthetic proxy task");

    assert!(status.reachable, "direct Tally response should be accepted");
    assert!(
        status.compatible,
        "synthetic Tally status should be recognized"
    );
    assert!(
        !proxy_received_request,
        "Tally traffic must never be sent through a configured proxy"
    );
}

#[cfg(feature = "voucher-scan")]
#[tokio::test]
async fn paired_outstandings_reads_health_check_between_and_after_requests() {
    const OPTIONAL_VOUCHERS: &str = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/unit_a_optional_voucher_live.xml"
    );
    const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    let identity = captured_company_identity();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        for (index, expected_prefix) in [
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
            "POST / HTTP/1.1",
            "GET /status HTTP/1.1",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("paired request timed out")
                .expect("accept paired request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected_prefix),
                "request {index} did not follow the required read/health-check sequence"
            );
            let body = match index {
                0 | 2 | 12 | 14 => COMPANY_EXTENT_V2,
                4 | 6 | 8 | 10 => OPTIONAL_VOUCHERS,
                _ => STATUS,
            };
            let response = if index % 2 == 0 {
                utf16_xml_response(body)
            } else {
                utf8_status_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write paired response");
        }
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let extent = client
        .fetch_company_book_extent(&identity)
        .await
        .expect("paired extent reads remain stable");
    let reporting_window = bridge_tally_protocol::outstandings::DateWindow::parse(
        bridge_tally_protocol::outstandings::DateBoundaryProfile::ModeAgnostic,
        "20260401",
        "20260401",
    )
    .expect("synthetic one-day window");
    let segment_window = reporting_window
        .narrow_partitions()
        .expect("one narrow partition")
        .remove(0);
    let observation = client
        .fetch_outstandings_segment_pair(
            extent.company(),
            segment_window.clone(),
            bridge_tally_protocol::outstandings::AlterIdRange::new(0, 101603)
                .expect("non-empty synthetic range"),
        )
        .await
        .expect("paired segment reads remain stable");
    let verification = observation.verification;
    let bridge_tally_protocol::outstandings::SegmentVerification::Complete(segment) = verification
    else {
        panic!("captured voucher response did not verify: {verification:?}");
    };
    assert!(
        !segment.vouchers().is_empty(),
        "captured voucher fixture unexpectedly had no vouchers"
    );
    let witness = client
        .fetch_empty_partition_witness_pair(extent.company(), segment_window)
        .await
        .expect("paired empty-date witness reads remain stable");
    let bridge_tally_protocol::outstandings::WitnessPairVerification::Complete(witness) = witness
    else {
        panic!("captured witness fixture did not verify: {witness:?}");
    };
    assert!(
        !witness.vouchers().is_empty(),
        "captured witness fixture unexpectedly had no identity rows"
    );
    let closing_extent = client
        .fetch_company_book_extent(&identity)
        .await
        .expect("closing paired extent reads remain stable");
    assert_eq!(closing_extent, extent, "synthetic book did not change");
    assert_eq!(
        client.observed_body_bytes(),
        Some(
            u64::try_from(
                bridge_tally_protocol::encode_tally_xml_request_utf16le(OPTIONAL_VOUCHERS).len(),
            )
            .expect("encoded fixture length fits u64"),
        ),
        "closing extent evidence must not overwrite the larger voucher payload"
    );
    server.await.expect("synthetic Tally server task");
}

#[cfg(feature = "voucher-scan")]
#[tokio::test]
async fn paired_ledger_opening_coverage_reports_intra_pair_drift() {
    const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    let identity = captured_company_identity();

    let coverage = |name: &str| {
        format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><LEDGER NAME=\"{name}\"><GUID>bb8ad19e-6aef-4239-a917-87fec0c6215e-00000001</GUID><ISBILLWISEON>Yes</ISBILLWISEON><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"
        )
    };
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let responses = vec![
        COMPANY_EXTENT_V2.to_string(),
        STATUS.to_string(),
        COMPANY_EXTENT_V2.to_string(),
        STATUS.to_string(),
        coverage("Before Rename"),
        STATUS.to_string(),
        coverage("After Rename"),
        STATUS.to_string(),
    ];
    let server = tokio::spawn(async move {
        for (index, body) in responses.into_iter().enumerate() {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("paired request timed out")
                .expect("accept paired request");
            let request = read_complete_http_request(&mut socket).await;
            let expected_prefix = if index % 2 == 0 {
                "POST / HTTP/1.1"
            } else {
                "GET /status HTTP/1.1"
            };
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected_prefix),
                "request {index} did not follow the required read/health-check sequence"
            );
            let response = if index % 2 == 0 {
                utf16_xml_response(body)
            } else {
                utf8_status_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write paired response");
        }
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let extent = client
        .fetch_company_book_extent(&identity)
        .await
        .expect("paired extent reads remain stable");
    assert!(matches!(
        client
            .fetch_ledger_opening_coverage(extent.company())
            .await
            .expect("coverage responses parse"),
        LedgerOpeningCoverageRead::Drifted
    ));
    server.await.expect("synthetic Tally server task");
}

/// The outstandings bracket (`fetch_company_book_extent`, feeding both
/// `fetch_outstandings_native_with_currency` and `fetch_ledgers`) must fail closed with
/// a typed error when both paired reads agree but neither carries
/// `ALTMSTID`. This is the exact case the review flagged: two
/// witness-less extents compare equal, so the ordinary `first != second`
/// drift check alone cannot tell a stable book from one where a
/// GROUP/LEDGER master moved mid-window without a signal to detect it.
/// Uses only a targeted mutation of the captured V2 response. The
/// production parser must continue refusing a stable pair without the
/// master witness instead of accepting a fabricated positive fixture.
#[tokio::test]
async fn outstandings_bracket_fails_closed_when_altmstid_is_absent() {
    const STATUS: &str = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    const CAPTURED_TARGET_WITNESS: &str = concat!(
        "<GUID TYPE=\"String\">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>\r\n",
        "     <COMPANYNUMBER TYPE=\"Number\"> 100000</COMPANYNUMBER>\r\n",
        "     <ALTVCHID TYPE=\"Number\"> 101605</ALTVCHID>\r\n",
        "     <ALTMSTID TYPE=\"Number\"> 328</ALTMSTID>"
    );
    const CAPTURED_TARGET_WITHOUT_WITNESS: &str = concat!(
        "<GUID TYPE=\"String\">bb8ad19e-6aef-4239-a917-87fec0c6215e</GUID>\r\n",
        "     <COMPANYNUMBER TYPE=\"Number\"> 100000</COMPANYNUMBER>\r\n",
        "     <ALTVCHID TYPE=\"Number\"> 101605</ALTVCHID>"
    );
    let identity = captured_company_identity();
    assert_eq!(
        COMPANY_EXTENT_V2.matches(CAPTURED_TARGET_WITNESS).count(),
        1,
        "the exact captured target witness must appear once before mutation"
    );
    let company_extent_without_altmstid =
        COMPANY_EXTENT_V2.replacen(CAPTURED_TARGET_WITNESS, CAPTURED_TARGET_WITHOUT_WITNESS, 1);
    assert!(
        !company_extent_without_altmstid.contains(CAPTURED_TARGET_WITNESS),
        "the captured target witness mutation must apply"
    );
    let parsed_without_witness =
        bridge_tally_protocol::outstandings_shared::parse_company_book_extent_v2(
            &company_extent_without_altmstid,
            &identity
                .company_book_extent_expectation()
                .expect("captured expectation"),
        )
        .expect("the captured negative mutation must remain a parseable extent");
    assert!(
        parsed_without_witness
            .master_alter_id_high_water()
            .is_none(),
        "the target witness mutation must remove only the target ALTMSTID"
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let responses = [
        company_extent_without_altmstid.clone(),
        STATUS.to_string(),
        company_extent_without_altmstid,
        STATUS.to_string(),
    ];
    let server = tokio::spawn(async move {
        for (index, body) in responses.into_iter().enumerate() {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .expect("paired request timed out")
                .expect("accept paired request");
            let request = read_complete_http_request(&mut socket).await;
            let expected_prefix = if index % 2 == 0 {
                "POST / HTTP/1.1"
            } else {
                "GET /status HTTP/1.1"
            };
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected_prefix),
                "request {index} did not follow the required read/health-check sequence"
            );
            let response = if index % 2 == 0 {
                utf16_xml_response(&body)
            } else {
                utf8_status_response(&body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write paired response");
        }
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let error = client
        .fetch_company_book_extent(&identity)
        .await
        .expect_err("a stable pair without ALTMSTID must still fail closed");
    server.await.expect("synthetic Tally server task");
    assert!(
        error
            .downcast_ref::<bridge_tally_protocol::outstandings_shared::OutstandingsError>()
            .is_some_and(|error| {
                *error
                    == bridge_tally_protocol::outstandings_shared::OutstandingsError::MasterWitnessAbsent
            }),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn http_success_with_tally_status_zero_is_not_an_empty_success() {
    let identity = captured_company_identity();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        for (index, body) in [
            COMPANY_EXTENT_V2,
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            COMPANY_EXTENT_V2,
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY /></ENVELOPE>",
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY /></ENVELOPE>",
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            let expected = if index == 1 || index == 3 || index == 5 || index == 7 {
                "GET /status HTTP/1.1"
            } else {
                "POST / HTTP/1.1"
            };
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected),
                "ledger fetch did not preserve its extent/read sequence"
            );
            if index == 4 || index == 6 {
                let body_start = request
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|offset| offset + 4)
                    .expect("native ledger POST has complete HTTP headers");
                let request_xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
                    &request[body_start..],
                    request.len(),
                )
                .expect("native ledger POST uses decodable UTF-16 XML")
                .text;
                assert!(request_xml.contains(r#"<SVFROMDATE TYPE="Date">20240401</SVFROMDATE>"#));
                assert!(request_xml.contains(r#"<SVTODATE TYPE="Date">20260401</SVTODATE>"#));
            }
            let response = if index == 1 || index == 3 || index == 5 || index == 7 {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write Tally response");
        }
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let error = client
        .fetch_ledgers(&identity, DateBoundaryProfile::ModeAgnostic)
        .await
        .expect_err("STATUS 0 must not become an empty ledger result");
    server.await.expect("synthetic Tally server task");
    assert!(
        error
            .to_string()
            .contains("native ledger collection did not report success"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn invalid_book_extent_stops_ledger_export_without_a_date_fallback() {
    let identity = captured_company_identity();
    let invalid_extent =
        COMPANY_EXTENT_V2.replacen(r#"<BOOKSFROM TYPE="Date">20240401</BOOKSFROM>"#, "", 1);
    assert_ne!(invalid_extent, COMPANY_EXTENT_V2);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        for (index, body) in [
            invalid_extent.as_str(),
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            invalid_extent.as_str(),
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept extent request");
            let request = read_complete_http_request(&mut socket).await;
            let expected = if index % 2 == 0 {
                "POST / HTTP/1.1"
            } else {
                "GET /status HTTP/1.1"
            };
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected),
                "request {index} did not follow the paired extent sequence"
            );
            let response = if index % 2 == 0 {
                utf16_xml_response(body)
            } else {
                utf8_status_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write extent response");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "an invalid extent must stop before any native ledger request"
        );
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    client
        .fetch_ledgers(&identity, DateBoundaryProfile::ModeAgnostic)
        .await
        .expect_err("missing BOOKSFROM must fail closed");
    server.await.expect("synthetic Tally server task");
}

#[tokio::test]
async fn education_profile_rejects_an_unsupported_books_from_before_ledger_export() {
    let extent = COMPANY_EXTENT_V2.replacen(
        r#"<BOOKSFROM TYPE="Date">20240401</BOOKSFROM>"#,
        r#"<BOOKSFROM TYPE="Date">20240115</BOOKSFROM>"#,
        1,
    );
    let identity = identity_from_company_extent(&extent);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        for (index, body) in [
            extent.as_str(),
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            extent.as_str(),
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept extent request");
            let request = read_complete_http_request(&mut socket).await;
            let expected = if index % 2 == 0 {
                "POST / HTTP/1.1"
            } else {
                "GET /status HTTP/1.1"
            };
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected),
                "request {index} did not follow the paired extent sequence"
            );
            let response = if index % 2 == 0 {
                utf16_xml_response(body)
            } else {
                utf8_status_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write extent response");
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "an Education-invalid BOOKSFROM must stop before the native ledger request"
        );
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let error = client
        .fetch_ledgers(&identity, DateBoundaryProfile::EducationRestricted)
        .await
        .expect_err("unsupported boundary must not reach the ledger export");
    server.await.expect("synthetic Tally server task");
    assert!(
        error
            .to_string()
            .contains("not supported by the endpoint compatibility profile"),
        "unexpected error: {error:#}"
    );
}

fn native_voucher_collection_xml(rows: &[&str]) -> String {
    format!(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>{}</COLLECTION></DATA></BODY></ENVELOPE>",
        rows.join("")
    )
}

const SYNTHETIC_VOUCHER_ROW: &str = r#"<VOUCHER REMOTEID="synthetic-company-guid-00000001"><DATE TYPE="Date">20260401</DATE><GUID TYPE="String">synthetic-company-guid-00000001</GUID><MASTERID TYPE="Number">1</MASTERID><ALTERID TYPE="Number">1</ALTERID><VOUCHERTYPENAME TYPE="String">Payment</VOUCHERTYPENAME><ISCANCELLED TYPE="String">No</ISCANCELLED><ISOPTIONAL TYPE="String">No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME TYPE="String">Cash</LEDGERNAME><AMOUNT TYPE="Amount">-100.00</AMOUNT><ISDEEMEDPOSITIVE TYPE="String">Yes</ISDEEMEDPOSITIVE></ALLLEDGERENTRIES.LIST></VOUCHER>"#;

/// A native Voucher collection carries no envelope company GUID, and a
/// zero-row response has no per-row GUID either -- there is nothing for
/// `parse_native_voucher_source_records_with_evidence` to bind. Before
/// the fix, `fetch_vouchers` accepted such a response unauthenticated,
/// so a silently substituted or dropped company looked identical to a
/// genuinely empty window. This reproduces that: the voucher read comes
/// back empty, and the out-of-band book-extent bracket that should
/// confirm the pinned company instead observes a different one.
#[tokio::test]
async fn empty_voucher_response_is_rejected_when_pinned_company_cannot_be_confirmed() {
    let identity = captured_company_identity();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let empty_vouchers = native_voucher_collection_xml(&[]);
    let substituted_extent =
        COMPANY_EXTENT_V2.replacen(CAPTURED_COMPANY_GUID, "substituted-company-guid", 1);
    let status = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    let steps: Vec<(&str, String, bool)> = vec![
        ("POST / HTTP/1.1", empty_vouchers, false),
        ("POST / HTTP/1.1", substituted_extent.clone(), false),
        ("GET /status HTTP/1.1", status.to_string(), true),
        ("POST / HTTP/1.1", substituted_extent, false),
        ("GET /status HTTP/1.1", status.to_string(), true),
    ];
    let server = tokio::spawn(async move {
        for (index, (expected_prefix, body, is_status)) in steps.into_iter().enumerate() {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "request {index} timed out -- the empty-voucher rejection path \
                                 did not attempt to confirm the pinned company out of band"
                    )
                })
                .expect("accept synthetic Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected_prefix),
                "request {index} did not follow the voucher-then-extent-bracket sequence"
            );
            let response = if is_status {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write synthetic Tally response");
        }
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let error = client
        .fetch_vouchers(&identity, "20260401", "20260401")
        .await
        .expect_err(
            "an empty voucher response must not be accepted when the pinned company book \
                 extent cannot be confirmed",
        );
    server.await.expect("synthetic Tally server task");
    assert!(
        error
            .to_string()
            .contains("empty voucher response could not confirm the pinned company book extent"),
        "unexpected error: {error:#}"
    );
}

/// Same empty voucher response as above, but this time the out-of-band
/// book-extent bracket confirms the pinned company is still selected and
/// stable -- so the empty result is accepted.
#[tokio::test]
async fn empty_voucher_response_is_accepted_when_bracket_confirms_pinned_company() {
    let identity = captured_company_identity();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let empty_vouchers = native_voucher_collection_xml(&[]);
    let confirmed_extent = COMPANY_EXTENT_V2.to_string();
    let status = "<RESPONSE>TallyPrime Server is Running</RESPONSE>";
    let steps: Vec<(&str, String, bool)> = vec![
        ("POST / HTTP/1.1", empty_vouchers, false),
        ("POST / HTTP/1.1", confirmed_extent.clone(), false),
        ("GET /status HTTP/1.1", status.to_string(), true),
        ("POST / HTTP/1.1", confirmed_extent, false),
        ("GET /status HTTP/1.1", status.to_string(), true),
    ];
    let request_count = steps.len();
    let server = tokio::spawn(async move {
        for (index, (expected_prefix, body, is_status)) in steps.into_iter().enumerate() {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap_or_else(|_| panic!("request {index} timed out"))
                .expect("accept synthetic Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                String::from_utf8_lossy(&request).starts_with(expected_prefix),
                "request {index} did not follow the voucher-then-extent-bracket sequence"
            );
            let response = if is_status {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write synthetic Tally response");
        }
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let vouchers = client
        .fetch_vouchers(&identity, "20260401", "20260401")
        .await
        .expect("an empty voucher response confirmed by the extent bracket must be accepted");
    server.await.expect("synthetic Tally server task");
    assert!(vouchers.is_empty());
    assert_eq!(
        request_count, 5,
        "the empty path must pay for exactly one voucher read plus the extent bracket"
    );
}

/// A non-empty voucher response keeps its existing row-GUID binding and
/// must not pay for the extent bracket -- the common case stays a single
/// request.
#[tokio::test]
async fn non_empty_voucher_response_issues_no_extra_request() {
    let identity = captured_company_identity();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let voucher_row =
        SYNTHETIC_VOUCHER_ROW.replace("synthetic-company-guid", CAPTURED_COMPANY_GUID);
    let non_empty_vouchers = native_voucher_collection_xml(&[&voucher_row]);

    let server = tokio::spawn(async move {
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("voucher request timed out")
            .expect("accept synthetic Tally request");
        let request = read_complete_http_request(&mut socket).await;
        assert!(
            String::from_utf8_lossy(&request).starts_with("POST / HTTP/1.1"),
            "unexpected request for the non-empty voucher read"
        );
        socket
            .write_all(&utf16_xml_response(non_empty_vouchers))
            .await
            .expect("write synthetic Tally response");

        // A non-empty response must not pay for the extent bracket: no
        // further connection should ever arrive.
        let extra = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
        assert!(
            extra.is_err(),
            "non-empty voucher fetch issued an unexpected extra request"
        );
    });

    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client");
    let vouchers = client
        .fetch_vouchers(&identity, "20260401", "20260401")
        .await
        .expect("non-empty voucher fetch must still succeed exactly as today");
    server.await.expect("synthetic Tally server task");
    assert_eq!(vouchers.len(), 1);
}

#[tokio::test]
async fn capability_probe_reports_only_observed_xml_support() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        for (index, body) in [
            "<RESPONSE>LOCAL STATUS HEURISTIC UNRECOGNIZED</RESPONSE>",
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"Synthetic Company\"><GUID TYPE=\"String\">guid-1</GUID><COMPANYNUMBER TYPE=\"Number\">100001</COMPANYNUMBER><BOOKSFROM TYPE=\"Date\">20260401</BOOKSFROM><PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME><EDUMODE>No</EDUMODE><SILVER>Yes</SILVER><GOLD>No</GOLD></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(!request.is_empty(), "synthetic Tally request must not be empty");
            let response = if index == 0 {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write Tally response");
        }
    });

    let probe = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .probe()
    .await
    .expect("probe synthetic Tally endpoint");
    server.await.expect("synthetic Tally server task");

    assert!(probe.connection.reachable);
    assert!(!probe.connection.compatible);
    assert_eq!(probe.companies.len(), 1);
    assert_eq!(
        probe.profile.transports[&TransportId::XmlHttp].state,
        CapabilityState::Supported
    );
    assert_eq!(
        probe.profile.packs[&CapabilityPackId::CoreAccounting].state,
        CapabilityState::Unknown
    );
    assert_eq!(probe.profile.product, "TallyPrime");
    assert!(probe.profile.release.is_none());
    assert_eq!(probe.profile.mode.as_deref(), Some("Licensed"));
    assert_eq!(probe.profile.profile_version, 4);
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::ProductAndMode].state,
        CapabilityState::Supported
    );
    let boundary = crate::tally::TallyRuntime::default()
        .master_ledger_export_boundary_profile_from_profile(Some(&probe.profile));
    assert_eq!(boundary, DateBoundaryProfile::ModeAgnostic);
    assert!(NativeLedgerExportPeriod::new(
        boundary,
        TallyDate::parse("20240115").expect("valid mid-month date"),
        TallyDate::parse("20240115").expect("valid mid-month date"),
    )
    .is_ok());
    for transport in [TransportId::TdlCompanion, TransportId::Odbc] {
        let evidence = &probe.profile.transports[&transport];
        assert_eq!(evidence.state, CapabilityState::Unknown);
        assert_eq!(evidence.confidence, EvidenceConfidence::Unknown);
        assert_eq!(
            evidence.safe_reason_code.as_deref(),
            Some("configuration_not_observed")
        );
    }
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::EndpointReachability].state,
        CapabilityState::Supported
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::LoadedCompanies].state,
        CapabilityState::Supported
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity].state,
        CapabilityState::Supported
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::EncodingBehaviour]
            .safe_reason_code
            .as_deref(),
        Some("utf16_le_bom_observed")
    );
    for feature in [
        CapabilityFeatureId::PracticalResponseLimit,
        CapabilityFeatureId::LedgerRead,
        CapabilityFeatureId::VoucherRead,
        CapabilityFeatureId::Write,
    ] {
        assert_eq!(
            probe.profile.features[&feature].state,
            CapabilityState::Unknown
        );
    }
}

#[tokio::test]
async fn capability_probe_records_unavailable_product_mode_evidence_without_refusing() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        let responses = [
            utf8_status_response("<RESPONSE>TallyPrime Server is Running</RESPONSE>"),
            utf16_xml_response(
                "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Capability collection unavailable</LINEERROR></DATA></BODY></ENVELOPE>",
            ),
            utf16_xml_response(
                "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
            ),
        ];
        for response in responses {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(
                !request.is_empty(),
                "synthetic Tally request must not be empty"
            );
            socket
                .write_all(&response)
                .await
                .expect("write Tally response");
        }
    });

    let probe = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .probe()
    .await
    .expect("unavailable product/mode evidence must not refuse the probe");
    server.await.expect("synthetic Tally server task");

    assert_eq!(probe.profile.product, "Unknown");
    assert!(probe.profile.mode.is_none());
    assert_eq!(
        crate::tally::TallyRuntime::default()
            .master_ledger_export_boundary_profile_from_profile(Some(&probe.profile)),
        DateBoundaryProfile::ModeAgnostic
    );
    let evidence = &probe.profile.features[&CapabilityFeatureId::ProductAndMode];
    assert_eq!(evidence.state, CapabilityState::Unknown);
    assert_eq!(evidence.confidence, EvidenceConfidence::Observed);
    assert_eq!(
        evidence.safe_reason_code.as_deref(),
        Some("product_mode_evidence_unavailable")
    );
}

/// `fetch_companies` now requests the native `Company` collection
/// (`ReadOnlyProfile::CompanyListV2`) instead of the legacy `CompanyListV1`
/// custom TDL report. This asserts the request itself carries that shape --
/// `TYPE=Collection`, no `REPORT`/`FORM`/`PART`/`LINE`/`FIELD` stack, and no
/// `SVCURRENTCOMPANY` scoping a discovery read to one company -- and that a
/// company row's `NAME` attribute and nested `GUID` are trimmed and returned.
#[tokio::test]
async fn interactive_company_fetch_sends_the_native_collection_request() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        let body = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="  Synthetic Company  "><GUID TYPE="String">  guid-1  </GUID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let (mut socket, _) = listener.accept().await.expect("accept Tally request");
        let request = read_complete_http_request(&mut socket).await;
        let body_start = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
            .expect("POST request has complete HTTP headers");
        let post_xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
            &request[body_start..],
            request.len(),
        )
        .expect("POST request uses decodable UTF-16 XML")
        .text;
        assert_company_collection_request_shape(&post_xml);
        assert!(!post_xml.contains("<SVCURRENTCOMPANY"));
        assert!(!post_xml.contains("<REPORT"));
        assert!(!post_xml.contains("<FORM "));
        socket
            .write_all(&utf16_xml_response(body))
            .await
            .expect("write Tally response");
    });

    let companies = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .fetch_companies()
    .await
    .expect("interactive company discovery accepts the native collection response");
    server.await.expect("synthetic Tally server task");

    assert_eq!(companies.len(), 1);
    assert_eq!(companies[0].name, "Synthetic Company");
    assert_eq!(companies[0].guid.as_deref(), Some("guid-1"));
}

/// A collection response omitting a company's `GUID` must fail closed --
/// the whole discovery read is rejected rather than silently returning an
/// identity-less company, since identity is what every other read binds
/// against.
#[tokio::test]
async fn interactive_company_fetch_fails_closed_without_a_guid() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        let body = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
        let (mut socket, _) = listener.accept().await.expect("accept Tally request");
        let request = read_complete_http_request(&mut socket).await;
        assert!(
            !request.is_empty(),
            "synthetic Tally request must not be empty"
        );
        socket
            .write_all(&utf16_xml_response(body))
            .await
            .expect("write Tally response");
    });

    let error = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .fetch_companies()
    .await
    .expect_err("a company row without a GUID must fail closed");
    server.await.expect("synthetic Tally server task");
    assert!(error.to_string().contains("GUID"));
    let retained = error
        .downcast_ref::<crate::tally::runtime::RuntimeReadFailure>()
        .expect("parser refusal retains its typed wire-evidence boundary");
    assert!(retained.evidence.bytes > 0);
}

#[tokio::test]
async fn direct_company_bootstrap_uses_only_the_shaped_collection_identity() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    // `fetch_companies` (the first of the two reads below) now requests the
    // native `Company` collection (`ReadOnlyProfile::CompanyListV2`), so its
    // response carries that shape rather than the legacy `CompanyListV1`
    // direct report. Its GUID must still not escape into the returned
    // identity -- only the second, scoped `standard` read may do that.
    let discovered = r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME="Synthetic Company"><GUID TYPE="String">scoped-guid</GUID><COMPANYNUMBER TYPE="Number">100001</COMPANYNUMBER><BOOKSFROM TYPE="Date">20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"#;
    let standard = "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DESC><CMPINFO /></DESC><DATA><COLLECTION MSTDEPTYPE=\"Ledger\" ISMSTDEPTYPE=\"Yes\"><SyntheticLedger NAME=\"synthetic-ledger\" RESERVEDNAME=\"\"><GUID TYPE=\"String\">ledger-guid</GUID><PARENT TYPE=\"String\">Primary</PARENT><BRIDGECOMPANYGUID TYPE=\"String\">scoped-guid</BRIDGECOMPANYGUID><BRIDGECOMPANYNAME TYPE=\"String\">Synthetic Company</BRIDGECOMPANYNAME><LANGUAGENAME.LIST><LANGUAGEID>1033</LANGUAGEID></LANGUAGENAME.LIST></SyntheticLedger></COLLECTION></DATA></BODY></ENVELOPE>";
    let server = tokio::spawn(async move {
        for body in [discovered, standard] {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(!request.is_empty());
            socket
                .write_all(&utf16_xml_response(body))
                .await
                .expect("write Tally response");
        }
    });

    let company = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .bootstrap_direct_company("Synthetic Company")
    .await
    .expect("strict scoped bootstrap should succeed");
    server.await.expect("synthetic Tally server task");

    assert_eq!(company.name, "Synthetic Company");
    assert_eq!(company.guid.as_deref(), Some("scoped-guid"));
    assert_eq!(company.company_number.as_deref(), Some("100001"));
    assert_eq!(company.books_from.as_deref(), Some("20260401"));
}

#[tokio::test]
async fn capability_probe_does_not_promote_a_direct_company_report() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        // This responder gives the same untrusted bare direct company
        // report regardless of what is requested: once for the `V2`
        // collection attempt (which the collection parser rejects, since
        // it never satisfies `HEADER/STATUS`), and once more for the
        // `V1` fallback that follows.
        for (index, body) in [
            "<RESPONSE>LOCAL STATUS HEURISTIC UNRECOGNIZED</RESPONSE>",
            "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
            "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>Synthetic Company</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-1</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(!request.is_empty(), "synthetic Tally request must not be empty");
            let response = if index == 0 {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write Tally response");
        }
    });

    let probe = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .probe()
    .await
    .expect("probe synthetic Tally endpoint");
    server.await.expect("synthetic Tally server task");

    assert!(probe.connection.reachable);
    assert!(probe.companies.is_empty());
    assert_eq!(
        probe.profile.transports[&TransportId::XmlHttp].state,
        CapabilityState::Unknown
    );
    assert_eq!(
        probe.profile.transports[&TransportId::XmlHttp]
            .safe_reason_code
            .as_deref(),
        Some("direct_company_report_untrusted")
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::CompanyRead].state,
        CapabilityState::Unknown
    );
}

#[tokio::test]
async fn capability_probe_does_not_promote_a_shaped_company_failure_to_xml_support() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        // Same shaped `STATUS=0` failure for both the `V2` collection
        // attempt and the `V1` fallback that follows it.
        for (index, body) in [
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Could not find Company ''</LINEERROR></DATA></BODY></ENVELOPE>",
            "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>Could not find Company ''</LINEERROR></DATA></BODY></ENVELOPE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            assert!(!request.is_empty(), "synthetic Tally request must not be empty");
            let response = if index == 0 {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write Tally response");
        }
    });

    let probe = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .probe()
    .await
    .expect("probe synthetic Tally endpoint");
    server.await.expect("synthetic Tally server task");

    assert!(probe.companies.is_empty());
    let xml = &probe.profile.transports[&TransportId::XmlHttp];
    assert_eq!(xml.state, CapabilityState::Unknown);
    assert_eq!(xml.confidence, EvidenceConfidence::Observed);
    assert_eq!(xml.safe_reason_code.as_deref(), Some("company_not_loaded"));
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::LoadedCompanies].state,
        CapabilityState::Unknown
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity].state,
        CapabilityState::Unknown
    );
}

/// A gateway-shaped `Company` collection response with two loaded
/// synthetic companies and the `CMPINFO` counter trap included. Proves
/// `probe` requests `CompanyListV2` on the happy path
/// and trusts its success without ever falling back to the legacy
/// `CompanyListV1` report: the mock server has exactly one POST response
/// queued, so a fallback request would hang and fail this test.
#[tokio::test]
async fn capability_probe_marks_presentation_equivalent_guid_siblings_ambiguous() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind synthetic Tally server");
    let address = listener.local_addr().expect("synthetic Tally address");
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (index, body) in [
            "<RESPONSE>TallyPrime Server is Running</RESPONSE>",
            "<ENVELOPE>\n <HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER>\n <BODY><DESC><CMPINFO><COMPANY>0</COMPANY></CMPINFO></DESC>\n  <DATA><COLLECTION>\n   <COMPANY NAME=\"Synthetic Company A\" RESERVEDNAME=\"\"><NAME TYPE=\"String\">Synthetic Company A</NAME><GUID TYPE=\"String\">synthetic-guid-a</GUID><COMPANYNUMBER TYPE=\"Number\">100001</COMPANYNUMBER><BOOKSFROM TYPE=\"Date\">20260401</BOOKSFROM><PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME><EDUMODE TYPE=\"Logical\">No</EDUMODE><SILVER TYPE=\"Logical\">Yes</SILVER><GOLD TYPE=\"Logical\">No</GOLD></COMPANY>\n   <COMPANY NAME=\" synthetic company a \" RESERVEDNAME=\"\"><NAME TYPE=\"String\"> synthetic company a </NAME><GUID TYPE=\"String\">SYNTHETIC-GUID-A</GUID><COMPANYNUMBER TYPE=\"Number\">100002</COMPANYNUMBER><BOOKSFROM TYPE=\"Date\">20270401</BOOKSFROM><PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME><EDUMODE TYPE=\"Logical\">No</EDUMODE><SILVER TYPE=\"Logical\">Yes</SILVER><GOLD TYPE=\"Logical\">No</GOLD></COMPANY>\n  </COLLECTION></DATA>\n </BODY>\n</ENVELOPE>",
        ]
        .into_iter()
        .enumerate()
        {
            let (mut socket, _) = listener.accept().await.expect("accept Tally request");
            let request = read_complete_http_request(&mut socket).await;
            requests.push(request);
            let response = if index == 0 {
                utf8_status_response(body)
            } else {
                utf16_xml_response(body)
            };
            socket
                .write_all(&response)
                .await
                .expect("write Tally response");
        }
        requests
    });

    let probe = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .expect("build synthetic Tally client")
    .probe()
    .await
    .expect("probe synthetic Tally endpoint");
    let requests = server.await.expect("synthetic Tally server task");

    assert_eq!(probe.companies.len(), 2);
    assert_eq!(probe.companies[0].name, "Synthetic Company A");
    assert_eq!(probe.companies[0].guid.as_deref(), Some("synthetic-guid-a"));
    assert_eq!(probe.companies[1].name, "synthetic company a");
    assert_eq!(probe.companies[1].guid.as_deref(), Some("SYNTHETIC-GUID-A"));
    assert_eq!(probe.profile.product, "TallyPrime");
    assert_eq!(probe.profile.mode.as_deref(), Some("Licensed"));
    assert_eq!(
        probe.profile.transports[&TransportId::XmlHttp].state,
        CapabilityState::Supported
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity]
            .safe_reason_code
            .as_deref(),
        Some("company_identity_display_scope_ambiguous")
    );
    assert_eq!(
        probe.profile.features[&CapabilityFeatureId::StableCompanyIdentity].state,
        CapabilityState::Unknown
    );

    let post_request = requests
        .iter()
        .find(|request| request.starts_with(b"POST"))
        .expect("exactly one POST request was sent");
    let body_start = post_request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
        .expect("POST request has complete HTTP headers");
    let post_xml = bridge_tally_protocol::decode_tally_text_bytes_limited(
        &post_request[body_start..],
        post_request.len(),
    )
    .expect("POST request uses decodable UTF-16 XML");
    assert_company_collection_request_shape(&post_xml.text);
    assert!(post_xml.text.contains("<ID>BridgeCompanyExtent</ID>"));
}

/// The budget admits exactly as many counted ledgers as its estimate allows
/// and refuses one more, carrying the numbers it refused on (#637).
#[test]
fn the_compliance_budget_admits_up_to_its_estimate_and_refuses_one_ledger_more() {
    let limit = super::COMPLIANCE_MASTER_RESPONSE_BUDGET_BYTES_UNVERIFIED
        / super::COMPLIANCE_MASTER_BYTES_PER_LEDGER_UNVERIFIED;
    assert!(super::admit_compliance_master_read(usize::try_from(limit).unwrap()).is_ok());
    match super::admit_compliance_master_read(usize::try_from(limit + 1).unwrap()) {
        Err(super::PartyLedgerMasterSourceValidationError::TooLarge {
            ledgers,
            estimated_bytes,
            budget_bytes,
        }) => {
            assert_eq!(ledgers, limit + 1);
            assert_eq!(
                estimated_bytes,
                (limit + 1) * super::COMPLIANCE_MASTER_BYTES_PER_LEDGER_UNVERIFIED
            );
            assert_eq!(
                budget_bytes,
                super::COMPLIANCE_MASTER_RESPONSE_BUDGET_BYTES_UNVERIFIED
            );
        }
        other => panic!("expected a size refusal, got {other:?}"),
    }
}

/// The budget boundary itself, at figures that land exactly on it (#637): an
/// estimate equal to the budget fits, one byte over does not, and a count that
/// would overflow saturates and never fits.
#[test]
fn a_compliance_estimate_exactly_at_the_budget_fits_and_one_over_does_not() {
    assert_eq!(
        super::compliance_estimate(4, 250, 1_000),
        super::ComplianceEstimate {
            estimated_bytes: 1_000,
            fits: true
        }
    );
    assert_eq!(
        super::compliance_estimate(5, 250, 1_000),
        super::ComplianceEstimate {
            estimated_bytes: 1_250,
            fits: false
        }
    );
    assert_eq!(
        super::compliance_estimate(1_000, 1, 999),
        super::ComplianceEstimate {
            estimated_bytes: 1_000,
            fits: false
        }
    );
    assert_eq!(
        super::compliance_estimate(u64::MAX, 2, u64::MAX - 1),
        super::ComplianceEstimate {
            estimated_bytes: u64::MAX,
            fits: false
        }
    );
}
