use super::*;

#[test]
fn endpoint_identity_normalizes_names_without_collapsing_ip_families() {
    for host in ["localhost", "LOCALHOST", "127.0.0.1"] {
        assert_eq!(
            canonical_loopback_origin(&TallyEndpointConfig {
                host: host.to_owned(),
                port: 9000,
            })
            .expect("valid loopback endpoint"),
            "http://127.0.0.1:9000"
        );
    }
    assert_eq!(
        canonical_loopback_origin(&TallyEndpointConfig {
            host: "::1".to_owned(),
            port: 9000,
        })
        .expect("valid IPv6 loopback endpoint"),
        "http://[::1]:9000"
    );
}

#[test]
fn remote_and_url_shaped_hosts_are_rejected() {
    for host in [
        "",
        "http://localhost",
        "localhost/path",
        "user@localhost",
        "192.168.1.10",
        "tally.internal",
    ] {
        let error = canonical_loopback_origin(&TallyEndpointConfig {
            host: host.to_owned(),
            port: 9000,
        })
        .expect_err("endpoint must fail closed");
        assert_eq!(error.safe_code(), "endpoint_invalid");
    }
}

#[test]
fn policy_cannot_expand_production_caps_or_deadline() {
    assert_eq!(
        TransportPolicy::default().request_timeout,
        Duration::from_secs(20)
    );
    assert_eq!(
        TransportPolicy::default().xml_response_max_bytes,
        32 * 1024 * 1024
    );
    #[cfg(feature = "voucher-scan")]
    assert_eq!(OUTSTANDINGS_XML_RESPONSE_MAX_BYTES, 40 * 1024 * 1024);
    let expanded = TransportPolicy {
        request_timeout: MAX_REQUEST_TIMEOUT + Duration::from_millis(1),
        ..TransportPolicy::default()
    };
    assert!(matches!(
        expanded.validate(),
        Err(TallyTransportError::PolicyInvalid { .. })
    ));
    let expanded = TransportPolicy {
        xml_response_max_bytes: XML_RESPONSE_MAX_BYTES + 1,
        ..TransportPolicy::default()
    };
    assert!(matches!(
        expanded.validate(),
        Err(TallyTransportError::PolicyInvalid { .. })
    ));
}

#[test]
fn oversized_xml_source_is_rejected_before_wire_encoding() {
    let error = match prepare_tally_xml_request_with_encoder(&"X".repeat(65), 64, |source| {
        panic!(
            "wire encoder must not run for an oversized XML source of {} bytes",
            source.len()
        )
    }) {
        Err(error) => error,
        Ok(prepared) => panic!(
            "oversized XML source unexpectedly produced {} wire bytes",
            prepared.body.len()
        ),
    };
    assert_eq!(error, TallyTransportError::RequestTooLarge { limit: 64 });
}

#[test]
fn xml_request_cap_is_inclusive_for_utf8_source_bytes() {
    let source = "éé";
    assert_eq!(source.len(), 4);
    let prepared = prepare_tally_xml_request(source, source.len())
        .expect("exact source-byte boundary is accepted");
    assert_eq!(prepared.body, encode_tally_xml_request_utf16le(source));
    assert_eq!(prepared.body.len(), 6);
    assert_eq!(
        prepared.body_sha256,
        hex_digest(Sha256::digest(&prepared.body))
    );
}
