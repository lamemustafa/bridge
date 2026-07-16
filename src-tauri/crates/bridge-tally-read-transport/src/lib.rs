//! Typed, loopback-only transport for sealed Tally read profiles.
//!
//! The generic XML POST capability is deliberately private to this crate. A
//! default caller can submit only a production-reviewed [`ReadOnlyProfile`].
//! A non-default feature adds one separately typed, qualification-only native
//! outstandings candidate without widening that profile enum.

#[cfg(feature = "bills-native-outstandings-probe-transport")]
use bridge_tally_protocol::bills_native_outstandings_probe::{
    NativeOutstandingsProbeProfileId, SealedNativeLedgerOutstandingsProbe,
};
use bridge_tally_protocol::{xml_read_profiles::ReadOnlyProfile, TallyTextEncoding};
#[cfg(feature = "bills-native-outstandings-probe-transport")]
use bridge_tally_transport::TransportPolicy;
use bridge_tally_transport::{
    TallyEndpointConfig, TallyHttpResponse, TallyHttpTransport, TallyTransportError,
};
#[cfg(feature = "bills-native-outstandings-probe-transport")]
use sha2::{Digest, Sha256};
#[cfg(feature = "bills-native-outstandings-probe-transport")]
use std::time::Duration;
use thiserror::Error;

#[cfg(feature = "bills-native-outstandings-probe-transport")]
pub const NATIVE_OUTSTANDINGS_REQUEST_MAX_BYTES: usize = 64 * 1024;
#[cfg(feature = "bills-native-outstandings-probe-transport")]
pub const NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES: usize = 1024 * 1024;
#[cfg(feature = "bills-native-outstandings-probe-transport")]
pub const NATIVE_OUTSTANDINGS_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(feature = "bills-native-outstandings-probe-transport")]
const CANDIDATE_V0_TEMPLATE_SHA256: &str =
    "bc3b87484adb9a10cc15f6c9042853bb1047278896bcf0f495b93e7e6b428526";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadLoopback {
    LocalhostAlias,
    Ipv4,
    Ipv6,
}

impl ReadLoopback {
    fn host(self) -> &'static str {
        match self {
            Self::LocalhostAlias => "localhost",
            Self::Ipv4 => "127.0.0.1",
            Self::Ipv6 => "::1",
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("Tally read-only transport failed ({code})")]
pub struct ReadOnlyTransportError {
    code: &'static str,
    http_status: Option<u16>,
}

impl ReadOnlyTransportError {
    pub fn safe_code(&self) -> &'static str {
        self.code
    }

    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }
}

impl From<TallyTransportError> for ReadOnlyTransportError {
    fn from(value: TallyTransportError) -> Self {
        let code = value.safe_code();
        let http_status = match value {
            TallyTransportError::HttpStatus { status } => Some(status),
            _ => None,
        };
        Self { code, http_status }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOnlyResponse {
    inner: TallyHttpResponse,
}

impl ReadOnlyResponse {
    pub fn text(&self) -> &str {
        self.inner.text()
    }

    pub fn encoding(&self) -> TallyTextEncoding {
        self.inner.encoding()
    }

    pub fn encoded_body(&self) -> &[u8] {
        self.inner.encoded_body()
    }

    pub fn encoded_bytes(&self) -> usize {
        self.inner.encoded_bytes()
    }

    pub fn http_status(&self) -> u16 {
        self.inner.http_status()
    }
}

/// Feature-gated transport for the unobserved native outstandings candidate.
/// It is intentionally separate from [`ReadOnlyProfile`] and accepts no raw
/// string or caller-authored XML.
#[cfg(feature = "bills-native-outstandings-probe-transport")]
#[derive(Clone)]
pub struct QualificationOnlyNativeOutstandingsTransport {
    inner: TallyHttpTransport,
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
impl QualificationOnlyNativeOutstandingsTransport {
    pub fn new(loopback: ReadLoopback, port: u16) -> Result<Self, ReadOnlyTransportError> {
        let inner = TallyHttpTransport::with_policy(
            TallyEndpointConfig {
                host: loopback.host().to_string(),
                port,
            },
            TransportPolicy {
                request_timeout: NATIVE_OUTSTANDINGS_REQUEST_TIMEOUT,
                status_response_max_bytes: NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES,
                xml_request_max_bytes: NATIVE_OUTSTANDINGS_REQUEST_MAX_BYTES,
                xml_response_max_bytes: NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES,
            },
        )?;
        Ok(Self { inner })
    }

    pub async fn send_candidate_v0(
        &self,
        candidate: &SealedNativeLedgerOutstandingsProbe,
    ) -> Result<QualificationOnlyNativeOutstandingsResponse, ReadOnlyTransportError> {
        validate_candidate_v0(candidate)?;
        let inner = self
            .inner
            .post_xml(candidate.rendered_xml().to_owned())
            .await?;
        Ok(QualificationOnlyNativeOutstandingsResponse { inner })
    }
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
#[derive(Clone, PartialEq, Eq)]
pub struct QualificationOnlyNativeOutstandingsResponse {
    inner: TallyHttpResponse,
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
impl std::fmt::Debug for QualificationOnlyNativeOutstandingsResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QualificationOnlyNativeOutstandingsResponse")
            .field("encoding", &self.encoding())
            .field("encoded_bytes", &self.encoded_body().len())
            .field("decoded_bytes", &self.text().len())
            .field("http_status", &self.http_status())
            .finish()
    }
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
impl QualificationOnlyNativeOutstandingsResponse {
    pub fn text(&self) -> &str {
        self.inner.text()
    }

    pub fn encoded_body(&self) -> &[u8] {
        self.inner.encoded_body()
    }

    pub fn encoding(&self) -> TallyTextEncoding {
        self.inner.encoding()
    }

    pub fn http_status(&self) -> u16 {
        self.inner.http_status()
    }
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
fn validate_candidate_v0(
    candidate: &SealedNativeLedgerOutstandingsProbe,
) -> Result<(), ReadOnlyTransportError> {
    if candidate.profile_id() != NativeOutstandingsProbeProfileId::LedgerOutstandingsCandidateV0 {
        return Err(error("native_outstandings_candidate_profile_changed"));
    }
    if candidate.template_sha256() != CANDIDATE_V0_TEMPLATE_SHA256 {
        return Err(error("native_outstandings_candidate_template_changed"));
    }
    let request = candidate.rendered_xml().as_bytes();
    if request.is_empty() || request.len() > NATIVE_OUTSTANDINGS_REQUEST_MAX_BYTES {
        return Err(error("native_outstandings_candidate_request_size_invalid"));
    }
    if sha256_hex(request) != candidate.request_sha256() {
        return Err(error("native_outstandings_candidate_request_changed"));
    }
    Ok(())
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
fn error(code: &'static str) -> ReadOnlyTransportError {
    ReadOnlyTransportError {
        code,
        http_status: None,
    }
}

#[cfg(feature = "bills-native-outstandings-probe-transport")]
fn sha256_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[derive(Clone)]
pub struct ReadOnlyTransport {
    inner: TallyHttpTransport,
}

impl ReadOnlyTransport {
    pub fn new(loopback: ReadLoopback, port: u16) -> Result<Self, ReadOnlyTransportError> {
        let inner = TallyHttpTransport::new(TallyEndpointConfig {
            host: loopback.host().to_string(),
            port,
        })?;
        Ok(Self { inner })
    }

    pub async fn send(
        &self,
        profile: ReadOnlyProfile<'_>,
    ) -> Result<ReadOnlyResponse, ReadOnlyTransportError> {
        let inner = self.inner.post_xml(profile.render()).await?;
        Ok(ReadOnlyResponse { inner })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tally_protocol_simulator::{Fixture, ScenarioPlan, Simulator};

    #[tokio::test]
    async fn sends_only_the_rendered_sealed_read_profile() {
        let profile = ReadOnlyProfile::CompanyListV1;
        let expected = profile.render();
        for attempt in 1..=5 {
            let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::EmptyExport)).unwrap();
            let transport =
                ReadOnlyTransport::new(ReadLoopback::Ipv4, simulator.address().port()).unwrap();
            match transport.send(profile).await {
                Ok(response) => {
                    assert_eq!(response.http_status(), 200);
                    let observed = simulator.finish().unwrap();
                    assert_eq!(observed.method, "POST");
                    assert_eq!(observed.path, "/");
                    assert!(observed.request_processed);
                    assert!(observed.bytes_received > expected.len());
                    return;
                }
                Err(error) if attempt < 5 && error.safe_code() == "request_failed" => {
                    // Windows endpoint-security software can abort a newly opened synthetic
                    // loopback flow before the request reaches the listener. Recreate only the
                    // test fixture; the production read transport remains single-attempt.
                    drop(simulator);
                }
                Err(error) => panic!("sealed read fixture failed: {error:?}"),
            }
        }
        unreachable!("bounded fixture attempts always return")
    }
}

#[cfg(all(test, feature = "bills-native-outstandings-probe-transport"))]
mod native_outstandings_tests {
    use super::*;
    use bridge_tally_protocol::bills_native_outstandings_probe::{
        NativeLedgerOutstandingsProbeScope, ValidatedProbeCompanyName, ValidatedProbeLedgerName,
        ValidatedProbeToDate,
    };
    use tally_protocol_simulator::{
        Fixture, ResponseFraming, ScenarioPlan, Simulator, WireEncoding,
    };

    fn candidate() -> SealedNativeLedgerOutstandingsProbe {
        NativeLedgerOutstandingsProbeScope::new(
            ValidatedProbeCompanyName::new("BRIDGE SYNTHETIC BOOK").unwrap(),
            ValidatedProbeLedgerName::new("BRIDGE PARTY").unwrap(),
            ValidatedProbeToDate::new("20260402").unwrap(),
        )
        .seal()
    }

    async fn deterministic_failure_fixture(
        plan: ScenarioPlan,
    ) -> (Simulator, ReadOnlyTransportError) {
        const MAX_HOST_ABORT_ATTEMPTS: usize = 3;
        for attempt in 1..=MAX_HOST_ABORT_ATTEMPTS {
            let simulator = Simulator::spawn(plan.clone()).unwrap();
            let transport = QualificationOnlyNativeOutstandingsTransport::new(
                ReadLoopback::Ipv4,
                simulator.address().port(),
            )
            .unwrap();
            let failure = transport.send_candidate_v0(&candidate()).await.unwrap_err();
            if attempt < MAX_HOST_ABORT_ATTEMPTS && failure.safe_code() == "request_failed" {
                // Windows endpoint-security software can abort a newly opened
                // loopback flow before a response exists. Recreate only this
                // synthetic read-only fixture; production retry remains off.
                drop(simulator);
                continue;
            }
            return (simulator, failure);
        }
        unreachable!("bounded synthetic attempts always return")
    }

    #[tokio::test]
    async fn dispatches_exact_frozen_candidate_once_and_retains_exact_response() {
        let response_xml = "<ENVELOPE><BODY><DATA>opaque-π</DATA></BODY></ENVELOPE>";
        let plan = ScenarioPlan::new(Fixture::SyntheticXml(response_xml.to_string()))
            .with_encoding(WireEncoding::Utf16Le);
        let expected_response = plan.response_bytes();
        let simulator = Simulator::spawn(plan).unwrap();
        let transport = QualificationOnlyNativeOutstandingsTransport::new(
            ReadLoopback::Ipv4,
            simulator.address().port(),
        )
        .unwrap();
        let candidate = candidate();
        let response = transport.send_candidate_v0(&candidate).await.unwrap();
        assert_eq!(response.http_status(), 200);
        assert_eq!(response.text(), response_xml);
        assert_eq!(response.encoded_body(), expected_response);

        let observed = simulator.finish().unwrap();
        assert_eq!(observed.method, "POST");
        assert_eq!(observed.path, "/");
        assert_eq!(observed.request_body_bytes, candidate.rendered_xml().len());
        assert_eq!(observed.request_body_sha256, candidate.request_sha256());
        assert!(observed.request_processed);
    }

    #[tokio::test]
    async fn qualification_caps_reject_oversized_response_without_retry() {
        let simulator = Simulator::spawn(ScenarioPlan::new(Fixture::Oversized {
            minimum_bytes: NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES + 1,
        }))
        .unwrap();
        let transport = QualificationOnlyNativeOutstandingsTransport::new(
            ReadLoopback::Ipv4,
            simulator.address().port(),
        )
        .unwrap();
        let error = transport.send_candidate_v0(&candidate()).await.unwrap_err();
        assert_eq!(error.safe_code(), "response_size_limit_exceeded");
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.request_body_sha256, candidate().request_sha256());
    }

    #[tokio::test]
    async fn qualification_transport_does_not_follow_redirects_or_retry_failures() {
        for (plan, expected_status) in [
            (
                ScenarioPlan::new(Fixture::SyntheticXml("<REDIRECT/>".to_string()))
                    .with_http_status(302)
                    .with_redirect_location("/must-not-be-followed"),
                Some(302),
            ),
            (
                ScenarioPlan::new(Fixture::SyntheticXml("<FAILURE/>".to_string()))
                    .with_http_status(500),
                Some(500),
            ),
            (
                ScenarioPlan::new(Fixture::SyntheticXml("<TRUNCATED/>".to_string()))
                    .with_framing(ResponseFraming::DeclaredContentLength { bytes: 4096 }),
                None,
            ),
        ] {
            let (simulator, failure) = deterministic_failure_fixture(plan).await;
            assert_eq!(failure.http_status(), expected_status);
            let observed = simulator.finish().unwrap();
            assert_eq!(observed.method, "POST");
            assert_eq!(observed.path, "/");
            assert_eq!(observed.request_body_sha256, candidate().request_sha256());
        }
    }

    #[test]
    fn qualification_policy_is_fixed_and_candidate_is_bounded() {
        let candidate = candidate();
        assert!(candidate.rendered_xml().len() <= NATIVE_OUTSTANDINGS_REQUEST_MAX_BYTES);
        assert_eq!(NATIVE_OUTSTANDINGS_RESPONSE_MAX_BYTES, 1024 * 1024);
        assert_eq!(NATIVE_OUTSTANDINGS_REQUEST_TIMEOUT, Duration::from_secs(20));
        validate_candidate_v0(&candidate).unwrap();
    }
}
