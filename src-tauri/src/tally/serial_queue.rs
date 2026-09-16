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
#[path = "serial_queue_tests.rs"]
mod tests;
