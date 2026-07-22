use super::{ConnectionStatus, TallyClient, TallyCompany, TallyConfig, TallyLedger};
use super::{TallyProbeResult, TallyVoucher};
use crate::tally::connection::{
    canonical_loopback_origin, LedgerCanaryReadbackXml, SelectedReadObservation,
};
use crate::tally::connector::SealedReadRequest;
use bridge_tally_protocol::xml_read_profiles::{
    ValidatedCanaryLedgerName, ValidatedCompanyName, ValidatedIdentityQuerySha256,
};
use bridge_tally_runtime::{
    BodyBytesObservation, EndpointCircuitState, EndpointIdentity, EndpointRuntimeSnapshot,
    PortableReadRuntime, ReadAttempt, ReadExecutionError, ReadFailureClass, ReadOperation,
    ReadRetryPolicy, TELEMETRY_PREVIEW_SCHEMA,
};
use bridge_tally_transport::TallyTransportError;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

const MAX_ENDPOINT_SESSIONS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointKey(String);

impl EndpointKey {
    pub fn from_config(config: &TallyConfig) -> anyhow::Result<Self> {
        Ok(Self(canonical_loopback_origin(config)?))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TallySessionSnapshot {
    pub session_id: String,
    pub canonical_endpoint: String,
    pub issued_requests: u64,
    pub active_requests: usize,
    pub active_request_ids: Vec<String>,
    pub consecutive_failures: u32,
    pub circuit_state: CircuitState,
    pub circuit_retry_after_unix_ms: Option<i64>,
    pub last_success_unix_ms: Option<i64>,
    pub last_failure_unix_ms: Option<i64>,
    pub cached_capability_observed_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TallyTelemetryPreviewExport {
    pub schema: &'static str,
    pub payload_sha256: String,
    pub preview_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Default)]
struct SessionHealth {
    last_success_unix_ms: Option<i64>,
    last_failure_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealthOutcome {
    TransportSuccess,
    TransportFailure,
    ApplicationRejected,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TallyRuntimeControlError {
    #[error("read_request_cancelled")]
    Cancelled,
    #[error("endpoint_queue_deadline_exceeded")]
    QueueDeadline,
    #[error("endpoint_circuit_cooldown")]
    CircuitCooldown,
    #[error("endpoint_half_open_probe_in_flight")]
    HalfOpenProbeInFlight,
    #[error("endpoint_session_capacity_reached")]
    EndpointSessionCapacity,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TallyRuntimeReadError {
    #[error("application_response_rejected")]
    ApplicationResponseRejected,
}

#[derive(Clone)]
struct CachedProbe {
    review_id: String,
    observed_at_unix_ms: i64,
    freshness_origin_unix_ms: i64,
    result: TallyProbeResult,
    reserved: bool,
}

struct TallySession {
    session_id: String,
    endpoint: EndpointKey,
    client: TallyClient,
    sequence: AtomicU64,
    active_requests: Mutex<HashMap<String, CancellationToken>>,
    health: Mutex<SessionHealth>,
    cached_probe: RwLock<Option<CachedProbe>>,
    active_ordinary_reads: AtomicU64,
}

impl TallySession {
    fn new(endpoint: EndpointKey, config: TallyConfig) -> anyhow::Result<Self> {
        Ok(Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            endpoint,
            client: TallyClient::new(config)?,
            sequence: AtomicU64::new(0),
            active_requests: Mutex::new(HashMap::new()),
            health: Mutex::new(SessionHealth::default()),
            cached_probe: RwLock::new(None),
            active_ordinary_reads: AtomicU64::new(0),
        })
    }

    #[cfg(test)]
    fn with_transport_policy(
        endpoint: EndpointKey,
        config: TallyConfig,
        policy: bridge_tally_transport::TransportPolicy,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            session_id: uuid::Uuid::new_v4().to_string(),
            endpoint,
            client: TallyClient::with_transport_policy(config, policy)?,
            sequence: AtomicU64::new(0),
            active_requests: Mutex::new(HashMap::new()),
            health: Mutex::new(SessionHealth::default()),
            cached_probe: RwLock::new(None),
            active_ordinary_reads: AtomicU64::new(0),
        })
    }

    fn begin_request(self: &Arc<Self>) -> anyhow::Result<RuntimeRequest> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let request_id = format!("{}:{sequence}", self.session_id);
        let cancellation = CancellationToken::new();
        self.active_requests
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally cancellation registry is unavailable"))?
            .insert(request_id.clone(), cancellation.clone());
        Ok(RuntimeRequest {
            session: Arc::clone(self),
            request_id,
            cancellation,
        })
    }

    fn record_result(&self, outcome: HealthOutcome) {
        let Ok(mut health) = self.health.lock() else {
            return;
        };
        let now = chrono::Utc::now().timestamp_millis();
        match outcome {
            HealthOutcome::TransportSuccess => {
                health.last_success_unix_ms = Some(now);
            }
            HealthOutcome::TransportFailure => {
                health.last_failure_unix_ms = Some(now);
            }
            // A rejected/malformed application response proves a responder was
            // reached but must not erase earlier transport failures. Operator
            // cancellation says nothing about endpoint health.
            HealthOutcome::ApplicationRejected | HealthOutcome::Cancelled => {}
        }
    }

    fn cancel(&self, request_id: &str) -> anyhow::Result<bool> {
        let requests = self
            .active_requests
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally cancellation registry is unavailable"))?;
        let Some(token) = requests.get(request_id) else {
            return Ok(false);
        };
        token.cancel();
        Ok(true)
    }

    fn snapshot(
        &self,
        control: Option<EndpointRuntimeSnapshot>,
    ) -> anyhow::Result<TallySessionSnapshot> {
        let (last_success_unix_ms, fallback_last_failure_unix_ms) = {
            let health = self
                .health
                .lock()
                .map_err(|_| anyhow::anyhow!("Tally session health is unavailable"))?;
            (health.last_success_unix_ms, health.last_failure_unix_ms)
        };
        let mut active_request_ids = self
            .active_requests
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally cancellation registry is unavailable"))?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        active_request_ids.sort();
        let cached_capability_observed_at_unix_ms = self
            .cached_probe
            .read()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?
            .as_ref()
            .map(|probe| probe.observed_at_unix_ms);
        let (
            consecutive_failures,
            circuit_state,
            circuit_retry_after_unix_ms,
            last_failure_unix_ms,
        ) = match control {
            Some(control) => (
                control.consecutive_failures,
                match control.circuit_state {
                    EndpointCircuitState::Closed => CircuitState::Closed,
                    EndpointCircuitState::Open => CircuitState::Open,
                    EndpointCircuitState::HalfOpen => CircuitState::HalfOpen,
                },
                control.circuit_retry_after_unix_ms,
                control.last_failure_unix_ms,
            ),
            None => (0, CircuitState::Closed, None, fallback_last_failure_unix_ms),
        };
        Ok(TallySessionSnapshot {
            session_id: self.session_id.clone(),
            canonical_endpoint: self.endpoint.as_str().to_string(),
            issued_requests: self.sequence.load(Ordering::Relaxed),
            active_requests: active_request_ids.len(),
            active_request_ids,
            consecutive_failures,
            circuit_state,
            circuit_retry_after_unix_ms,
            last_success_unix_ms,
            last_failure_unix_ms,
            cached_capability_observed_at_unix_ms,
        })
    }
}

struct RuntimeRequest {
    session: Arc<TallySession>,
    request_id: String,
    cancellation: CancellationToken,
}

struct OrdinaryReadLease {
    session: Arc<TallySession>,
}

impl Drop for OrdinaryReadLease {
    fn drop(&mut self) {
        self.session
            .active_ordinary_reads
            .fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for RuntimeRequest {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.session.active_requests.lock() {
            requests.remove(&self.request_id);
        }
    }
}

struct SessionSlot {
    session: Arc<TallySession>,
    last_used: Instant,
}

#[derive(Clone)]
pub struct TallyRuntime {
    sessions: Arc<Mutex<HashMap<EndpointKey, SessionSlot>>>,
    runtime_identity: Arc<()>,
    control: PortableReadRuntime,
    #[cfg(test)]
    transport_policy: Option<bridge_tally_transport::TransportPolicy>,
}

/// Opaque, owner-bound authority over one fresh reviewed probe.
///
/// The lease keeps the endpoint session alive and releases the reservation on
/// every unwind/abort/early-return path unless it was explicitly consumed or
/// atomically replaced. Drop never touches a different or newer review.
pub struct CachedProbeReservation {
    session: Arc<TallySession>,
    runtime_identity: Arc<()>,
    review_id: String,
    observed_at_unix_ms: i64,
    result: TallyProbeResult,
    armed: bool,
}

impl CachedProbeReservation {
    pub fn observed_at_unix_ms(&self) -> i64 {
        self.observed_at_unix_ms
    }

    pub fn result(&self) -> &TallyProbeResult {
        &self.result
    }

    pub fn review_id(&self) -> &str {
        &self.review_id
    }

    fn authorize(&self, runtime: &TallyRuntime, config: &TallyConfig) -> anyhow::Result<()> {
        if !self.armed
            || !Arc::ptr_eq(&self.runtime_identity, &runtime.runtime_identity)
            || self.session.endpoint != EndpointKey::from_config(config)?
        {
            anyhow::bail!("Tally reviewed setup operation ownership changed");
        }
        if self
            .session
            .cached_probe
            .read()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?
            .as_ref()
            .is_some_and(|probe| probe.reserved && probe.review_id == self.review_id)
        {
            Ok(())
        } else {
            anyhow::bail!("Tally reviewed setup operation ownership changed")
        }
    }

    pub fn release(&mut self) -> anyhow::Result<bool> {
        self.finish(false)
    }

    pub fn consume(&mut self) -> anyhow::Result<bool> {
        self.finish(true)
    }

    pub fn replace(
        &mut self,
        replacement_review_id: String,
        observed_at_unix_ms: i64,
        result: TallyProbeResult,
    ) -> anyhow::Result<bool> {
        if replacement_review_id.is_empty()
            || replacement_review_id.len() > 128
            || replacement_review_id.chars().any(char::is_control)
        {
            anyhow::bail!("Tally replacement review ID is invalid");
        }
        let mut cache = self
            .session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        let Some(current) = cache.as_ref() else {
            self.armed = false;
            return Ok(false);
        };
        if !self.armed || current.review_id != self.review_id || !current.reserved {
            self.armed = false;
            return Ok(false);
        }
        let freshness_origin_unix_ms = current.freshness_origin_unix_ms;
        *cache = Some(CachedProbe {
            review_id: replacement_review_id,
            observed_at_unix_ms,
            freshness_origin_unix_ms,
            result,
            reserved: false,
        });
        self.armed = false;
        Ok(true)
    }

    fn finish(&mut self, consume: bool) -> anyhow::Result<bool> {
        if !self.armed {
            return Ok(false);
        }
        let mut cache = self
            .session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        let is_reserved_match = cache
            .as_ref()
            .is_some_and(|probe| probe.review_id == self.review_id && probe.reserved);
        if !is_reserved_match {
            self.armed = false;
            return Ok(false);
        }
        if consume {
            cache.take();
        } else if let Some(probe) = cache.as_mut() {
            probe.reserved = false;
        }
        self.armed = false;
        Ok(true)
    }
}

impl Drop for CachedProbeReservation {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl Default for TallyRuntime {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            runtime_identity: Arc::new(()),
            control: PortableReadRuntime::default(),
            #[cfg(test)]
            transport_policy: None,
        }
    }
}

fn apply_scoped_standard_identity(result: &mut TallyProbeResult, company: TallyCompany) {
    result.companies = vec![company];
    for feature in [
        bridge_tally_core::CapabilityFeatureId::LoadedCompanies,
        bridge_tally_core::CapabilityFeatureId::StableCompanyIdentity,
    ] {
        result.profile.features.insert(
            feature,
            bridge_tally_core::CapabilityEvidence {
                state: bridge_tally_core::CapabilityState::Supported,
                confidence: bridge_tally_core::EvidenceConfidence::Observed,
                safe_reason_code: Some("scoped_standard_identity_observed".to_string()),
            },
        );
    }
    result.profile.transports.insert(
        bridge_tally_core::TransportId::XmlHttp,
        bridge_tally_core::CapabilityEvidence {
            state: bridge_tally_core::CapabilityState::Supported,
            confidence: bridge_tally_core::EvidenceConfidence::Observed,
            safe_reason_code: Some("standard_ledger_identity_profile_observed".to_string()),
        },
    );
}

impl TallyRuntime {
    #[cfg(test)]
    pub(crate) fn with_transport_policy(policy: bridge_tally_transport::TransportPolicy) -> Self {
        Self {
            transport_policy: Some(policy),
            ..Self::default()
        }
    }

    fn session(&self, config: TallyConfig) -> anyhow::Result<Arc<TallySession>> {
        let endpoint = EndpointKey::from_config(&config)?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        if let Some(slot) = sessions.get_mut(&endpoint) {
            slot.last_used = Instant::now();
            return Ok(Arc::clone(&slot.session));
        }
        if sessions.len() >= MAX_ENDPOINT_SESSIONS {
            let inactive_oldest =
                sessions
                    .iter()
                    .filter(|(_, slot)| {
                        Arc::strong_count(&slot.session) == 1
                            && slot.session.cached_probe.read().is_ok_and(|cache| {
                                !cache.as_ref().is_some_and(|probe| probe.reserved)
                            })
                    })
                    .min_by_key(|(_, slot)| slot.last_used)
                    .map(|(key, _)| key.clone());
            if let Some(key) = inactive_oldest {
                sessions.remove(&key);
            } else {
                anyhow::bail!("Tally runtime endpoint-session limit is in use");
            }
        }
        #[cfg(test)]
        let session = Arc::new(match self.transport_policy {
            Some(policy) => TallySession::with_transport_policy(endpoint.clone(), config, policy)?,
            None => TallySession::new(endpoint.clone(), config)?,
        });
        #[cfg(not(test))]
        let session = Arc::new(TallySession::new(endpoint.clone(), config)?);
        sessions.insert(
            endpoint,
            SessionSlot {
                session: Arc::clone(&session),
                last_used: Instant::now(),
            },
        );
        Ok(session)
    }

    async fn execute<T, F, Fut>(
        &self,
        config: TallyConfig,
        operation_class: ReadOperation,
        retry: ReadRetryPolicy,
        operation: F,
    ) -> anyhow::Result<T>
    where
        F: FnMut(TallyClient) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        self.execute_cancellable(config, None, operation_class, retry, operation)
            .await
    }

    async fn execute_cancellable<T, F, Fut>(
        &self,
        config: TallyConfig,
        external_cancellation: Option<CancellationToken>,
        operation_class: ReadOperation,
        retry: ReadRetryPolicy,
        mut operation: F,
    ) -> anyhow::Result<T>
    where
        F: FnMut(TallyClient) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let session = self.session(config)?;
        let request = session.begin_request()?;
        let client = session.client.clone();
        let endpoint = EndpointIdentity::new(session.endpoint.as_str().to_string())
            .map_err(anyhow::Error::new)?;
        let effective_cancellation = request.cancellation.child_token();
        let external_watcher = external_cancellation.map(|external| {
            let effective_cancellation = effective_cancellation.clone();
            tokio::spawn(async move {
                external.cancelled().await;
                effective_cancellation.cancel();
            })
        });
        let result = self
            .control
            .execute_read(
                endpoint,
                operation_class,
                retry,
                effective_cancellation,
                move |_| {
                    let attempt_client = client.clone();
                    attempt_client.reset_observed_body_bytes();
                    let future = operation(attempt_client.clone());
                    async move {
                        let observed_body_bytes = || {
                            if operation_class == ReadOperation::Capability {
                                BodyBytesObservation::Unavailable
                            } else {
                                attempt_client
                                    .observed_body_bytes()
                                    .map(BodyBytesObservation::Observed)
                                    .unwrap_or(BodyBytesObservation::Unavailable)
                            }
                        };
                        match future.await {
                            Ok(value) => ReadAttempt::Success {
                                value,
                                observed_body_bytes: observed_body_bytes(),
                            },
                            Err(error) => ReadAttempt::Failure {
                                class: classify_failure(&error),
                                error,
                                observed_body_bytes: observed_body_bytes(),
                            },
                        }
                    }
                },
            )
            .await;
        if let Some(watcher) = external_watcher {
            watcher.abort();
        }
        let health_outcome = match &result {
            Ok(_) => HealthOutcome::TransportSuccess,
            Err(ReadExecutionError::Attempt(error)) => classify_error(error),
            Err(ReadExecutionError::Cancelled) => HealthOutcome::Cancelled,
            Err(
                ReadExecutionError::QueueDeadline
                | ReadExecutionError::CircuitRejected { .. }
                | ReadExecutionError::EndpointSessionLimit,
            ) => HealthOutcome::ApplicationRejected,
        };
        session.record_result(health_outcome);
        result.map_err(map_execution_error)
    }

    pub async fn check_connection(&self, config: TallyConfig) -> anyhow::Result<ConnectionStatus> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::Status,
            ReadRetryPolicy::transient_default(),
            |client| async move { client.check_connection_strict().await },
        )
        .await
    }

    pub async fn probe_with_observation(
        &self,
        config: TallyConfig,
    ) -> anyhow::Result<(String, i64, TallyProbeResult)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let session = self.session(config.clone())?;
        let result = self
            .execute(
                config,
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe().await },
            )
            .await?;
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let review_id = uuid::Uuid::new_v4().to_string();
        let mut cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            anyhow::bail!("Tally reviewed setup save is in progress");
        }
        *cache = Some(CachedProbe {
            review_id: review_id.clone(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: result.clone(),
            reserved: false,
        });
        Ok((review_id, observed_at_unix_ms, result))
    }

    /// Establishes one setup-review candidate only after the direct listing is
    /// re-read and a separate shaped standard ledger collection confirms its
    /// computed name/GUID context. This never upgrades the direct listing
    /// itself into evidence.
    pub async fn bootstrap_direct_company_with_observation(
        &self,
        config: TallyConfig,
        candidate_name: String,
    ) -> anyhow::Result<(String, i64, TallyProbeResult)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let session = self.session(config.clone())?;
        let mut result = self
            .execute(
                config.clone(),
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe().await },
            )
            .await?;
        if !result.companies.is_empty() {
            anyhow::bail!("Tally direct company bootstrap was not required");
        }
        let company = self
            .execute(
                config,
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                move |client| {
                    let candidate_name = candidate_name.clone();
                    async move { client.bootstrap_direct_company(&candidate_name).await }
                },
            )
            .await?;
        apply_scoped_standard_identity(&mut result, company);
        let observed_at_unix_ms = chrono::Utc::now().timestamp_millis();
        let review_id = uuid::Uuid::new_v4().to_string();
        let mut cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            anyhow::bail!("Tally reviewed setup save is in progress");
        }
        *cache = Some(CachedProbe {
            review_id: review_id.clone(),
            observed_at_unix_ms,
            freshness_origin_unix_ms: observed_at_unix_ms,
            result: result.clone(),
            reserved: false,
        });
        Ok((review_id, observed_at_unix_ms, result))
    }

    /// Observe the endpoint for snapshot admission without creating or replacing
    /// an interactive setup review. Snapshot start and end probes are lifecycle
    /// evidence, not user-reviewed setup state, so they must remain uncached.
    pub(crate) async fn snapshot_probe_with_observation(
        &self,
        config: TallyConfig,
        expected_company_name: &str,
    ) -> anyhow::Result<(i64, TallyProbeResult)> {
        let _lease = self.begin_ordinary_read(&config)?;
        let mut result = self
            .execute(
                config.clone(),
                ReadOperation::Capability,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                |client| async move { client.probe().await },
            )
            .await?;
        if result.companies.is_empty() {
            let expected_company_name = expected_company_name.to_string();
            let company = self
                .execute(
                    config,
                    ReadOperation::Capability,
                    ReadRetryPolicy::SINGLE_ATTEMPT,
                    move |client| {
                        let expected_company_name = expected_company_name.clone();
                        async move {
                            client
                                .bootstrap_direct_company(&expected_company_name)
                                .await
                        }
                    },
                )
                .await?;
            apply_scoped_standard_identity(&mut result, company);
        }
        Ok((chrono::Utc::now().timestamp_millis(), result))
    }

    pub async fn probe(&self, config: TallyConfig) -> anyhow::Result<TallyProbeResult> {
        self.probe_with_observation(config)
            .await
            .map(|(_, _, result)| result)
    }

    pub async fn fetch_companies(&self, config: TallyConfig) -> anyhow::Result<Vec<TallyCompany>> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::CompanyList,
            ReadRetryPolicy::transient_default(),
            |client| async move { client.fetch_companies().await },
        )
        .await
    }

    pub async fn fetch_ledgers(
        &self,
        config: TallyConfig,
        company: String,
        expected_company_guid: String,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let company = company.clone();
                let expected_company_guid = expected_company_guid.clone();
                async move { client.fetch_ledgers(&company, &expected_company_guid).await }
            },
        )
        .await
    }

    /// Fetches the limited, documented standard collection response used for
    /// compatibility diagnostics. This is intentionally separate from the
    /// Bridge ledger export: it returns only ledger names and parents and is
    /// never eligible for qualification or synchronization.
    pub async fn fetch_standard_ledger_catalog(
        &self,
        config: TallyConfig,
        company: String,
        expected_company_guid: String,
    ) -> anyhow::Result<Vec<TallyLedger>> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let company = company.clone();
                let expected_company_guid = expected_company_guid.clone();
                async move {
                    client
                        .fetch_standard_ledger_catalog(&company, &expected_company_guid)
                        .await
                }
            },
        )
        .await
    }

    /// Executes one sealed, serial canary readback. This remains a read-only
    /// internal primitive for the future write coordinator; it never exposes
    /// response XML to commands, the UI, or persistence.
    #[allow(
        dead_code,
        reason = "the sealed runtime seam is intentionally staged before the write coordinator"
    )]
    pub(crate) async fn fetch_ledger_canary_readback(
        &self,
        config: TallyConfig,
        company: ValidatedCompanyName,
        ledger_name: ValidatedCanaryLedgerName,
        identity_query_sha256: ValidatedIdentityQuerySha256,
        expected_company_guid: String,
    ) -> anyhow::Result<LedgerCanaryReadbackXml> {
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let company = company.clone();
                let ledger_name = ledger_name.clone();
                let identity_query_sha256 = identity_query_sha256.clone();
                let expected_company_guid = expected_company_guid.clone();
                async move {
                    client
                        .fetch_ledger_canary_readback(
                            company,
                            ledger_name,
                            identity_query_sha256,
                            &expected_company_guid,
                        )
                        .await
                }
            },
        )
        .await
    }

    pub async fn qualify_selected_ledgers(
        &self,
        config: TallyConfig,
        reservation: &CachedProbeReservation,
        company: String,
        expected_company_guid: String,
    ) -> anyhow::Result<SelectedReadObservation> {
        reservation.authorize(self, &config)?;
        self.execute(
            config,
            ReadOperation::MasterExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let company = company.clone();
                let expected_company_guid = expected_company_guid.clone();
                async move {
                    client
                        .qualify_selected_ledgers(&company, &expected_company_guid)
                        .await
                }
            },
        )
        .await
    }

    pub async fn fetch_vouchers(
        &self,
        config: TallyConfig,
        company: String,
        expected_company_guid: String,
        from: String,
        to: String,
    ) -> anyhow::Result<Vec<TallyVoucher>> {
        let _lease = self.begin_ordinary_read(&config)?;
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let company = company.clone();
                let expected_company_guid = expected_company_guid.clone();
                let from = from.clone();
                let to = to.clone();
                async move {
                    client
                        .fetch_vouchers(&company, &expected_company_guid, &from, &to)
                        .await
                }
            },
        )
        .await
    }

    pub async fn qualify_selected_vouchers(
        &self,
        config: TallyConfig,
        reservation: &CachedProbeReservation,
        company: String,
        expected_company_guid: String,
        from: String,
        to: String,
    ) -> anyhow::Result<SelectedReadObservation> {
        reservation.authorize(self, &config)?;
        self.execute(
            config,
            ReadOperation::VoucherExport,
            ReadRetryPolicy::SINGLE_ATTEMPT,
            move |client| {
                let company = company.clone();
                let expected_company_guid = expected_company_guid.clone();
                let from = from.clone();
                let to = to.clone();
                async move {
                    client
                        .qualify_selected_vouchers(&company, &expected_company_guid, &from, &to)
                        .await
                }
            },
        )
        .await
    }

    pub(super) async fn post_xml_cancellable_validated<P>(
        &self,
        config: TallyConfig,
        request: SealedReadRequest,
        cancellation: CancellationToken,
        validate_application_response: P,
    ) -> anyhow::Result<String>
    where
        P: Fn(&str) -> bool + Send + Sync,
    {
        let _lease = self.begin_ordinary_read(&config)?;
        let request_xml = request.into_xml();
        let validate_application_response = Arc::new(validate_application_response);
        self.execute_cancellable(
            config,
            Some(cancellation),
            ReadOperation::ReportExport,
            ReadRetryPolicy::transient_default(),
            move |client| {
                let request_xml = request_xml.clone();
                let validate_application_response = Arc::clone(&validate_application_response);
                async move {
                    let xml = client.post_xml(request_xml).await?;
                    if validate_application_response(&xml) {
                        Ok(xml)
                    } else {
                        Err(anyhow::Error::new(
                            TallyRuntimeReadError::ApplicationResponseRejected,
                        ))
                    }
                }
            },
        )
        .await
    }

    pub fn cancel_request(&self, request_id: &str) -> anyhow::Result<bool> {
        if request_id.is_empty()
            || request_id.len() > 256
            || request_id.chars().any(char::is_control)
        {
            anyhow::bail!("Tally request ID is invalid");
        }
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        for slot in sessions.values() {
            if slot.session.cancel(request_id)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn snapshots(&self) -> anyhow::Result<Vec<TallySessionSnapshot>> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let mut snapshots = sessions
            .values()
            .map(|slot| {
                let endpoint = EndpointIdentity::new(slot.session.endpoint.as_str().to_string())
                    .map_err(anyhow::Error::new)?;
                slot.session
                    .snapshot(self.control.endpoint_snapshot(&endpoint))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        snapshots.sort_by(|left, right| left.canonical_endpoint.cmp(&right.canonical_endpoint));
        Ok(snapshots)
    }

    pub fn cached_probe(&self, config: &TallyConfig) -> anyhow::Result<Option<TallyProbeResult>> {
        let endpoint = EndpointKey::from_config(config)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let Some(session) = sessions
            .get(&endpoint)
            .map(|slot| Arc::clone(&slot.session))
        else {
            return Ok(None);
        };
        let cached = session
            .cached_probe
            .read()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?
            .as_ref()
            .map(|probe| probe.result.clone());
        Ok(cached)
    }

    pub fn reserve_cached_probe_fresh(
        &self,
        config: &TallyConfig,
        expected_review_id: &str,
        max_age_ms: i64,
    ) -> anyhow::Result<Option<CachedProbeReservation>> {
        if !(1..=600_000).contains(&max_age_ms) {
            anyhow::bail!("Tally capability cache freshness bound is invalid");
        }
        let endpoint = EndpointKey::from_config(config)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let Some(session) = sessions
            .get(&endpoint)
            .map(|slot| Arc::clone(&slot.session))
        else {
            return Ok(None);
        };
        let now = chrono::Utc::now().timestamp_millis();
        let mut cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if session.active_ordinary_reads.load(Ordering::Acquire) != 0 {
            anyhow::bail!("Tally read operation is already in progress");
        }
        let Some(probe) = cache.as_mut() else {
            return Ok(None);
        };
        if probe.review_id != expected_review_id
            || probe.reserved
            || probe.freshness_origin_unix_ms > now
            || now.saturating_sub(probe.freshness_origin_unix_ms) > max_age_ms
        {
            return Ok(None);
        }
        probe.reserved = true;
        let reservation = CachedProbeReservation {
            session: Arc::clone(&session),
            runtime_identity: Arc::clone(&self.runtime_identity),
            review_id: probe.review_id.clone(),
            observed_at_unix_ms: probe.observed_at_unix_ms,
            result: probe.result.clone(),
            armed: true,
        };
        drop(cache);
        Ok(Some(reservation))
    }

    pub fn telemetry_preview(&self) -> anyhow::Result<TallyTelemetryPreviewExport> {
        let export = self.control.collector().privacy_reduced_export_v2()?;
        Ok(TallyTelemetryPreviewExport {
            schema: TELEMETRY_PREVIEW_SCHEMA,
            payload_sha256: export.payload_sha256().to_string(),
            preview_json: export.json().to_string(),
        })
    }

    fn begin_ordinary_read(&self, config: &TallyConfig) -> anyhow::Result<OrdinaryReadLease> {
        let endpoint = EndpointKey::from_config(config)?;
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("Tally runtime session registry is unavailable"))?;
        let Some(session) = sessions
            .get(&endpoint)
            .map(|slot| Arc::clone(&slot.session))
        else {
            drop(sessions);
            let session = self.session(config.clone())?;
            return self.begin_ordinary_read_for_session(session);
        };
        drop(sessions);
        self.begin_ordinary_read_for_session(session)
    }

    fn begin_ordinary_read_for_session(
        &self,
        session: Arc<TallySession>,
    ) -> anyhow::Result<OrdinaryReadLease> {
        let cache = session
            .cached_probe
            .write()
            .map_err(|_| anyhow::anyhow!("Tally capability cache is unavailable"))?;
        if cache.as_ref().is_some_and(|probe| probe.reserved) {
            anyhow::bail!("Tally reviewed setup operation is in progress");
        }
        session
            .active_ordinary_reads
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("Tally read admission capacity is unavailable"))?;
        drop(cache);
        Ok(OrdinaryReadLease { session })
    }
}

fn classify_error(error: &anyhow::Error) -> HealthOutcome {
    match classify_failure(error) {
        ReadFailureClass::Connection
        | ReadFailureClass::RequestTimeout
        | ReadFailureClass::RequestFailed
        | ReadFailureClass::HttpServer
        | ReadFailureClass::RateLimited => HealthOutcome::TransportFailure,
        ReadFailureClass::HttpClient
        | ReadFailureClass::SizeLimit
        | ReadFailureClass::Decode
        | ReadFailureClass::Application
        | ReadFailureClass::Parse
        | ReadFailureClass::Validation
        | ReadFailureClass::CompanyMismatch => HealthOutcome::ApplicationRejected,
    }
}

fn classify_failure(error: &anyhow::Error) -> ReadFailureClass {
    if matches!(
        error.downcast_ref::<TallyRuntimeReadError>(),
        Some(TallyRuntimeReadError::ApplicationResponseRejected)
    ) {
        return ReadFailureClass::Application;
    }
    match error.downcast_ref::<TallyTransportError>() {
        Some(TallyTransportError::ConnectionFailed) => ReadFailureClass::Connection,
        Some(TallyTransportError::RequestTimedOut) => ReadFailureClass::RequestTimeout,
        Some(
            TallyTransportError::RequestFailed
            | TallyTransportError::ResponseTruncated
            | TallyTransportError::ResponseReadFailed,
        ) => ReadFailureClass::RequestFailed,
        Some(TallyTransportError::HttpStatus { status: 429 }) => ReadFailureClass::RateLimited,
        Some(TallyTransportError::HttpStatus { status }) if *status >= 500 => {
            ReadFailureClass::HttpServer
        }
        Some(TallyTransportError::HttpStatus { .. }) => ReadFailureClass::HttpClient,
        Some(
            TallyTransportError::RequestTooLarge { .. }
            | TallyTransportError::ResponseTooLarge { .. },
        ) => ReadFailureClass::SizeLimit,
        Some(
            TallyTransportError::UnsupportedContentEncoding
            | TallyTransportError::InvalidEncoding { .. },
        ) => ReadFailureClass::Decode,
        Some(
            TallyTransportError::EndpointInvalid { .. }
            | TallyTransportError::PolicyInvalid { .. }
            | TallyTransportError::ClientInitializationFailed,
        ) => ReadFailureClass::Validation,
        None => ReadFailureClass::Validation,
    }
}

fn map_execution_error(error: ReadExecutionError<anyhow::Error>) -> anyhow::Error {
    match error {
        ReadExecutionError::Attempt(error) => error,
        ReadExecutionError::Cancelled => anyhow::Error::new(TallyRuntimeControlError::Cancelled),
        ReadExecutionError::QueueDeadline => {
            anyhow::Error::new(TallyRuntimeControlError::QueueDeadline)
        }
        ReadExecutionError::CircuitRejected {
            reason: bridge_tally_runtime::CircuitRejectReason::Cooldown,
            ..
        } => anyhow::Error::new(TallyRuntimeControlError::CircuitCooldown),
        ReadExecutionError::CircuitRejected {
            reason: bridge_tally_runtime::CircuitRejectReason::HalfOpenProbeInFlight,
            ..
        } => anyhow::Error::new(TallyRuntimeControlError::HalfOpenProbeInFlight),
        ReadExecutionError::EndpointSessionLimit => {
            anyhow::Error::new(TallyRuntimeControlError::EndpointSessionCapacity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tally::TallyProduct;
    use bridge_tally_core::CapabilityProfile;
    use std::collections::BTreeMap;

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
            }],
            profile: CapabilityProfile {
                profile_version: 2,
                product: "Unknown".to_string(),
                release: None,
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
                "Synthetic Company".to_string(),
                "synthetic-guid".to_string(),
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
                    "Synthetic Company".to_string(),
                    "synthetic-guid".to_string(),
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
}
