//! Short-lived, one-use bindings between a completed native read and its
//! working-paper export. The webview receives only an opaque identifier.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use uuid::Uuid;

use super::outstandings_working_paper::OutstandingsWorkingPaperSource;
use crate::tally::OutstandingsLoadResult;

const EXPORT_HANDLE_TTL: Duration = Duration::from_secs(15 * 60);
/// A statement source lives as long as a working day at the screen: the
/// operator sends statements one party at a time from the result on screen
/// (bridge#551). Any refresh of the company revokes it sooner.
const STATEMENT_SOURCE_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_STORED_EXPORTS: usize = 4;
const MAX_BILL_ROWS: usize = 200_000;
const MAX_UNALLOCATED_ROWS: usize = 100_000;
const MAX_SOURCE_BYTES: usize = 128 * 1024 * 1024;
const MAX_CELL_TEXT_BYTES: usize = 4_096;
const MAX_AGGREGATE_TEXT_BYTES: usize = 64 * 1024 * 1024;

struct StoredExport {
    id: String,
    expires_at: Instant,
    revocation_key: String,
    source: Arc<OutstandingsWorkingPaperSource>,
}

pub struct WorkingPaperExportStore {
    entries: Mutex<VecDeque<StoredExport>>,
    ttl: Duration,
}

impl Default for WorkingPaperExportStore {
    fn default() -> Self {
        Self::with_ttl(EXPORT_HANDLE_TTL)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkingPaperExportStoreError {
    #[error("working-paper source exceeds the bounded export budget")]
    ResourceLimit,
    #[error("working-paper export approval is invalid or expired")]
    InvalidOrExpired,
    #[error("working-paper export store is unavailable")]
    Unavailable,
}

impl WorkingPaperExportStore {
    fn with_ttl(ttl: Duration) -> Self {
        Self {
            entries: Mutex::default(),
            ttl,
        }
    }

    /// Replaces every older approval for `company_revocation_key` with the source from
    /// the newest completed read. Passing `None` still revokes the older
    /// approval: a partial or otherwise ineligible refresh must not leave a
    /// now-hidden snapshot exportable by a stale webview capability.
    pub fn replace_for_company(
        &self,
        company_revocation_key: &str,
        source: Option<OutstandingsWorkingPaperSource>,
    ) -> Result<Option<String>, WorkingPaperExportStoreError> {
        self.replace_shared_for_company(company_revocation_key, source.map(Arc::new))
    }

    /// As [`Self::replace_for_company`], holding a source another store also
    /// holds, so one completed read is kept once in memory.
    pub fn replace_shared_for_company(
        &self,
        company_revocation_key: &str,
        source: Option<Arc<OutstandingsWorkingPaperSource>>,
    ) -> Result<Option<String>, WorkingPaperExportStoreError> {
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| WorkingPaperExportStoreError::Unavailable)?;
        let now = Instant::now();
        entries.retain(|entry| {
            entry.expires_at > now && entry.revocation_key != company_revocation_key
        });
        let Some(source) = source else {
            return Ok(None);
        };
        while entries.len() >= MAX_STORED_EXPORTS {
            entries.pop_front();
        }
        let id = Uuid::new_v4().to_string();
        entries.push_back(StoredExport {
            id: id.clone(),
            expires_at: now + self.ttl,
            revocation_key: company_revocation_key.to_string(),
            source,
        });
        Ok(Some(id))
    }

    pub fn take(
        &self,
        id: &str,
    ) -> Result<OutstandingsWorkingPaperSource, WorkingPaperExportStoreError> {
        self.with_live_entry(id, |entries, position| {
            entries.remove(position).map(|entry| entry.source)
        })
        .map(|source| Arc::try_unwrap(source).unwrap_or_else(|shared| (*shared).clone()))
    }

    /// The source behind `id`, left in place: every party's statement comes
    /// from the same read (bridge#551).
    pub fn get(
        &self,
        id: &str,
    ) -> Result<Arc<OutstandingsWorkingPaperSource>, WorkingPaperExportStoreError> {
        self.with_live_entry(id, |entries, position| {
            entries.get(position).map(|entry| Arc::clone(&entry.source))
        })
    }

    fn with_live_entry(
        &self,
        id: &str,
        read: impl FnOnce(
            &mut VecDeque<StoredExport>,
            usize,
        ) -> Option<Arc<OutstandingsWorkingPaperSource>>,
    ) -> Result<Arc<OutstandingsWorkingPaperSource>, WorkingPaperExportStoreError> {
        if id.len() > 64 || Uuid::parse_str(id).is_err() {
            return Err(WorkingPaperExportStoreError::InvalidOrExpired);
        }
        let mut entries = self
            .entries
            .lock()
            .map_err(|_| WorkingPaperExportStoreError::Unavailable)?;
        let now = Instant::now();
        entries.retain(|entry| entry.expires_at > now);
        let position = entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(WorkingPaperExportStoreError::InvalidOrExpired)?;
        read(&mut entries, position).ok_or(WorkingPaperExportStoreError::InvalidOrExpired)
    }
}

/// The server-held source of party statements (bridge#551): the same
/// completed read as the working paper, under its own handle so that a
/// working-paper export, which consumes its handle, never takes the
/// statements with it. The webview sends the handle, never the rows. A
/// handle lives [`STATEMENT_SOURCE_TTL`] and is revoked by the company's next
/// completed refresh.
pub struct PartyStatementSourceStore(WorkingPaperExportStore);

impl Default for PartyStatementSourceStore {
    fn default() -> Self {
        Self(WorkingPaperExportStore::with_ttl(STATEMENT_SOURCE_TTL))
    }
}

impl PartyStatementSourceStore {
    #[cfg(test)]
    pub(crate) fn with_ttl(ttl: Duration) -> Self {
        Self(WorkingPaperExportStore::with_ttl(ttl))
    }

    pub fn replace_for_company(
        &self,
        company_revocation_key: &str,
        source: Option<Arc<OutstandingsWorkingPaperSource>>,
    ) -> Result<Option<String>, WorkingPaperExportStoreError> {
        self.0
            .replace_shared_for_company(company_revocation_key, source)
    }

    pub fn get(
        &self,
        id: &str,
    ) -> Result<Arc<OutstandingsWorkingPaperSource>, WorkingPaperExportStoreError> {
        self.0.get(id)
    }
}

/// Copies a working-paper source only after the completed result passes
/// cardinality, source-byte, and text-size budgets. `None` is the legacy path,
/// whose missing unallocated control cannot substantiate an all-party paper.
pub fn source_from_complete_result(
    result: &OutstandingsLoadResult,
    company_guid: &str,
) -> Result<Option<OutstandingsWorkingPaperSource>, WorkingPaperExportStoreError> {
    let OutstandingsLoadResult::Complete {
        report,
        currency_assertion,
        ageing_anchor,
        synced_at_unix_ms,
        unallocated_total,
        statement_unallocated_by_party,
        statement_open_bills,
        ..
    } = result
    else {
        return Ok(None);
    };
    let Some(unallocated_total) = unallocated_total else {
        return Ok(None);
    };
    if statement_open_bills.len() > MAX_BILL_ROWS
        || statement_unallocated_by_party.len() > MAX_UNALLOCATED_ROWS
        || report.source_bytes > MAX_SOURCE_BYTES
        || company_guid.len() > MAX_CELL_TEXT_BYTES
        || report.company_name.len() > MAX_CELL_TEXT_BYTES
    {
        return Err(WorkingPaperExportStoreError::ResourceLimit);
    }
    let mut aggregate_text_bytes = company_guid
        .len()
        .checked_add(report.company_name.len())
        .ok_or(WorkingPaperExportStoreError::ResourceLimit)?;
    for row in statement_open_bills {
        if row.party.len() > MAX_CELL_TEXT_BYTES || row.reference.len() > MAX_CELL_TEXT_BYTES {
            return Err(WorkingPaperExportStoreError::ResourceLimit);
        }
        aggregate_text_bytes = aggregate_text_bytes
            .checked_add(row.party.len())
            .and_then(|value| value.checked_add(row.reference.len()))
            .ok_or(WorkingPaperExportStoreError::ResourceLimit)?;
    }
    for row in statement_unallocated_by_party {
        if row.party.len() > MAX_CELL_TEXT_BYTES {
            return Err(WorkingPaperExportStoreError::ResourceLimit);
        }
        aggregate_text_bytes = aggregate_text_bytes
            .checked_add(row.party.len())
            .ok_or(WorkingPaperExportStoreError::ResourceLimit)?;
    }
    if aggregate_text_bytes > MAX_AGGREGATE_TEXT_BYTES {
        return Err(WorkingPaperExportStoreError::ResourceLimit);
    }

    Ok(Some(OutstandingsWorkingPaperSource {
        company: report.company_name.clone(),
        company_guid: company_guid.to_string(),
        as_of_yyyymmdd: report.as_of_yyyymmdd.clone(),
        currency_assertion: *currency_assertion,
        synced_at_unix_ms: *synced_at_unix_ms,
        source_bytes: report.source_bytes,
        source_ageing_anchor: *ageing_anchor,
        receivable_bill_total: report.receivable_total.clone(),
        payable_bill_total: report.payable_total.clone(),
        unallocated_total: unallocated_total.clone(),
        open_bills: statement_open_bills.clone(),
        unallocated_by_party: statement_unallocated_by_party.clone(),
    }))
}

#[cfg(test)]
#[path = "outstandings_working_paper_store_tests.rs"]
mod tests;
