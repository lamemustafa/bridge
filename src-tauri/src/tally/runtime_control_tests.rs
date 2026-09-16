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
                        error: "validation_failed",
                        class: ReadFailureClass::Validation,
                        observed_body_bytes: BodyBytesObservation::Observed(100),
                    }
                }
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ReadExecutionError::Attempt("validation_failed")
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
