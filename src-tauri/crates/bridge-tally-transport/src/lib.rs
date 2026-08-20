//! Portable, loopback-only and bounded HTTP transport for Tally.
//!
//! This crate establishes HTTP delivery and decoding facts only. A successful
//! return is not evidence that Tally accepted an import or completed an export;
//! callers must still validate the application envelope.

use std::{net::IpAddr, time::Duration};

#[cfg(feature = "voucher-scan")]
use bridge_tally_protocol::outstandings::VoucherOutstandingsRequestXml;
use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, encode_tally_xml_request_utf16le,
    validate_tally_xml_response_content_type, ExpectedTallyTextEncoding, TallyTextDecodeError,
    TallyTextEncoding, TallyTextStreamDecoder,
};
use reqwest::{
    header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE},
    redirect::Policy,
    Client, ClientBuilder, Response, Url,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const STATUS_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
/// Maximum UTF-8 byte length of an XML source string accepted before UTF-16LE
/// wire encoding. This preserves the pre-U1 request-cap contract; the encoded
/// HTTP entity is mathematically bounded to at most twice this size plus its BOM.
pub const XML_REQUEST_MAX_BYTES: usize = 32 * 1024 * 1024;
pub const XML_RESPONSE_MAX_BYTES: usize = 32 * 1024 * 1024;
/// Sealed exception for the voucher-scan wildcard outstandings request only
/// -- see `post_outstandings_xml_decoded`. Gated with the request type that
/// is the only thing admitted through it: with `voucher-scan` off, nothing
/// can construct that request, so a raised cap nothing can reach would be a
/// widened attack surface for no reason.
#[cfg(feature = "voucher-scan")]
pub const OUTSTANDINGS_XML_RESPONSE_MAX_BYTES: usize = 40 * 1024 * 1024;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
// `/status` is a bodyless ASCII-only liveness probe. Keep its pre-U1 UTF-8
// contract so build-specific charset mirroring cannot create a false outage.
const TALLY_STATUS_CONTENT_TYPE: &str = "text/xml";
const TALLY_STATUS_RESPONSE_ENCODING: ExpectedTallyTextEncoding = ExpectedTallyTextEncoding::Utf8;
const TALLY_XML_POST_CONTENT_TYPE: &str = "text/xml; charset=utf-16";
const TALLY_XML_POST_RESPONSE_ENCODING: ExpectedTallyTextEncoding =
    ExpectedTallyTextEncoding::Utf16Le;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TallyEndpointConfig {
    pub host: String,
    pub port: u16,
}

impl Default for TallyEndpointConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_owned(),
            port: 9000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportPolicy {
    pub request_timeout: Duration,
    pub status_response_max_bytes: usize,
    /// UTF-8 byte limit for the XML source string before wire encoding.
    pub xml_request_max_bytes: usize,
    pub xml_response_max_bytes: usize,
}

impl Default for TransportPolicy {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            status_response_max_bytes: STATUS_RESPONSE_MAX_BYTES,
            xml_request_max_bytes: XML_REQUEST_MAX_BYTES,
            xml_response_max_bytes: XML_RESPONSE_MAX_BYTES,
        }
    }
}

impl TransportPolicy {
    fn validate(self) -> Result<Self, TallyTransportError> {
        if self.request_timeout.is_zero() || self.request_timeout > MAX_REQUEST_TIMEOUT {
            return Err(TallyTransportError::PolicyInvalid {
                code: "request_timeout_out_of_range",
            });
        }
        for (value, maximum, code) in [
            (
                self.status_response_max_bytes,
                STATUS_RESPONSE_MAX_BYTES,
                "status_response_limit_out_of_range",
            ),
            (
                self.xml_request_max_bytes,
                XML_REQUEST_MAX_BYTES,
                "xml_request_limit_out_of_range",
            ),
            (
                self.xml_response_max_bytes,
                XML_RESPONSE_MAX_BYTES,
                "xml_response_limit_out_of_range",
            ),
        ] {
            if value == 0 || value > maximum {
                return Err(TallyTransportError::PolicyInvalid { code });
            }
        }
        Ok(self)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TallyHttpResponse {
    text: String,
    encoding: TallyTextEncoding,
    encoded_body: Vec<u8>,
    encoded_bytes: usize,
    request_body_sha256: Option<String>,
    http_status: u16,
}

impl std::fmt::Debug for TallyHttpResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TallyHttpResponse")
            .field("encoding", &self.encoding)
            .field("encoded_bytes", &self.encoded_bytes)
            .field("decoded_bytes", &self.text.len())
            .field("http_status", &self.http_status)
            .finish()
    }
}

impl TallyHttpResponse {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn encoding(&self) -> TallyTextEncoding {
        self.encoding
    }

    /// Exact HTTP entity bytes received before Tally text decoding.
    pub fn encoded_body(&self) -> &[u8] {
        &self.encoded_body
    }

    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    /// SHA-256 of the exact HTTP entity bytes dispatched for a POST request.
    /// Status GET responses have no request entity and return `None`.
    pub fn request_body_sha256(&self) -> Option<&str> {
        self.request_body_sha256.as_deref()
    }

    pub fn http_status(&self) -> u16 {
        self.http_status
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TallyDecodedHttpResponse {
    text: String,
    encoding: TallyTextEncoding,
    encoded_bytes: usize,
    decoded_bytes: usize,
    encoded_sha256: String,
    decoded_sha256: String,
    request_body_sha256: Option<String>,
    http_status: u16,
}

impl std::fmt::Debug for TallyDecodedHttpResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TallyDecodedHttpResponse")
            .field("encoding", &self.encoding)
            .field("encoded_bytes", &self.encoded_bytes)
            .field("decoded_bytes", &self.decoded_bytes)
            .field("http_status", &self.http_status)
            .finish()
    }
}

impl TallyDecodedHttpResponse {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn encoding(&self) -> TallyTextEncoding {
        self.encoding
    }

    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    pub fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }

    pub fn encoded_sha256(&self) -> &str {
        &self.encoded_sha256
    }

    pub fn decoded_sha256(&self) -> &str {
        &self.decoded_sha256
    }

    /// SHA-256 of the exact HTTP entity bytes dispatched for a POST request.
    /// Status GET responses have no request entity and return `None`.
    pub fn request_body_sha256(&self) -> Option<&str> {
        self.request_body_sha256.as_deref()
    }

    pub fn http_status(&self) -> u16 {
        self.http_status
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TallyTransportError {
    #[error("Tally endpoint validation failed ({code})")]
    EndpointInvalid { code: &'static str },
    #[error("Tally transport policy is invalid ({code})")]
    PolicyInvalid { code: &'static str },
    #[error("Tally HTTP client could not be initialized")]
    ClientInitializationFailed,
    #[error("Tally request exceeded the {limit}-byte limit")]
    RequestTooLarge { limit: usize },
    #[error("Tally endpoint did not accept the connection")]
    ConnectionFailed,
    #[error("Tally request exceeded its deadline")]
    RequestTimedOut,
    #[error("Tally request failed before a response was available")]
    RequestFailed,
    #[error("Tally returned HTTP status {status}")]
    HttpStatus { status: u16 },
    #[error("Tally response exceeded the {limit}-byte limit")]
    ResponseTooLarge {
        limit: usize,
        declared_by_peer: bool,
    },
    #[error("Tally response ended before its declared HTTP body was complete")]
    ResponseTruncated,
    #[error("Tally response body could not be read")]
    ResponseReadFailed,
    #[error("Tally response used an unsupported content encoding")]
    UnsupportedContentEncoding,
    #[error("Tally response encoding was invalid ({code})")]
    InvalidEncoding { code: &'static str },
}

impl TallyTransportError {
    pub fn safe_code(&self) -> &'static str {
        match self {
            Self::EndpointInvalid { .. } => "endpoint_invalid",
            Self::PolicyInvalid { .. } => "transport_policy_invalid",
            Self::ClientInitializationFailed => "http_client_initialization_failed",
            Self::RequestTooLarge { .. } => "request_size_limit_exceeded",
            Self::ConnectionFailed => "endpoint_unreachable",
            Self::RequestTimedOut => "request_deadline_exceeded",
            Self::RequestFailed => "request_failed",
            Self::HttpStatus { .. } => "http_status_failure",
            Self::ResponseTooLarge { .. } => "response_size_limit_exceeded",
            Self::ResponseTruncated => "response_truncated",
            Self::ResponseReadFailed => "response_read_failed",
            Self::UnsupportedContentEncoding => "response_content_encoding_unsupported",
            Self::InvalidEncoding { .. } => "response_encoding_invalid",
        }
    }
}

struct PreparedTallyXmlRequest {
    body: Vec<u8>,
    body_sha256: String,
}

fn prepare_tally_xml_request(
    xml: &str,
    xml_source_max_bytes: usize,
) -> Result<PreparedTallyXmlRequest, TallyTransportError> {
    prepare_tally_xml_request_with_encoder(
        xml,
        xml_source_max_bytes,
        encode_tally_xml_request_utf16le,
    )
}

fn prepare_tally_xml_request_with_encoder(
    xml: &str,
    xml_source_max_bytes: usize,
    encoder: impl FnOnce(&str) -> Vec<u8>,
) -> Result<PreparedTallyXmlRequest, TallyTransportError> {
    if xml.len() > xml_source_max_bytes {
        return Err(TallyTransportError::RequestTooLarge {
            limit: xml_source_max_bytes,
        });
    }
    let encoded_length_upper_bound = xml
        .len()
        .checked_mul(2)
        .and_then(|length| length.checked_add(2))
        .ok_or(TallyTransportError::RequestTooLarge {
            limit: xml_source_max_bytes,
        })?;
    let body = encoder(xml);
    debug_assert!(body.len() <= encoded_length_upper_bound);
    let body_sha256 = hex_digest(Sha256::digest(&body));
    Ok(PreparedTallyXmlRequest { body, body_sha256 })
}

#[derive(Clone)]
pub struct TallyHttpTransport {
    config: TallyEndpointConfig,
    policy: TransportPolicy,
    client: Client,
}

impl TallyHttpTransport {
    pub fn new(config: TallyEndpointConfig) -> Result<Self, TallyTransportError> {
        Self::with_builder(config, TransportPolicy::default(), Client::builder())
    }

    pub fn with_policy(
        config: TallyEndpointConfig,
        policy: TransportPolicy,
    ) -> Result<Self, TallyTransportError> {
        Self::with_builder(config, policy, Client::builder())
    }

    /// Test seam that still applies all production proxy, redirect, and timeout
    /// controls after the supplied builder customizations.
    #[doc(hidden)]
    pub fn with_builder(
        config: TallyEndpointConfig,
        policy: TransportPolicy,
        builder: ClientBuilder,
    ) -> Result<Self, TallyTransportError> {
        endpoint_url(&config, "/")?;
        let policy = policy.validate()?;
        let client = builder
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(policy.request_timeout)
            .redirect(Policy::none())
            .build()
            .map_err(|_| TallyTransportError::ClientInitializationFailed)?;
        Ok(Self {
            config,
            policy,
            client,
        })
    }

    pub fn canonical_origin(&self) -> Result<String, TallyTransportError> {
        canonical_loopback_origin(&self.config)
    }

    pub async fn get_status(&self) -> Result<TallyHttpResponse, TallyTransportError> {
        let url = endpoint_url(&self.config, "/status")?;
        let response = self
            .client
            .get(url)
            .header(CONTENT_TYPE, TALLY_STATUS_CONTENT_TYPE)
            .send()
            .await
            .map_err(classify_request_error)?;
        read_response(
            response,
            self.policy.status_response_max_bytes,
            TALLY_STATUS_RESPONSE_ENCODING,
        )
        .await
    }

    pub async fn get_status_decoded(
        &self,
    ) -> Result<TallyDecodedHttpResponse, TallyTransportError> {
        let url = endpoint_url(&self.config, "/status")?;
        let response = self
            .client
            .get(url)
            .header(CONTENT_TYPE, TALLY_STATUS_CONTENT_TYPE)
            .send()
            .await
            .map_err(classify_request_error)?;
        read_decoded_response(
            response,
            self.policy.status_response_max_bytes,
            TALLY_STATUS_RESPONSE_ENCODING,
        )
        .await
    }

    pub async fn post_xml(&self, xml: String) -> Result<TallyHttpResponse, TallyTransportError> {
        let PreparedTallyXmlRequest { body, body_sha256 } =
            prepare_tally_xml_request(&xml, self.policy.xml_request_max_bytes)?;
        let content_length = body.len();
        let url = endpoint_url(&self.config, "/")?;
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, TALLY_XML_POST_CONTENT_TYPE)
            .header(CONTENT_LENGTH, content_length)
            .body(body)
            .send()
            .await
            .map_err(classify_request_error)?;
        let mut response = read_response(
            response,
            self.policy.xml_response_max_bytes,
            TALLY_XML_POST_RESPONSE_ENCODING,
        )
        .await?;
        response.request_body_sha256 = Some(body_sha256);
        Ok(response)
    }

    pub async fn post_xml_decoded(
        &self,
        xml: String,
    ) -> Result<TallyDecodedHttpResponse, TallyTransportError> {
        self.post_xml_decoded_with_response_limit(xml, self.policy.xml_response_max_bytes)
            .await
    }

    /// Closed exception for the live-verified wildcard outstandings profile.
    /// The general policy remains capped at 32 MiB and the same client keeps
    /// the immutable 20-second request deadline.
    #[cfg(feature = "voucher-scan")]
    pub async fn post_outstandings_xml_decoded(
        &self,
        request: VoucherOutstandingsRequestXml,
    ) -> Result<TallyDecodedHttpResponse, TallyTransportError> {
        self.post_xml_decoded_with_response_limit(
            request.into_xml(),
            OUTSTANDINGS_XML_RESPONSE_MAX_BYTES,
        )
        .await
    }

    async fn post_xml_decoded_with_response_limit(
        &self,
        xml: String,
        response_max_bytes: usize,
    ) -> Result<TallyDecodedHttpResponse, TallyTransportError> {
        let PreparedTallyXmlRequest { body, body_sha256 } =
            prepare_tally_xml_request(&xml, self.policy.xml_request_max_bytes)?;
        let content_length = body.len();
        let url = endpoint_url(&self.config, "/")?;
        let response = self
            .client
            .post(url)
            .header(CONTENT_TYPE, TALLY_XML_POST_CONTENT_TYPE)
            .header(CONTENT_LENGTH, content_length)
            .body(body)
            .send()
            .await
            .map_err(classify_request_error)?;
        let mut response = read_decoded_response(
            response,
            response_max_bytes,
            TALLY_XML_POST_RESPONSE_ENCODING,
        )
        .await?;
        response.request_body_sha256 = Some(body_sha256);
        Ok(response)
    }
}

pub fn canonical_loopback_origin(
    config: &TallyEndpointConfig,
) -> Result<String, TallyTransportError> {
    Ok(endpoint_url(config, "/")?.origin().ascii_serialization())
}

fn endpoint_url(config: &TallyEndpointConfig, path: &str) -> Result<Url, TallyTransportError> {
    let host = config.host.trim();
    if host.is_empty()
        || host.len() > 253
        || host.chars().any(char::is_control)
        || host.contains(['/', '\\', '?', '#', '@'])
    {
        return Err(TallyTransportError::EndpointInvalid {
            code: "host_syntax_invalid",
        });
    }
    if config.port == 0 {
        return Err(TallyTransportError::EndpointInvalid {
            code: "port_out_of_range",
        });
    }

    let mut url =
        Url::parse("http://localhost").map_err(|_| TallyTransportError::EndpointInvalid {
            code: "base_url_invalid",
        })?;
    if let Ok(ip_address) = host.parse::<IpAddr>() {
        if !ip_address.is_loopback() {
            return Err(TallyTransportError::EndpointInvalid {
                code: "non_loopback_forbidden",
            });
        }
        url.set_ip_host(ip_address)
            .map_err(|_| TallyTransportError::EndpointInvalid {
                code: "ip_address_invalid",
            })?;
    } else {
        if !host.eq_ignore_ascii_case("localhost") {
            return Err(TallyTransportError::EndpointInvalid {
                code: "non_loopback_forbidden",
            });
        }
        let loopback = "127.0.0.1"
            .parse::<IpAddr>()
            .expect("static loopback address is valid");
        url.set_ip_host(loopback)
            .map_err(|_| TallyTransportError::EndpointInvalid {
                code: "ip_address_invalid",
            })?;
    }
    url.set_port(Some(config.port))
        .map_err(|_| TallyTransportError::EndpointInvalid {
            code: "port_out_of_range",
        })?;
    url.set_path(path);
    Ok(url)
}

async fn read_response(
    mut response: Response,
    max_bytes: usize,
    expected_encoding: ExpectedTallyTextEncoding,
) -> Result<TallyHttpResponse, TallyTransportError> {
    let head = validate_response_head(&response, max_bytes, expected_encoding)?;
    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(max_bytes);
    let mut bytes = Vec::with_capacity(initial_capacity);
    loop {
        let chunk = response.chunk().await.map_err(classify_body_error)?;
        let Some(chunk) = chunk else { break };
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(TallyTransportError::ResponseTooLarge {
                limit: max_bytes,
                declared_by_peer: false,
            });
        }
        bytes.extend_from_slice(&chunk);
    }

    let encoded_bytes = bytes.len();
    let decoded = decode_tally_xml_response_bytes_limited(
        &bytes,
        &head.content_type,
        expected_encoding,
        max_bytes,
    )
    .map_err(|error| map_stream_decode_error(error, max_bytes))?;
    if decoded.text.len() > max_bytes {
        return Err(TallyTransportError::ResponseTooLarge {
            limit: max_bytes,
            declared_by_peer: false,
        });
    }
    Ok(TallyHttpResponse {
        text: decoded.text,
        encoding: decoded.encoding,
        encoded_body: bytes,
        encoded_bytes,
        request_body_sha256: None,
        http_status: head.http_status,
    })
}

struct ValidatedResponseHead {
    http_status: u16,
    content_type: String,
}

fn validate_response_head(
    response: &Response,
    max_bytes: usize,
    expected_encoding: ExpectedTallyTextEncoding,
) -> Result<ValidatedResponseHead, TallyTransportError> {
    let status = response.status();
    if !status.is_success() {
        return Err(TallyTransportError::HttpStatus {
            status: status.as_u16(),
        });
    }
    let mut content_encodings = response.headers().get_all(CONTENT_ENCODING).iter();
    if let Some(value) = content_encodings.next() {
        let exactly_identity = value
            .to_str()
            .map(|encoding| encoding.trim().eq_ignore_ascii_case("identity"))
            .unwrap_or(false);
        if !exactly_identity || content_encodings.next().is_some() {
            return Err(TallyTransportError::UnsupportedContentEncoding);
        }
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(TallyTransportError::ResponseTooLarge {
            limit: max_bytes,
            declared_by_peer: true,
        });
    }
    let mut content_types = response.headers().get_all(CONTENT_TYPE).iter();
    let content_type = content_types
        .next()
        .and_then(|value| value.to_str().ok())
        .filter(|_| content_types.next().is_none())
        .ok_or(TallyTransportError::InvalidEncoding {
            code: "response_content_type_unsupported",
        })?;
    validate_tally_xml_response_content_type(content_type, expected_encoding)
        .map_err(map_decode_error)?;
    Ok(ValidatedResponseHead {
        http_status: status.as_u16(),
        content_type: content_type.to_owned(),
    })
}

async fn read_decoded_response(
    mut response: Response,
    max_bytes: usize,
    expected_encoding: ExpectedTallyTextEncoding,
) -> Result<TallyDecodedHttpResponse, TallyTransportError> {
    let head = validate_response_head(&response, max_bytes, expected_encoding)?;
    let mut decoder =
        TallyTextStreamDecoder::new_with_expected_encoding(max_bytes, expected_encoding);
    let mut encoded_bytes = 0_usize;
    let mut encoded_sha256 = Sha256::new();
    loop {
        let chunk = response.chunk().await.map_err(classify_body_error)?;
        let Some(chunk) = chunk else { break };
        if encoded_bytes.saturating_add(chunk.len()) > max_bytes {
            return Err(TallyTransportError::ResponseTooLarge {
                limit: max_bytes,
                declared_by_peer: false,
            });
        }
        encoded_bytes += chunk.len();
        encoded_sha256.update(&chunk);
        decoder
            .push_chunk(&chunk)
            .map_err(|error| map_stream_decode_error(error, max_bytes))?;
    }
    let decoded = decoder
        .finish()
        .map_err(|error| map_stream_decode_error(error, max_bytes))?;
    Ok(TallyDecodedHttpResponse {
        text: decoded.text,
        encoding: decoded.encoding,
        encoded_bytes,
        decoded_bytes: decoded.decoded_bytes,
        encoded_sha256: hex_digest(encoded_sha256.finalize()),
        decoded_sha256: decoded.decoded_sha256,
        request_body_sha256: None,
        http_status: head.http_status,
    })
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn classify_request_error(error: reqwest::Error) -> TallyTransportError {
    if error.is_timeout() {
        TallyTransportError::RequestTimedOut
    } else if error.is_connect() {
        TallyTransportError::ConnectionFailed
    } else {
        TallyTransportError::RequestFailed
    }
}

fn classify_body_error(error: reqwest::Error) -> TallyTransportError {
    if error.is_timeout() {
        TallyTransportError::RequestTimedOut
    } else if error.is_body() || error.is_decode() {
        TallyTransportError::ResponseTruncated
    } else {
        TallyTransportError::ResponseReadFailed
    }
}

fn map_decode_error(error: TallyTextDecodeError) -> TallyTransportError {
    let code = match error {
        TallyTextDecodeError::TooLarge => "decoded_body_too_large",
        TallyTextDecodeError::InvalidUtf8 => "invalid_utf8",
        TallyTextDecodeError::InvalidUtf16Le => "invalid_utf16le",
        TallyTextDecodeError::InvalidUtf16Be => "invalid_utf16be",
        TallyTextDecodeError::UnsupportedContentType => "response_content_type_unsupported",
        TallyTextDecodeError::DeclaredEncodingMismatch => "declared_encoding_mismatch",
        TallyTextDecodeError::ObservedEncodingMismatch => "observed_encoding_mismatch",
    };
    TallyTransportError::InvalidEncoding { code }
}

fn map_stream_decode_error(error: TallyTextDecodeError, max_bytes: usize) -> TallyTransportError {
    match error {
        TallyTextDecodeError::TooLarge => TallyTransportError::ResponseTooLarge {
            limit: max_bytes,
            declared_by_peer: false,
        },
        error => map_decode_error(error),
    }
}

#[cfg(test)]
mod unit_tests {
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
}
