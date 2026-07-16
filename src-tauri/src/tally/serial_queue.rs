use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex as StdMutex, OnceLock, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, MutexGuard};

#[derive(Default)]
struct GateState {
    next_request_not_before: Option<Instant>,
}

type Gate = Mutex<GateState>;

struct RequestSpacingGuard<'a> {
    state: MutexGuard<'a, GateState>,
    spacing: Duration,
}

impl Drop for RequestSpacingGuard<'_> {
    fn drop(&mut self) {
        self.state.next_request_not_before = Some(Instant::now() + self.spacing);
    }
}

fn endpoint_gates() -> &'static StdMutex<HashMap<String, Weak<Gate>>> {
    static GATES: OnceLock<StdMutex<HashMap<String, Weak<Gate>>>> = OnceLock::new();
    GATES.get_or_init(|| StdMutex::new(HashMap::new()))
}

#[derive(Clone)]
pub struct SerialTallyQueue {
    gate: Arc<Gate>,
    spacing: Duration,
    queue_deadline: Duration,
}

impl Default for SerialTallyQueue {
    fn default() -> Self {
        Self {
            gate: Arc::new(Mutex::new(GateState::default())),
            spacing: Duration::from_millis(500),
            queue_deadline: Duration::from_secs(30),
        }
    }
}

impl SerialTallyQueue {
    pub fn for_endpoint(endpoint_key: impl Into<String>) -> Self {
        let endpoint_key = endpoint_key.into();
        let mut gates = endpoint_gates()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        gates.retain(|_, gate| gate.strong_count() > 0);
        let gate = gates
            .get(&endpoint_key)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| {
                let gate = Arc::new(Mutex::new(GateState::default()));
                gates.insert(endpoint_key, Arc::downgrade(&gate));
                gate
            });
        Self {
            gate,
            spacing: Duration::from_millis(500),
            queue_deadline: Duration::from_secs(30),
        }
    }

    #[cfg(test)]
    fn for_endpoint_with_timing(
        endpoint_key: impl Into<String>,
        spacing: Duration,
        queue_deadline: Duration,
    ) -> Self {
        let mut queue = Self::for_endpoint(endpoint_key);
        queue.spacing = spacing;
        queue.queue_deadline = queue_deadline;
        queue
    }

    pub async fn run<F, Fut, T>(&self, request: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let queued_at = Instant::now();
        let state = tokio::time::timeout(self.queue_deadline, self.gate.lock())
            .await
            .map_err(|_| anyhow::anyhow!("Tally endpoint queue deadline exceeded"))?;
        if let Some(wait) = state
            .next_request_not_before
            .and_then(|not_before| not_before.checked_duration_since(Instant::now()))
        {
            let elapsed = queued_at.elapsed();
            let remaining = self
                .queue_deadline
                .checked_sub(elapsed)
                .ok_or_else(|| anyhow::anyhow!("Tally endpoint queue deadline exceeded"))?;
            tokio::time::timeout(remaining, tokio::time::sleep(wait))
                .await
                .map_err(|_| anyhow::anyhow!("Tally endpoint queue deadline exceeded"))?;
        }
        let _guard = RequestSpacingGuard {
            state,
            spacing: self.spacing,
        };
        let result = request().await;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::SerialTallyQueue;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn independently_created_queues_serialize_the_same_endpoint() {
        let first = SerialTallyQueue::for_endpoint("127.0.0.1:19000");
        let second = SerialTallyQueue::for_endpoint("127.0.0.1:19000");
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));

        let run = |queue: SerialTallyQueue,
                   in_flight: Arc<AtomicUsize>,
                   max_in_flight: Arc<AtomicUsize>| async move {
            queue
                .run(|| async move {
                    let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        };

        let (left, right) = tokio::join!(
            run(first, Arc::clone(&in_flight), Arc::clone(&max_in_flight)),
            run(second, in_flight, Arc::clone(&max_in_flight))
        );
        left.expect("first queue run");
        right.expect("second queue run");
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_endpoints_do_not_share_a_gate() {
        let first = SerialTallyQueue::for_endpoint("127.0.0.1:19001");
        let second = SerialTallyQueue::for_endpoint("127.0.0.1:19002");
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_in_flight = Arc::new(AtomicUsize::new(0));

        let run = |queue: SerialTallyQueue,
                   in_flight: Arc<AtomicUsize>,
                   max_in_flight: Arc<AtomicUsize>| async move {
            queue
                .run(|| async move {
                    let current = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_in_flight.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        };

        let (left, right) = tokio::join!(
            run(first, Arc::clone(&in_flight), Arc::clone(&max_in_flight)),
            run(second, in_flight, Arc::clone(&max_in_flight))
        );
        left.expect("first queue run");
        right.expect("second queue run");
        assert_eq!(max_in_flight.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn queue_wait_has_a_bounded_deadline() {
        let first = SerialTallyQueue::for_endpoint_with_timing(
            "127.0.0.1:19003",
            Duration::ZERO,
            Duration::from_secs(1),
        );
        let second = SerialTallyQueue::for_endpoint_with_timing(
            "127.0.0.1:19003",
            Duration::ZERO,
            Duration::from_millis(20),
        );
        let (acquired_tx, acquired_rx) = tokio::sync::oneshot::channel();
        let first_run = tokio::spawn(async move {
            first
                .run(|| async move {
                    let _ = acquired_tx.send(());
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(())
                })
                .await
        });
        acquired_rx
            .await
            .expect("first request acquired queue gate");
        let error = second
            .run(|| async { Ok(()) })
            .await
            .expect_err("second request should not wait indefinitely");
        assert!(error.to_string().contains("queue deadline"));
        first_run
            .await
            .expect("first queue task")
            .expect("first queue run");
    }

    #[tokio::test]
    async fn cancelling_an_in_flight_request_preserves_endpoint_spacing() {
        let spacing = Duration::from_millis(80);
        let first = SerialTallyQueue::for_endpoint_with_timing(
            "127.0.0.1:19004",
            spacing,
            Duration::from_secs(1),
        );
        let second = SerialTallyQueue::for_endpoint_with_timing(
            "127.0.0.1:19004",
            spacing,
            Duration::from_secs(1),
        );
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let first_run = tokio::spawn(async move {
            first
                .run(|| async move {
                    let _ = started_tx.send(());
                    std::future::pending::<anyhow::Result<()>>().await
                })
                .await
        });
        started_rx.await.expect("first request started");
        first_run.abort();
        assert!(first_run
            .await
            .expect_err("first request aborted")
            .is_cancelled());

        let cancelled_at = Instant::now();
        second
            .run(|| async { Ok(()) })
            .await
            .expect("follow-up request");
        assert!(
            cancelled_at.elapsed() >= spacing.saturating_sub(Duration::from_millis(10)),
            "follow-up must observe the cancellation quarantine"
        );
    }
}
