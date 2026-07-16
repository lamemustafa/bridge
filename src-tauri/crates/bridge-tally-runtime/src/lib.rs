//! Portable execution control for read-only Tally operations.
//!
//! This crate authenticates no endpoint and establishes no accounting or
//! support claim. It controls local execution and emits only fixed-cardinality,
//! privacy-reduced observations.

use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    fmt,
    future::Future,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use bridge_tally_observability::{
    AttemptObservation, ObservationSink, RequestClass, ResponseOutcome, TelemetryCollector,
};
pub use bridge_tally_observability::{BodyBytesObservation, CircuitRejectReason};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use tokio_util::sync::CancellationToken;

const MAX_ENDPOINT_IDENTITY_BYTES: usize = 512;
const MAX_QUEUE_DEADLINE: Duration = Duration::from_secs(120);
const MAX_REQUEST_SPACING: Duration = Duration::from_secs(10);
const MAX_CIRCUIT_COOLDOWN: Duration = Duration::from_secs(10 * 60);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
pub const TELEMETRY_PREVIEW_SCHEMA: &str = bridge_tally_observability::PREVIEW_SCHEMA;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EndpointIdentity(String);

impl EndpointIdentity {
    pub fn new(value: impl Into<String>) -> Result<Self, RuntimeConfigurationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_ENDPOINT_IDENTITY_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(RuntimeConfigurationError::EndpointIdentityInvalid);
        }
        Ok(Self(value))
    }

    fn private_value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EndpointIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EndpointIdentity([redacted])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOperation {
    Status,
    Capability,
    CompanyList,
    MasterExport,
    VoucherExport,
    ReportExport,
    OtherRead,
}

impl ReadOperation {
    pub const fn request_class(self) -> RequestClass {
        match self {
            Self::Status => RequestClass::Status,
            Self::Capability => RequestClass::Capability,
            Self::CompanyList => RequestClass::CompanyList,
            Self::MasterExport => RequestClass::MasterExport,
            Self::VoucherExport => RequestClass::VoucherExport,
            Self::ReportExport => RequestClass::ReportExport,
            Self::OtherRead => RequestClass::OtherRead,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailureClass {
    Connection,
    RequestTimeout,
    RequestFailed,
    HttpServer,
    RateLimited,
    HttpClient,
    SizeLimit,
    Decode,
    Application,
    Parse,
    Validation,
    CompanyMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointCircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointRuntimeSnapshot {
    pub consecutive_failures: u32,
    pub circuit_state: EndpointCircuitState,
    pub circuit_retry_after_unix_ms: Option<i64>,
    pub half_open_probe_in_flight: bool,
    pub last_failure_unix_ms: Option<i64>,
}

impl ReadFailureClass {
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::Connection
                | Self::RequestTimeout
                | Self::RequestFailed
                | Self::HttpServer
                | Self::RateLimited
        )
    }

    const fn response_outcome(self) -> ResponseOutcome {
        match self {
            Self::Connection | Self::RequestFailed => ResponseOutcome::Transport,
            Self::RequestTimeout => ResponseOutcome::Timeout,
            Self::HttpServer | Self::RateLimited | Self::HttpClient => ResponseOutcome::HttpStatus,
            Self::SizeLimit => ResponseOutcome::SizeLimit,
            Self::Decode => ResponseOutcome::Decode,
            Self::Application => ResponseOutcome::Application,
            Self::Parse => ResponseOutcome::Parse,
            Self::Validation | Self::CompanyMismatch => ResponseOutcome::Validation,
        }
    }

    const fn circuit_outcome(self) -> CircuitOutcome {
        match self {
            Self::Connection
            | Self::RequestTimeout
            | Self::RequestFailed
            | Self::HttpServer
            | Self::RateLimited => CircuitOutcome::TransportFailure,
            Self::HttpClient
            | Self::SizeLimit
            | Self::Decode
            | Self::Application
            | Self::Parse
            | Self::Validation
            | Self::CompanyMismatch => CircuitOutcome::ApplicationRejected,
        }
    }
}

pub enum ReadAttempt<T, E> {
    Success {
        value: T,
        observed_body_bytes: BodyBytesObservation,
    },
    Failure {
        error: E,
        class: ReadFailureClass,
        observed_body_bytes: BodyBytesObservation,
    },
}

impl<T, E> ReadAttempt<T, E> {
    fn observation(&self) -> (ResponseOutcome, BodyBytesObservation) {
        match self {
            Self::Success {
                observed_body_bytes,
                ..
            } => (ResponseOutcome::Success, *observed_body_bytes),
            Self::Failure {
                class,
                observed_body_bytes,
                ..
            } => (class.response_outcome(), *observed_body_bytes),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadRetryPolicy {
    maximum_attempts: u8,
    base_delay: Duration,
    maximum_delay: Duration,
    jitter_percent: u8,
}

impl ReadRetryPolicy {
    pub const SINGLE_ATTEMPT: Self = Self {
        maximum_attempts: 1,
        base_delay: Duration::ZERO,
        maximum_delay: Duration::ZERO,
        jitter_percent: 0,
    };

    pub fn transient_default() -> Self {
        Self {
            maximum_attempts: 3,
            base_delay: Duration::from_millis(250),
            maximum_delay: Duration::from_secs(2),
            jitter_percent: 20,
        }
    }

    pub fn new(
        maximum_attempts: u8,
        base_delay: Duration,
        maximum_delay: Duration,
        jitter_percent: u8,
    ) -> Result<Self, RuntimeConfigurationError> {
        if maximum_attempts == 0
            || maximum_attempts > 5
            || base_delay > maximum_delay
            || maximum_delay > MAX_RETRY_DELAY
            || jitter_percent > 25
        {
            return Err(RuntimeConfigurationError::RetryPolicyInvalid);
        }
        Ok(Self {
            maximum_attempts,
            base_delay,
            maximum_delay,
            jitter_percent,
        })
    }

    pub const fn maximum_attempts(self) -> u8 {
        self.maximum_attempts
    }

    fn delay_after_failure(self, completed_attempt: u8, entropy: u64) -> Duration {
        if self.base_delay.is_zero() || completed_attempt >= self.maximum_attempts {
            return Duration::ZERO;
        }
        let exponent = u32::from(completed_attempt.saturating_sub(1));
        let multiplier = 1_u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let base = self
            .base_delay
            .saturating_mul(multiplier)
            .min(self.maximum_delay);
        let jitter_ceiling = base
            .as_millis()
            .saturating_mul(u128::from(self.jitter_percent))
            / 100;
        let jitter = if jitter_ceiling == 0 {
            0
        } else {
            u128::from(entropy) % (jitter_ceiling + 1)
        };
        base.saturating_add(Duration::from_millis(
            u64::try_from(jitter).unwrap_or(u64::MAX),
        ))
        .min(MAX_RETRY_DELAY)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimePolicy {
    pub queue_deadline: Duration,
    pub request_spacing: Duration,
    pub circuit_failure_threshold: u32,
    pub circuit_cooldown: Duration,
    pub maximum_endpoint_sessions: usize,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            queue_deadline: Duration::from_secs(30),
            request_spacing: Duration::from_millis(500),
            circuit_failure_threshold: 3,
            circuit_cooldown: Duration::from_secs(10),
            maximum_endpoint_sessions: 32,
        }
    }
}

impl RuntimePolicy {
    fn validate(self) -> Result<Self, RuntimeConfigurationError> {
        if self.queue_deadline.is_zero()
            || self.queue_deadline > MAX_QUEUE_DEADLINE
            || self.request_spacing > MAX_REQUEST_SPACING
            || self.circuit_failure_threshold == 0
            || self.circuit_failure_threshold > 100
            || self.circuit_cooldown.is_zero()
            || self.circuit_cooldown > MAX_CIRCUIT_COOLDOWN
            || self.maximum_endpoint_sessions == 0
            || self.maximum_endpoint_sessions > 128
        {
            return Err(RuntimeConfigurationError::RuntimePolicyInvalid);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeConfigurationError {
    EndpointIdentityInvalid,
    RetryPolicyInvalid,
    RuntimePolicyInvalid,
}

impl RuntimeConfigurationError {
    pub const fn safe_code(self) -> &'static str {
        match self {
            Self::EndpointIdentityInvalid => "endpoint_identity_invalid",
            Self::RetryPolicyInvalid => "read_retry_policy_invalid",
            Self::RuntimePolicyInvalid => "endpoint_runtime_policy_invalid",
        }
    }
}

impl fmt::Display for RuntimeConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.safe_code())
    }
}

impl std::error::Error for RuntimeConfigurationError {}

#[derive(Debug)]
pub enum ReadExecutionError<E> {
    QueueDeadline,
    Cancelled,
    CircuitRejected {
        reason: CircuitRejectReason,
        retry_after_unix_ms: Option<i64>,
    },
    EndpointSessionLimit,
    Attempt(E),
}

impl<E> ReadExecutionError<E> {
    pub const fn safe_code(&self) -> &'static str {
        match self {
            Self::QueueDeadline => "endpoint_queue_deadline_exceeded",
            Self::Cancelled => "read_request_cancelled",
            Self::CircuitRejected {
                reason: CircuitRejectReason::Cooldown,
                ..
            } => "endpoint_circuit_cooldown",
            Self::CircuitRejected {
                reason: CircuitRejectReason::HalfOpenProbeInFlight,
                ..
            } => "endpoint_half_open_probe_in_flight",
            Self::EndpointSessionLimit => "endpoint_session_limit_in_use",
            Self::Attempt(_) => "read_attempt_failed",
        }
    }

    pub fn into_attempt_error(self) -> Option<E> {
        match self {
            Self::Attempt(error) => Some(error),
            _ => None,
        }
    }
}

struct GateState {
    next_request_not_before: Option<Instant>,
}

struct SpacingGuard<'a> {
    state: MutexGuard<'a, GateState>,
    spacing: Duration,
}

impl Drop for SpacingGuard<'_> {
    fn drop(&mut self) {
        self.state.next_request_not_before = Some(Instant::now() + self.spacing);
    }
}

#[derive(Default)]
struct CircuitState {
    consecutive_failures: u32,
    last_failure_unix_ms: Option<i64>,
    half_open_probe_in_flight: bool,
}

struct CircuitBreaker {
    state: Mutex<CircuitState>,
    threshold: u32,
    cooldown: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CircuitOutcome {
    TransportSuccess,
    TransportFailure,
    ApplicationRejected,
    Cancelled,
}

struct CircuitPermit<'a> {
    circuit: &'a CircuitBreaker,
    half_open: bool,
    completed: bool,
}

impl CircuitBreaker {
    fn admit(
        &self,
        now_unix_ms: i64,
    ) -> Result<CircuitPermit<'_>, (CircuitRejectReason, Option<i64>)> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.consecutive_failures < self.threshold {
            return Ok(CircuitPermit {
                circuit: self,
                half_open: false,
                completed: false,
            });
        }
        let retry_after = state
            .last_failure_unix_ms
            .unwrap_or(now_unix_ms)
            .saturating_add(duration_millis_i64(self.cooldown));
        if now_unix_ms < retry_after {
            return Err((CircuitRejectReason::Cooldown, Some(retry_after)));
        }
        if state.half_open_probe_in_flight {
            return Err((CircuitRejectReason::HalfOpenProbeInFlight, None));
        }
        state.half_open_probe_in_flight = true;
        Ok(CircuitPermit {
            circuit: self,
            half_open: true,
            completed: false,
        })
    }

    fn record(&self, outcome: CircuitOutcome, now_unix_ms: i64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.half_open_probe_in_flight = false;
        match outcome {
            CircuitOutcome::TransportSuccess => {
                state.consecutive_failures = 0;
            }
            CircuitOutcome::TransportFailure => {
                state.consecutive_failures = state.consecutive_failures.saturating_add(1);
                state.last_failure_unix_ms = Some(now_unix_ms);
            }
            CircuitOutcome::ApplicationRejected | CircuitOutcome::Cancelled => {}
        }
    }

    fn release_half_open(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.half_open_probe_in_flight = false;
    }

    fn snapshot(&self, now_unix_ms: i64) -> EndpointRuntimeSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let retry_after = state
            .last_failure_unix_ms
            .map(|last_failure| last_failure.saturating_add(duration_millis_i64(self.cooldown)));
        let circuit_state = if state.consecutive_failures < self.threshold {
            EndpointCircuitState::Closed
        } else if retry_after.is_some_and(|deadline| now_unix_ms < deadline) {
            EndpointCircuitState::Open
        } else {
            EndpointCircuitState::HalfOpen
        };
        EndpointRuntimeSnapshot {
            consecutive_failures: state.consecutive_failures,
            circuit_state,
            circuit_retry_after_unix_ms: match circuit_state {
                EndpointCircuitState::Open => retry_after,
                EndpointCircuitState::Closed | EndpointCircuitState::HalfOpen => None,
            },
            half_open_probe_in_flight: state.half_open_probe_in_flight,
            last_failure_unix_ms: state.last_failure_unix_ms,
        }
    }
}

impl CircuitPermit<'_> {
    fn complete(mut self, outcome: CircuitOutcome, now_unix_ms: i64) {
        self.circuit.record(outcome, now_unix_ms);
        self.completed = true;
    }
}

impl Drop for CircuitPermit<'_> {
    fn drop(&mut self) {
        if self.half_open && !self.completed {
            self.circuit.release_half_open();
        }
    }
}

struct EndpointSession {
    gate: AsyncMutex<GateState>,
    circuit: CircuitBreaker,
    sequence: std::sync::atomic::AtomicU64,
}

impl EndpointSession {
    fn new(policy: RuntimePolicy) -> Self {
        Self {
            gate: AsyncMutex::new(GateState {
                next_request_not_before: None,
            }),
            circuit: CircuitBreaker {
                state: Mutex::new(CircuitState::default()),
                threshold: policy.circuit_failure_threshold,
                cooldown: policy.circuit_cooldown,
            },
            sequence: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

struct SessionSlot {
    session: Arc<EndpointSession>,
    last_used: Instant,
}

#[derive(Clone)]
pub struct PortableReadRuntime {
    sessions: Arc<Mutex<HashMap<EndpointIdentity, SessionSlot>>>,
    collector: Arc<TelemetryCollector>,
    policy: RuntimePolicy,
}

impl fmt::Debug for PortableReadRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PortableReadRuntime")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl Default for PortableReadRuntime {
    fn default() -> Self {
        Self::new(RuntimePolicy::default()).expect("default runtime policy is valid")
    }
}

impl PortableReadRuntime {
    pub fn new(policy: RuntimePolicy) -> Result<Self, RuntimeConfigurationError> {
        Self::with_collector(policy, Arc::new(TelemetryCollector::new()))
    }

    pub fn with_collector(
        policy: RuntimePolicy,
        collector: Arc<TelemetryCollector>,
    ) -> Result<Self, RuntimeConfigurationError> {
        Ok(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            collector,
            policy: policy.validate()?,
        })
    }

    pub fn collector(&self) -> Arc<TelemetryCollector> {
        Arc::clone(&self.collector)
    }

    pub fn endpoint_snapshot(
        &self,
        endpoint: &EndpointIdentity,
    ) -> Option<EndpointRuntimeSnapshot> {
        let sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        sessions
            .get(endpoint)
            .map(|slot| slot.session.circuit.snapshot(now_unix_ms()))
    }

    pub async fn execute_read<T, E, F, Fut>(
        &self,
        endpoint: EndpointIdentity,
        operation: ReadOperation,
        retry: ReadRetryPolicy,
        cancellation: CancellationToken,
        mut request: F,
    ) -> Result<T, ReadExecutionError<E>>
    where
        F: FnMut(u8) -> Fut,
        Fut: Future<Output = ReadAttempt<T, E>>,
    {
        let session = self
            .session(&endpoint)
            .map_err(|()| ReadExecutionError::EndpointSessionLimit)?;
        let sequence = session
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        let class = operation.request_class();
        let mut attempt_number = 1_u8;
        loop {
            let attempt = match self
                .run_queued(&session, class, &cancellation, || request(attempt_number))
                .await
            {
                Ok(attempt) => attempt,
                Err(error) => return Err(error),
            };
            match attempt {
                ReadAttempt::Success { value, .. } => return Ok(value),
                ReadAttempt::Failure { error, class, .. } => {
                    if !class.retryable() || attempt_number >= retry.maximum_attempts {
                        return Err(ReadExecutionError::Attempt(error));
                    }
                    let entropy = retry_entropy(&endpoint, sequence, attempt_number);
                    let delay = retry.delay_after_failure(attempt_number, entropy);
                    attempt_number = attempt_number.saturating_add(1);
                    if !delay.is_zero() {
                        tokio::select! {
                            _ = cancellation.cancelled() => return Err(ReadExecutionError::Cancelled),
                            _ = tokio::time::sleep(delay) => {}
                        }
                    }
                }
            }
        }
    }

    fn session(&self, endpoint: &EndpointIdentity) -> Result<Arc<EndpointSession>, ()> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(slot) = sessions.get_mut(endpoint) {
            slot.last_used = Instant::now();
            return Ok(Arc::clone(&slot.session));
        }
        if sessions.len() >= self.policy.maximum_endpoint_sessions {
            let oldest = sessions
                .iter()
                .filter(|(_, slot)| Arc::strong_count(&slot.session) == 1)
                .min_by_key(|(_, slot)| slot.last_used)
                .map(|(identity, _)| identity.clone());
            if let Some(identity) = oldest {
                sessions.remove(&identity);
            } else {
                return Err(());
            }
        }
        let session = Arc::new(EndpointSession::new(self.policy));
        sessions.insert(
            endpoint.clone(),
            SessionSlot {
                session: Arc::clone(&session),
                last_used: Instant::now(),
            },
        );
        Ok(session)
    }

    async fn run_queued<T, E, F, Fut>(
        &self,
        session: &EndpointSession,
        class: RequestClass,
        cancellation: &CancellationToken,
        request: F,
    ) -> Result<ReadAttempt<T, E>, ReadExecutionError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ReadAttempt<T, E>>,
    {
        let queued_at = Instant::now();
        let state = tokio::select! {
            _ = cancellation.cancelled() => {
                self.collector.record_attempt(AttemptObservation::QueueCancelled {
                    class,
                    queue_wait: queued_at.elapsed(),
                });
                return Err(ReadExecutionError::Cancelled);
            }
            result = tokio::time::timeout(self.policy.queue_deadline, session.gate.lock()) => {
                match result {
                    Ok(state) => state,
                    Err(_) => {
                        self.collector.record_attempt(AttemptObservation::QueueDeadline {
                            class,
                            queue_wait: queued_at.elapsed(),
                        });
                        return Err(ReadExecutionError::QueueDeadline);
                    }
                }
            }
        };
        let queue_wait = queued_at.elapsed();
        if let Some(spacing_wait) = state
            .next_request_not_before
            .and_then(|not_before| not_before.checked_duration_since(Instant::now()))
        {
            let remaining = self
                .policy
                .queue_deadline
                .checked_sub(queued_at.elapsed())
                .ok_or_else(|| {
                    self.collector
                        .record_attempt(AttemptObservation::QueueDeadline { class, queue_wait });
                    ReadExecutionError::QueueDeadline
                })?;
            let wait = spacing_wait.min(remaining);
            tokio::select! {
                _ = cancellation.cancelled() => {
                    self.collector.record_attempt(AttemptObservation::QueueCancelled {
                        class,
                        queue_wait,
                    });
                    return Err(ReadExecutionError::Cancelled);
                }
                _ = tokio::time::sleep(wait) => {}
            }
            if wait < spacing_wait {
                self.collector
                    .record_attempt(AttemptObservation::QueueDeadline { class, queue_wait });
                return Err(ReadExecutionError::QueueDeadline);
            }
        }
        let permit = match session.circuit.admit(now_unix_ms()) {
            Ok(permit) => permit,
            Err((reason, retry_after_unix_ms)) => {
                self.collector
                    .record_attempt(AttemptObservation::CircuitRejected { class, reason });
                return Err(ReadExecutionError::CircuitRejected {
                    reason,
                    retry_after_unix_ms,
                });
            }
        };
        let _guard = SpacingGuard {
            state,
            spacing: self.policy.request_spacing,
        };
        let response_started = Instant::now();
        let attempt = tokio::select! {
            _ = cancellation.cancelled() => {
                self.collector.record_attempt(AttemptObservation::Response {
                    class,
                    queue_wait,
                    outcome: ResponseOutcome::Cancelled,
                    response_pipeline_elapsed: response_started.elapsed(),
                    observed_body_bytes: BodyBytesObservation::Unavailable,
                });
                permit.complete(CircuitOutcome::Cancelled, now_unix_ms());
                return Err(ReadExecutionError::Cancelled);
            }
            attempt = request() => attempt,
        };
        let (outcome, observed_body_bytes) = attempt.observation();
        self.collector.record_attempt(AttemptObservation::Response {
            class,
            queue_wait,
            outcome,
            response_pipeline_elapsed: response_started.elapsed(),
            observed_body_bytes,
        });
        let circuit_outcome = match &attempt {
            ReadAttempt::Success { .. } => CircuitOutcome::TransportSuccess,
            ReadAttempt::Failure { class, .. } => class.circuit_outcome(),
        };
        permit.complete(circuit_outcome, now_unix_ms());
        Ok(attempt)
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

fn duration_millis_i64(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn retry_entropy(endpoint: &EndpointIdentity, sequence: u64, attempt: u8) -> u64 {
    let mut hasher = DefaultHasher::new();
    endpoint.private_value().hash(&mut hasher);
    sequence.hash(&mut hasher);
    attempt.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn test_runtime(spacing: Duration, threshold: u32) -> PortableReadRuntime {
        PortableReadRuntime::new(RuntimePolicy {
            queue_deadline: Duration::from_secs(1),
            request_spacing: spacing,
            circuit_failure_threshold: threshold,
            circuit_cooldown: Duration::from_millis(50),
            maximum_endpoint_sessions: 4,
        })
        .unwrap()
    }

    fn endpoint(value: &str) -> EndpointIdentity {
        EndpointIdentity::new(value).unwrap()
    }

    #[tokio::test]
    async fn same_endpoint_serializes_while_distinct_endpoints_are_independent() {
        let runtime = test_runtime(Duration::ZERO, 3);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let same_max = Arc::new(AtomicUsize::new(0));
        let run = |runtime: PortableReadRuntime,
                   endpoint: EndpointIdentity,
                   in_flight: Arc<AtomicUsize>,
                   maximum: Arc<AtomicUsize>| async move {
            runtime
                .execute_read(
                    endpoint,
                    ReadOperation::CompanyList,
                    ReadRetryPolicy::SINGLE_ATTEMPT,
                    CancellationToken::new(),
                    |_| {
                        let in_flight = Arc::clone(&in_flight);
                        let maximum = Arc::clone(&maximum);
                        async move {
                            let active = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                            maximum.fetch_max(active, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            in_flight.fetch_sub(1, Ordering::SeqCst);
                            ReadAttempt::<_, ()>::Success {
                                value: (),
                                observed_body_bytes: BodyBytesObservation::Observed(10),
                            }
                        }
                    },
                )
                .await
        };
        let (first, second) = tokio::join!(
            run(
                runtime.clone(),
                endpoint("loopback-a"),
                Arc::clone(&in_flight),
                Arc::clone(&same_max)
            ),
            run(
                runtime.clone(),
                endpoint("loopback-a"),
                Arc::clone(&in_flight),
                Arc::clone(&same_max)
            )
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(same_max.load(Ordering::SeqCst), 1);

        let distinct_max = Arc::new(AtomicUsize::new(0));
        let (first, second) = tokio::join!(
            run(
                runtime.clone(),
                endpoint("loopback-a"),
                Arc::clone(&in_flight),
                Arc::clone(&distinct_max)
            ),
            run(
                runtime,
                endpoint("loopback-b"),
                in_flight,
                Arc::clone(&distinct_max)
            )
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(distinct_max.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn transient_reads_retry_exactly_but_validation_never_retries() {
        let runtime = test_runtime(Duration::ZERO, 100);
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&attempts);
        let result = runtime
            .execute_read(
                endpoint("retry-endpoint"),
                ReadOperation::VoucherExport,
                ReadRetryPolicy::new(3, Duration::ZERO, Duration::ZERO, 0).unwrap(),
                CancellationToken::new(),
                move |_| {
                    let observed = Arc::clone(&observed);
                    async move {
                        let attempt = observed.fetch_add(1, Ordering::SeqCst) + 1;
                        if attempt < 3 {
                            ReadAttempt::Failure {
                                error: "transient",
                                class: ReadFailureClass::RequestTimeout,
                                observed_body_bytes: BodyBytesObservation::Unavailable,
                            }
                        } else {
                            ReadAttempt::Success {
                                value: "ok",
                                observed_body_bytes: BodyBytesObservation::Observed(12),
                            }
                        }
                    }
                },
            )
            .await
            .unwrap();
        assert_eq!(result, "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 3);

        let attempts = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&attempts);
        let error = runtime
            .execute_read(
                endpoint("validation-endpoint"),
                ReadOperation::MasterExport,
                ReadRetryPolicy::new(3, Duration::ZERO, Duration::ZERO, 0).unwrap(),
                CancellationToken::new(),
                move |_| {
                    let observed = Arc::clone(&observed);
                    async move {
                        observed.fetch_add(1, Ordering::SeqCst);
                        ReadAttempt::<(), _>::Failure {
                            error: "company_mismatch",
                            class: ReadFailureClass::CompanyMismatch,
                            observed_body_bytes: BodyBytesObservation::Observed(100),
                        }
                    }
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ReadExecutionError::Attempt("company_mismatch")
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_is_terminal_and_preserves_follow_up_spacing() {
        let spacing = Duration::from_millis(60);
        let runtime = test_runtime(spacing, 3);
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        let running = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .execute_read(
                        endpoint("cancel-endpoint"),
                        ReadOperation::ReportExport,
                        ReadRetryPolicy::SINGLE_ATTEMPT,
                        cancellation,
                        |_| async { std::future::pending::<ReadAttempt<(), ()>>().await },
                    )
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();
        assert!(matches!(
            running.await.unwrap(),
            Err(ReadExecutionError::Cancelled)
        ));
        let started = Instant::now();
        runtime
            .execute_read(
                endpoint("cancel-endpoint"),
                ReadOperation::ReportExport,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                CancellationToken::new(),
                |_| async {
                    ReadAttempt::<_, ()>::Success {
                        value: (),
                        observed_body_bytes: BodyBytesObservation::Observed(0),
                    }
                },
            )
            .await
            .unwrap();
        assert!(started.elapsed() >= spacing.saturating_sub(Duration::from_millis(10)));
    }

    #[tokio::test]
    async fn circuit_cooldown_and_single_half_open_probe_are_enforced() {
        let runtime = test_runtime(Duration::ZERO, 1);
        let fail = || async {
            ReadAttempt::<(), _>::Failure {
                error: "offline",
                class: ReadFailureClass::Connection,
                observed_body_bytes: BodyBytesObservation::Unavailable,
            }
        };
        let _ = runtime
            .execute_read(
                endpoint("circuit-endpoint"),
                ReadOperation::CompanyList,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                CancellationToken::new(),
                |_| fail(),
            )
            .await;
        let rejected = runtime
            .execute_read(
                endpoint("circuit-endpoint"),
                ReadOperation::CompanyList,
                ReadRetryPolicy::SINGLE_ATTEMPT,
                CancellationToken::new(),
                |_| fail(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            rejected,
            ReadExecutionError::CircuitRejected {
                reason: CircuitRejectReason::Cooldown,
                ..
            }
        ));
        tokio::time::sleep(Duration::from_millis(60)).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let first = {
            let runtime = runtime.clone();
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            tokio::spawn(async move {
                runtime
                    .execute_read(
                        endpoint("circuit-endpoint"),
                        ReadOperation::CompanyList,
                        ReadRetryPolicy::SINGLE_ATTEMPT,
                        CancellationToken::new(),
                        |_| {
                            let entered = Arc::clone(&entered);
                            let release = Arc::clone(&release);
                            async move {
                                entered.notify_one();
                                release.notified().await;
                                ReadAttempt::<_, ()>::Success {
                                    value: (),
                                    observed_body_bytes: BodyBytesObservation::Observed(1),
                                }
                            }
                        },
                    )
                    .await
            })
        };
        entered.notified().await;
        let second = {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                runtime
                    .execute_read(
                        endpoint("circuit-endpoint"),
                        ReadOperation::CompanyList,
                        ReadRetryPolicy::SINGLE_ATTEMPT,
                        CancellationToken::new(),
                        |_| async {
                            ReadAttempt::<_, ()>::Success {
                                value: (),
                                observed_body_bytes: BodyBytesObservation::Observed(1),
                            }
                        },
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "a second probe must remain serialized"
        );
        release.notify_one();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn queued_request_cannot_use_a_stale_closed_circuit_admission() {
        let runtime = test_runtime(Duration::ZERO, 1);
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let first = {
            let runtime = runtime.clone();
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            tokio::spawn(async move {
                runtime
                    .execute_read(
                        endpoint("stale-admission-endpoint"),
                        ReadOperation::CompanyList,
                        ReadRetryPolicy::SINGLE_ATTEMPT,
                        CancellationToken::new(),
                        |_| {
                            let entered = Arc::clone(&entered);
                            let release = Arc::clone(&release);
                            async move {
                                entered.notify_one();
                                release.notified().await;
                                ReadAttempt::<(), _>::Failure {
                                    error: "offline",
                                    class: ReadFailureClass::Connection,
                                    observed_body_bytes: BodyBytesObservation::Unavailable,
                                }
                            }
                        },
                    )
                    .await
            })
        };
        entered.notified().await;
        let executed = Arc::new(AtomicUsize::new(0));
        let second = {
            let runtime = runtime.clone();
            let executed = Arc::clone(&executed);
            tokio::spawn(async move {
                runtime
                    .execute_read(
                        endpoint("stale-admission-endpoint"),
                        ReadOperation::CompanyList,
                        ReadRetryPolicy::SINGLE_ATTEMPT,
                        CancellationToken::new(),
                        move |_| {
                            let executed = Arc::clone(&executed);
                            async move {
                                executed.fetch_add(1, Ordering::SeqCst);
                                ReadAttempt::<_, ()>::Success {
                                    value: (),
                                    observed_body_bytes: BodyBytesObservation::Observed(1),
                                }
                            }
                        },
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        release.notify_one();
        assert!(matches!(
            first.await.unwrap(),
            Err(ReadExecutionError::Attempt("offline"))
        ));
        assert!(matches!(
            second.await.unwrap(),
            Err(ReadExecutionError::CircuitRejected {
                reason: CircuitRejectReason::Cooldown,
                ..
            })
        ));
        assert_eq!(executed.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn circuit_breaker_rejects_a_concurrent_half_open_permit() {
        let breaker = CircuitBreaker {
            state: Mutex::new(CircuitState {
                consecutive_failures: 1,
                last_failure_unix_ms: Some(now_unix_ms().saturating_sub(100)),
                half_open_probe_in_flight: false,
            }),
            threshold: 1,
            cooldown: Duration::from_millis(50),
        };
        let permit = breaker
            .admit(now_unix_ms())
            .expect("first half-open permit");
        assert!(matches!(
            breaker.admit(now_unix_ms()),
            Err((CircuitRejectReason::HalfOpenProbeInFlight, None))
        ));
        drop(permit);
        assert!(breaker.admit(now_unix_ms()).is_ok());
    }

    #[tokio::test]
    async fn deterministic_runtime_sequence_retries_server_failure_then_succeeds() {
        let runtime = test_runtime(Duration::ZERO, 3);
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&attempts);
        let xml = runtime
            .execute_read(
                endpoint("sequence-endpoint"),
                ReadOperation::ReportExport,
                ReadRetryPolicy::new(2, Duration::ZERO, Duration::ZERO, 0).unwrap(),
                CancellationToken::new(),
                move |_| {
                    let observed_attempts = Arc::clone(&observed_attempts);
                    async move {
                        if observed_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            ReadAttempt::Failure {
                                error: "http_server_failure",
                                class: ReadFailureClass::HttpServer,
                                observed_body_bytes: BodyBytesObservation::Observed(64),
                            }
                        } else {
                            ReadAttempt::Success {
                                value: "<STATUS>1</STATUS>",
                                observed_body_bytes: BodyBytesObservation::Observed(18),
                            }
                        }
                    }
                },
            )
            .await
            .expect("second deterministic response succeeds");
        assert!(xml.contains("<STATUS>1</STATUS>"));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn operation_and_retry_surface_are_read_only_and_redacted() {
        assert_eq!(ReadOperation::Status.request_class(), RequestClass::Status);
        assert!(ReadFailureClass::HttpServer.retryable());
        assert!(!ReadFailureClass::Application.retryable());
        assert!(format!("{:?}", endpoint("sensitive-loopback")).contains("[redacted]"));
        assert_eq!(
            ReadRetryPolicy::new(0, Duration::ZERO, Duration::ZERO, 0),
            Err(RuntimeConfigurationError::RetryPolicyInvalid)
        );
    }
}
