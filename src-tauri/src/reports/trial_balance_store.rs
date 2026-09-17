//! Bounded backend-owned bindings for completed Trial Balance export sources.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::tally::runtime::TrialBalanceRead;

const TTL: Duration = Duration::from_secs(15 * 60);
const MAX_ROWS: usize = 200_000;
const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CELL_TEXT_BYTES: usize = 4_096;
const MAX_TEXT_BYTES: usize = 64 * 1024 * 1024;

struct StoredRead {
    id: String,
    expires_at: Instant,
    read: Arc<TrialBalanceRead>,
}

pub struct StoredTrialBalanceCapture {
    pub read: Arc<TrialBalanceRead>,
    pub expires_in: Duration,
}

#[derive(Default)]
pub struct TrialBalanceExportStore {
    entry: Mutex<Option<StoredRead>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrialBalanceExportStoreError {
    #[error("Trial Balance source exceeds the bounded export budget")]
    ResourceLimit,
    #[error("Trial Balance export handle is invalid or expired")]
    InvalidOrExpired,
    #[error("Trial Balance export store is unavailable")]
    Unavailable,
}

impl TrialBalanceExportStore {
    pub fn insert(&self, read: TrialBalanceRead) -> Result<String, TrialBalanceExportStoreError> {
        validate(&read)?;
        let id = Uuid::new_v4().to_string();
        *self
            .entry
            .lock()
            .map_err(|_| TrialBalanceExportStoreError::Unavailable)? = Some(StoredRead {
            id: id.clone(),
            expires_at: Instant::now() + TTL,
            read: Arc::new(read),
        });
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Result<Arc<TrialBalanceRead>, TrialBalanceExportStoreError> {
        self.get_capture(id).map(|capture| capture.read)
    }

    /// Returns only the backend-retained capture bound to an opaque handle.
    /// The remaining lifetime lets read-only follow-ups disclose their expiry.
    pub fn get_capture(
        &self,
        id: &str,
    ) -> Result<StoredTrialBalanceCapture, TrialBalanceExportStoreError> {
        if id.len() > 64 || Uuid::parse_str(id).is_err() {
            return Err(TrialBalanceExportStoreError::InvalidOrExpired);
        }
        let mut entry = self
            .entry
            .lock()
            .map_err(|_| TrialBalanceExportStoreError::Unavailable)?;
        let Some(stored) = entry.as_ref() else {
            return Err(TrialBalanceExportStoreError::InvalidOrExpired);
        };
        if stored.expires_at <= Instant::now() {
            *entry = None;
            return Err(TrialBalanceExportStoreError::InvalidOrExpired);
        }
        if stored.id != id {
            return Err(TrialBalanceExportStoreError::InvalidOrExpired);
        }
        Ok(StoredTrialBalanceCapture {
            read: Arc::clone(&stored.read),
            expires_in: stored.expires_at.saturating_duration_since(Instant::now()),
        })
    }

    pub fn clear(&self) -> Result<(), TrialBalanceExportStoreError> {
        *self
            .entry
            .lock()
            .map_err(|_| TrialBalanceExportStoreError::Unavailable)? = None;
        Ok(())
    }
}

fn validate(read: &TrialBalanceRead) -> Result<(), TrialBalanceExportStoreError> {
    if read.report.rows.len() > MAX_ROWS || read.evidence.bytes > MAX_SOURCE_BYTES {
        return Err(TrialBalanceExportStoreError::ResourceLimit);
    }
    let mut bytes = read.company_guid.len()
        + read.company_name.len()
        + read.read_at.len()
        + read.from.as_str().len()
        + read.to.as_str().len()
        + read.currency.mailing_name.len()
        + read.currency.symbol.len()
        + read.evidence.request_sha256.len()
        + read.evidence.response_sha256.len();
    for text in [
        &read.company_guid,
        &read.company_name,
        &read.read_at,
        read.from.as_str(),
        read.to.as_str(),
        &read.currency.mailing_name,
        &read.currency.symbol,
        &read.evidence.request_sha256,
        &read.evidence.response_sha256,
    ] {
        if text.len() > MAX_CELL_TEXT_BYTES {
            return Err(TrialBalanceExportStoreError::ResourceLimit);
        }
    }
    for row in &read.report.rows {
        for text in [&row.name, &row.guid, row.parent.workbook_text()] {
            if text.len() > MAX_CELL_TEXT_BYTES {
                return Err(TrialBalanceExportStoreError::ResourceLimit);
            }
            bytes = bytes
                .checked_add(text.len())
                .ok_or(TrialBalanceExportStoreError::ResourceLimit)?;
        }
    }
    if bytes > MAX_TEXT_BYTES {
        return Err(TrialBalanceExportStoreError::ResourceLimit);
    }
    Ok(())
}

#[cfg(test)]
#[path = "trial_balance_store_tests.rs"]
mod tests;
