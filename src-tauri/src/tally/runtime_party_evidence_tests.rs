//! Refusal replay of captured reports; UTF-8 fault captures are sent in the
//! negotiated UTF-16LE encoding without changing their XML content.
use super::*;
use crate::tally::connection::PartyLedgerMasterSourceValidationError;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";
const MASTER: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-masters.utf16le.xml"
);
const IDENTITY_MISSING_BALANCE: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/ledger_snapshot_aarav.xml"
);
const BALANCE: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-party-balances.utf16le.xml"
);
const IDENTITY_MISSING_GROUP: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_aarav.xml"
);
const EXTENT: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
);
const BILLS: &[u8] = include_bytes!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/bills_receivable_aarav.xml"
);

fn captured_identity(extent: &str) -> VerifiedCompanyIdentity {
    let companies = bridge_tally_protocol::parse_companies_from_collection(extent).unwrap();
    let company = companies
        .iter()
        .find(|company| company.guid.as_deref() == Some(GUID))
        .expect("captured extent includes the party-master company");
    VerifiedCompanyIdentity::from_observed_companies(
        company.name.clone(),
        company.guid.clone().unwrap(),
        company.company_number.clone().unwrap(),
        company.books_from.clone().unwrap(),
        &companies,
    )
    .expect("captured party-master tuple is uniquely verified")
}

async fn refusal(
    retained: &[&'static [u8]],
    fail_transport: bool,
) -> (anyhow::Error, RuntimeReadEvidence) {
    let extent = std::str::from_utf8(EXTENT).unwrap();
    let identity = captured_identity(extent);
    let parsed_extent = bridge_tally_protocol::outstandings_shared::parse_company_book_extent_v2(
        extent,
        &identity.company_book_extent_expectation().unwrap(),
    )
    .unwrap();
    let assertion = PartyLedgerMasterCurrencyAssertion {
        assertion: OutstandingsCurrencyAssertion::Inr,
        decimal_places: 2,
        currency_read_extent: parsed_extent,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut bodies = vec![bridge_tally_protocol::encode_tally_xml_request_utf16le(
        extent,
    )];
    bodies.extend(retained.iter().map(|body| {
        if body.get(1) == Some(&0) || body.starts_with(&[0xff, 0xfe]) {
            body.to_vec()
        } else {
            bridge_tally_protocol::encode_tally_xml_request_utf16le(
                std::str::from_utf8(body).unwrap(),
            )
        }
    }));
    if fail_transport {
        bodies.push(Vec::new());
    }
    let server = tokio::spawn(async move {
        let mut evidence = RuntimeReadEvidence::empty();
        for (index, body) in bodies.iter().enumerate() {
            let mut requests = Vec::new();
            for paired in 0..4 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") {
                    headers.push(socket.read_u8().await.unwrap());
                    assert!(headers.len() <= 64 * 1024);
                }
                let headers = String::from_utf8(headers).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length <= 1024 * 1024);
                let mut request = vec![0; length];
                socket.read_exact(&mut request).await.unwrap();
                if fail_transport && index + 1 == bodies.len() {
                    assert_eq!(paired, 0);
                    assert!(headers.starts_with("POST / "));
                    socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                    socket.shutdown().await.unwrap();
                    return evidence;
                }
                let response_body = if paired % 2 == 0 {
                    assert!(headers.starts_with("POST / "));
                    requests.push(request);
                    body.as_slice()
                } else {
                    assert!(headers.starts_with("GET /status "));
                    b"<RESPONSE>TallyPrime Server is Running</RESPONSE>"
                };
                let encoding = if response_body.starts_with(&[0xff, 0xfe])
                    || response_body.get(1) == Some(&0)
                {
                    "utf-16"
                } else {
                    "utf-8"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml; charset={encoding}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response_body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(response_body).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            assert_eq!(requests[0], requests[1]);
            if index > 0 {
                evidence = evidence.combine(RuntimeReadEvidence {
                    request_sha256: sha256_hex(&requests[0]),
                    response_sha256: sha256_hex(body),
                    bytes: body.len() * 2,
                });
            }
        }
        evidence
    });
    let client = TallyClient::new(TallyConfig {
        host: address.ip().to_string(),
        port: address.port(),
    })
    .unwrap();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.fetch_party_ledger_master_source(
            &identity,
            DateBoundaryProfile::ModeAgnostic,
            assertion,
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    let expected = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    (error, expected)
}

fn assert_retained(error: &anyhow::Error, expected: RuntimeReadEvidence) {
    let actual = &error
        .downcast_ref::<RuntimeReadFailure>()
        .expect("completed paired reads survive refusal")
        .evidence;
    assert_eq!(actual.bytes, expected.bytes);
    assert_eq!(actual.request_sha256, expected.request_sha256);
    assert_eq!(actual.response_sha256, expected.response_sha256);
}

#[tokio::test]
async fn party_master_parse_refusal_retains_its_actual_paired_wire_evidence() {
    // An unchanged captured Bills response is deliberately sent to the master
    // parser. This is a wrong-response fault, never positive qualification.
    let (error, expected) = refusal(&[BILLS], false).await;
    assert!(error.chain().any(|cause| matches!(
        cause.downcast_ref::<PartyLedgerMasterSourceValidationError>(),
        Some(PartyLedgerMasterSourceValidationError::MasterResponseInvalid { .. })
    )));
    assert_retained(&error, expected);
}

#[tokio::test]
async fn party_balance_identity_refusal_retains_master_and_balance_wire_evidence() {
    // The captured balance report omits company GUID. It must remain refused,
    // while both the earlier accepted master and rejected balance are audited.
    let (error, expected) = refusal(&[MASTER, IDENTITY_MISSING_BALANCE], false).await;
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<PartyLedgerMasterSourceValidationError>(),
            Some(PartyLedgerMasterSourceValidationError::BalanceCompanyIdentityUnverified)
        )),
        "unexpected typed refusal: {error:?}"
    );
    assert_retained(&error, expected);
}

#[tokio::test]
async fn party_group_identity_refusal_retains_all_completed_reports() {
    // A captured group response lacking computed company identity is refused
    // after the unchanged captured master and balance responses are accepted.
    let (error, expected) = refusal(&[MASTER, BALANCE, IDENTITY_MISSING_GROUP], false).await;
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<PartyLedgerMasterSourceValidationError>(),
            Some(PartyLedgerMasterSourceValidationError::GroupCompanyIdentityUnverified)
        )),
        "unexpected typed refusal: {error:?}"
    );
    assert_retained(&error, expected);
}

#[tokio::test]
async fn party_later_transport_refusal_preserves_completed_master_pair() {
    let (error, expected) = refusal(&[MASTER], true).await;
    assert!(
        error.chain().any(|cause| matches!(
            cause.downcast_ref::<bridge_tally_transport::TallyTransportError>(),
            Some(bridge_tally_transport::TallyTransportError::HttpStatus { status: 503 })
        )),
        "unexpected typed refusal: {error:?}"
    );
    assert_retained(&error, expected);
}
