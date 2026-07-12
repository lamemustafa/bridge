use std::time::Duration;

pub fn backoff_for_attempt(attempt: u32) -> Option<Duration> {
    match attempt {
        0 => Some(Duration::from_secs(30)),
        1 => Some(Duration::from_secs(120)),
        2 => Some(Duration::from_secs(600)),
        _ => None,
    }
}
