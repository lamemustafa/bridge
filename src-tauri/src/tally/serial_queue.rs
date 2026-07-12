use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct SerialTallyQueue {
    gate: Arc<Mutex<()>>,
    spacing: Duration,
}

impl Default for SerialTallyQueue {
    fn default() -> Self {
        Self {
            gate: Arc::new(Mutex::new(())),
            spacing: Duration::from_millis(500),
        }
    }
}

impl SerialTallyQueue {
    pub async fn run<F, Fut, T>(&self, request: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let _guard = self.gate.lock().await;
        let result = request().await;
        tokio::time::sleep(self.spacing).await;
        result
    }
}
