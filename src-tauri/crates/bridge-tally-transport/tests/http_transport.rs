use std::time::Duration;

use bridge_tally_protocol::TallyTextEncoding;
use bridge_tally_transport::{
    TallyDecodedHttpResponse, TallyEndpointConfig, TallyHttpResponse, TallyHttpTransport,
    TallyTransportError, TransportPolicy,
};
use sha2::{Digest, Sha256};
use tally_protocol_simulator::{
    Delivery, Fixture, ResponseContentEncoding, ResponseFraming, ScenarioPlan, Simulator,
    WireEncoding,
};

fn endpoint(simulator: &Simulator) -> TallyEndpointConfig {
    TallyEndpointConfig {
        host: simulator.address().ip().to_string(),
        port: simulator.address().port(),
    }
}

fn policy(response_limit: usize, timeout: Duration) -> TransportPolicy {
    TransportPolicy {
        request_timeout: timeout,
        status_response_max_bytes: response_limit,
        xml_request_max_bytes: response_limit,
        xml_response_max_bytes: response_limit,
    }
}

async fn post_synthetic(
    plan: ScenarioPlan,
    transport_policy: Option<TransportPolicy>,
    xml: &str,
) -> (Simulator, Result<TallyHttpResponse, TallyTransportError>) {
    const MAX_HOST_ABORT_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_HOST_ABORT_ATTEMPTS {
        let simulator = Simulator::spawn(plan.clone()).expect("spawn loopback simulator");
        let transport = match transport_policy {
            Some(policy) => TallyHttpTransport::with_policy(endpoint(&simulator), policy),
            None => TallyHttpTransport::new(endpoint(&simulator)),
        }
        .expect("build synthetic transport");
        let result = transport.post_xml(xml.to_owned()).await;
        if attempt < MAX_HOST_ABORT_ATTEMPTS
            && matches!(&result, Err(TallyTransportError::RequestFailed))
        {
            // Windows endpoint-security software can occasionally abort a new
            // loopback TCP flow before any response exists. Recreate the
            // read-only synthetic fixture so the assertion still exercises a
            // completed deterministic exchange. Production retry policy is
            // tested separately and is not changed by this harness helper.
            drop(simulator);
            continue;
        }
        return (simulator, result);
    }
    unreachable!("bounded synthetic attempts always return")
}

async fn post_synthetic_decoded(
    plan: ScenarioPlan,
    transport_policy: Option<TransportPolicy>,
    xml: &str,
) -> (
    Simulator,
    Result<TallyDecodedHttpResponse, TallyTransportError>,
) {
    const MAX_HOST_ABORT_ATTEMPTS: usize = 3;
    for attempt in 1..=MAX_HOST_ABORT_ATTEMPTS {
        let simulator = Simulator::spawn(plan.clone()).expect("spawn loopback simulator");
        let transport = match transport_policy {
            Some(policy) => TallyHttpTransport::with_policy(endpoint(&simulator), policy),
            None => TallyHttpTransport::new(endpoint(&simulator)),
        }
        .expect("build synthetic decoded transport");
        let result = transport.post_xml_decoded(xml.to_owned()).await;
        if attempt < MAX_HOST_ABORT_ATTEMPTS
            && matches!(&result, Err(TallyTransportError::RequestFailed))
        {
            drop(simulator);
            continue;
        }
        return (simulator, result);
    }
    unreachable!("bounded synthetic attempts always return")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn production_transport_decodes_utf16_across_closed_framing_modes() {
    let cases = [
        (
            WireEncoding::Utf16Le,
            ResponseFraming::ContentLength,
            TallyTextEncoding::Utf16LeBom,
        ),
        (
            WireEncoding::Utf16Be,
            ResponseFraming::Chunked { chunk_bytes: 7 },
            TallyTextEncoding::Utf16BeBom,
        ),
        (
            WireEncoding::Utf8Bom,
            ResponseFraming::ConnectionClose,
            TallyTextEncoding::Utf8Bom,
        ),
    ];

    for (wire_encoding, framing, expected_encoding) in cases {
        let plan = ScenarioPlan::new(Fixture::ExportStatusOne)
            .with_encoding(wire_encoding)
            .with_framing(framing);
        let expected_encoded_body = plan.response_bytes();
        let (simulator, result) = post_synthetic(plan, None, "<ENVELOPE />").await;
        let response = result.expect("read encoded synthetic response");
        assert_eq!(response.encoding(), expected_encoding);
        assert_eq!(response.http_status(), 200);
        assert!(response.text().contains("<STATUS>1</STATUS>"));
        assert!(response.encoded_bytes() > 0);
        assert_eq!(response.encoded_body(), expected_encoded_body);
        let observed = simulator.finish().expect("finish simulator");
        assert_eq!(observed.method, "POST");
        assert_eq!(observed.path, "/");
        assert!(
            observed.request_content_type_is_tally_xml,
            "Tally's XML gateway requires the documented XML media type"
        );
        assert!(observed.request_processed);
    }
}

#[tokio::test]
async fn decoded_only_transport_streams_all_encodings_without_retaining_wire_body() {
    for (encoding, framing, expected_encoding) in [
        (
            WireEncoding::Utf8,
            ResponseFraming::ContentLength,
            TallyTextEncoding::Utf8,
        ),
        (
            WireEncoding::Utf8Bom,
            ResponseFraming::ConnectionClose,
            TallyTextEncoding::Utf8Bom,
        ),
        (
            WireEncoding::Utf16Le,
            ResponseFraming::Chunked { chunk_bytes: 1 },
            TallyTextEncoding::Utf16LeBom,
        ),
        (
            WireEncoding::Utf16Be,
            ResponseFraming::Chunked { chunk_bytes: 3 },
            TallyTextEncoding::Utf16BeBom,
        ),
    ] {
        let plan = ScenarioPlan::new(Fixture::ExportStatusOne)
            .with_encoding(encoding)
            .with_framing(framing);
        let encoded = plan.response_bytes();
        let expected_text = Fixture::ExportStatusOne.body();
        let (simulator, result) = post_synthetic_decoded(plan, None, "<E />").await;
        let response = result.expect("stream decoded-only response");

        assert_eq!(response.text(), expected_text);
        assert_eq!(response.encoding(), expected_encoding);
        assert_eq!(response.encoded_bytes(), encoded.len());
        assert_eq!(response.decoded_bytes(), expected_text.len());
        assert_eq!(response.encoded_sha256(), sha256_hex(&encoded));
        assert_eq!(
            response.decoded_sha256(),
            sha256_hex(expected_text.as_bytes())
        );
        assert_eq!(response.http_status(), 200);
        assert!(!format!("{response:?}").contains("<ENVELOPE>"));
        let observed = simulator.finish().expect("finish decoded-only simulator");
        assert!(
            observed.request_content_type_is_tally_xml,
            "decoded-only transport must preserve Tally's documented XML media type"
        );
    }
}

#[tokio::test]
async fn decoded_only_transport_enforces_decoded_cap_during_streaming() {
    const LIMIT: usize = 100;
    let plan = ScenarioPlan::new(Fixture::SyntheticXml("\u{20ac}".repeat(40)))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::Chunked { chunk_bytes: 1 });
    assert!(plan.response_bytes().len() <= LIMIT);
    let (simulator, result) =
        post_synthetic_decoded(plan, Some(policy(LIMIT, Duration::from_secs(2))), "<E />").await;
    assert_eq!(
        result.expect_err("decoded cap must fail while streaming"),
        TallyTransportError::ResponseTooLarge {
            limit: LIMIT,
            declared_by_peer: false,
        }
    );
    simulator.finish().expect("finish decoded-cap simulator");
}

#[tokio::test]
async fn decoded_response_cap_is_enforced_independently_of_encoded_cap() {
    const LIMIT: usize = 100;
    let plan = ScenarioPlan::new(Fixture::SyntheticXml("€".repeat(40)))
        .with_encoding(WireEncoding::Utf16Le);
    assert!(plan.response_bytes().len() <= LIMIT);
    let (simulator, result) =
        post_synthetic(plan, Some(policy(LIMIT, Duration::from_secs(2))), "<E />").await;
    assert_eq!(
        result.expect_err("decoded cap must fail"),
        TallyTransportError::ResponseTooLarge {
            limit: LIMIT,
            declared_by_peer: false,
        }
    );
    simulator.finish().expect("finish decoded-cap simulator");
}

#[tokio::test]
async fn exact_encoded_cap_is_inclusive_for_all_framing_modes() {
    const LIMIT: usize = 512;
    for framing in [
        ResponseFraming::ContentLength,
        ResponseFraming::ConnectionClose,
        ResponseFraming::Chunked { chunk_bytes: 31 },
    ] {
        let (simulator, result) = post_synthetic(
            ScenarioPlan::new(Fixture::Oversized {
                minimum_bytes: LIMIT,
            })
            .with_framing(framing),
            Some(policy(LIMIT, Duration::from_secs(2))),
            "<E />",
        )
        .await;
        let response = result.expect("accept exact response cap");
        assert_eq!(response.encoded_bytes(), LIMIT);
        simulator.finish().expect("finish exact-cap simulator");
    }
}

#[tokio::test]
async fn declared_and_streamed_cap_plus_one_fail_closed() {
    const LIMIT: usize = 512;
    for (framing, declared_by_peer) in [
        (ResponseFraming::ContentLength, true),
        (ResponseFraming::ConnectionClose, false),
        (ResponseFraming::Chunked { chunk_bytes: 29 }, false),
    ] {
        let (simulator, result) = post_synthetic(
            ScenarioPlan::new(Fixture::Oversized {
                minimum_bytes: LIMIT + 1,
            })
            .with_framing(framing),
            Some(policy(LIMIT, Duration::from_secs(2))),
            "<E />",
        )
        .await;
        let error = result.expect_err("cap plus one must fail");
        assert_eq!(
            error,
            TallyTransportError::ResponseTooLarge {
                limit: LIMIT,
                declared_by_peer,
            }
        );
        drop(simulator);
    }
}

#[tokio::test]
async fn truncated_declared_body_is_never_partial_success() {
    let fixture = Fixture::ExportStatusOne;
    let actual_bytes = fixture.body().len();
    let (simulator, result) = post_synthetic(
        ScenarioPlan::new(fixture).with_framing(ResponseFraming::DeclaredContentLength {
            bytes: actual_bytes + 17,
        }),
        None,
        "<E />",
    )
    .await;
    let error = result.expect_err("truncated response must fail");
    assert!(matches!(
        error,
        TallyTransportError::ResponseTruncated | TallyTransportError::ResponseReadFailed
    ));
    simulator.finish().expect("finish truncated simulator");
}

#[tokio::test]
async fn whole_request_deadline_covers_slow_headers() {
    let (simulator, result) = post_synthetic(
        ScenarioPlan::new(Fixture::ExportStatusOne)
            .with_delivery(Delivery::SlowHeaders(Duration::from_millis(150))),
        Some(policy(1024, Duration::from_millis(30))),
        "<E />",
    )
    .await;
    let error = result.expect_err("slow headers must time out");
    assert_eq!(error, TallyTransportError::RequestTimedOut);
    simulator.cancel();
    let observed = simulator.finish().expect("cancel slow simulator");
    assert!(observed.cancelled);
}

#[tokio::test]
async fn non_success_and_redirect_statuses_are_typed_failures() {
    for status in [302, 500] {
        let (simulator, result) = post_synthetic(
            ScenarioPlan::new(Fixture::ExportStatusOne).with_http_status(status),
            None,
            "<E />",
        )
        .await;
        let error = result.expect_err("non-success HTTP status must fail");
        assert_eq!(error, TallyTransportError::HttpStatus { status });
        drop(simulator);
    }
}

#[tokio::test]
async fn non_identity_content_encoding_is_rejected_before_body_interpretation() {
    for (encoding, expected) in [
        (ResponseContentEncoding::Identity, None),
        (
            ResponseContentEncoding::Gzip,
            Some(TallyTransportError::UnsupportedContentEncoding),
        ),
        (
            ResponseContentEncoding::DuplicateIdentityThenGzip,
            Some(TallyTransportError::UnsupportedContentEncoding),
        ),
    ] {
        let (simulator, result) = post_synthetic(
            ScenarioPlan::new(Fixture::ExportStatusOne).with_content_encoding(encoding),
            None,
            "<E />",
        )
        .await;
        match expected {
            Some(error) => assert_eq!(result.expect_err("encoding must fail"), error),
            None => assert!(result.is_ok(), "identity encoding must pass"),
        }
        drop(simulator);
    }
}

#[tokio::test]
async fn outbound_request_cap_is_checked_before_connecting() {
    let transport = TallyHttpTransport::with_policy(
        TallyEndpointConfig {
            host: "127.0.0.1".to_owned(),
            port: 9,
        },
        policy(64, Duration::from_millis(50)),
    )
    .expect("build bounded transport");
    let error = transport
        .post_xml("X".repeat(65))
        .await
        .expect_err("oversized request must be rejected locally");
    assert_eq!(error, TallyTransportError::RequestTooLarge { limit: 64 });
}
