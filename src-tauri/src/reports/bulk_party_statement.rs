//! Writes one statement file per party from an already-complete local result.
//!
//! This module deliberately has no Tally transport dependency. The caller
//! supplies the rows obtained during the completed outstandings read.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use uuid::Uuid;

use super::party_statement::{build_party_statement_with_ageing_anchor, PartyStatement};
use crate::tally::{ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, UnallocatedParty};

const MAX_PENDING_DESTINATION_APPROVALS: usize = 8;
const DESTINATION_APPROVAL_TTL: Duration = Duration::from_secs(5 * 60);

/// Process-local authorizations issued only after the native folder picker
/// returns a folder. Each approval is scoped to one destination and is
/// consumed by the first export that presents it. Keeping a small, expiring
/// set lets independent picker flows coexist without making an old selection
/// reusable indefinitely.
#[derive(Default)]
pub struct PartyStatementDestinationApprovals {
    pending: Mutex<HashMap<String, PendingDestinationApproval>>,
}

struct PendingDestinationApproval {
    destination: PathBuf,
    issued_at: Instant,
}

/// Why an approval could not be issued or consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyStatementDestinationApprovalError {
    StoreUnavailable,
    CapacityReached,
    NotAuthorized,
}

/// A destination that the native picker approved for one bulk export.
///
/// Its field is private: only `PartyStatementDestinationApprovals::consume`
/// can turn a renderer-provided token and destination into a value accepted by
/// the bulk writer.
#[cfg_attr(
    not(feature = "live-calibration-harness"),
    doc = "```compile_fail\nuse bridge_lib::reports::bulk_party_statement::ApprovedPartyStatementDestination;\nlet _ = ApprovedPartyStatementDestination(std::path::PathBuf::from(\"/tmp\"));\n```"
)]
#[derive(Debug)]
pub struct ApprovedPartyStatementDestination(PathBuf);

impl PartyStatementDestinationApprovals {
    /// Records one successful picker choice and returns the opaque token that
    /// must accompany the subsequent export request.
    pub(crate) fn issue(
        &self,
        destination: PathBuf,
    ) -> Result<String, PartyStatementDestinationApprovalError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PartyStatementDestinationApprovalError::StoreUnavailable)?;
        discard_expired_approvals(&mut pending);
        if pending.len() >= MAX_PENDING_DESTINATION_APPROVALS {
            return Err(PartyStatementDestinationApprovalError::CapacityReached);
        }

        let approval_id = Uuid::new_v4().to_string();
        pending.insert(
            approval_id.clone(),
            PendingDestinationApproval {
                destination,
                issued_at: Instant::now(),
            },
        );
        Ok(approval_id)
    }

    /// Consumes a picker approval. A destination mismatch also consumes the
    /// token so an intercepted request cannot probe or reuse it.
    pub(crate) fn consume(
        &self,
        approval_id: &str,
        destination: &Path,
    ) -> Result<ApprovedPartyStatementDestination, PartyStatementDestinationApprovalError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PartyStatementDestinationApprovalError::StoreUnavailable)?;
        discard_expired_approvals(&mut pending);
        let Some(approval) = pending.remove(approval_id) else {
            return Err(PartyStatementDestinationApprovalError::NotAuthorized);
        };
        if approval.destination != destination {
            return Err(PartyStatementDestinationApprovalError::NotAuthorized);
        }
        Ok(ApprovedPartyStatementDestination(approval.destination))
    }

    /// Releases an approval that the renderer will not present for export.
    /// This is intentionally idempotent: an export may already have consumed
    /// the approval before its renderer-side cleanup runs.
    pub(crate) fn revoke(
        &self,
        approval_id: &str,
    ) -> Result<(), PartyStatementDestinationApprovalError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PartyStatementDestinationApprovalError::StoreUnavailable)?;
        discard_expired_approvals(&mut pending);
        pending.remove(approval_id);
        Ok(())
    }
}

impl ApprovedPartyStatementDestination {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

fn discard_expired_approvals(pending: &mut HashMap<String, PendingDestinationApproval>) {
    pending.retain(|_, approval| approval.issued_at.elapsed() < DESTINATION_APPROVAL_TTL);
}

#[derive(Debug, Clone, Serialize)]
pub struct BulkPartyStatementResult {
    pub destination: String,
    pub manifest_path: String,
    pub written: Vec<WrittenStatement>,
    pub failures: Vec<StatementFailure>,
}

/// All inputs needed to write a batch from a completed outstandings result.
///
/// Grouping the destination, report identity, selected ageing basis, and
/// renderer keeps the write contract cohesive as client-facing statement
/// metadata grows.
pub struct BulkPartyStatementRequest<'a, Render> {
    pub destination: &'a ApprovedPartyStatementDestination,
    pub company: &'a str,
    pub as_of_yyyymmdd: &'a str,
    pub format: &'a str,
    pub open_bills: &'a [OpenBillRow],
    pub unallocated_by_party: &'a [UnallocatedParty],
    pub ageing_anchor: OutstandingsAgeingAnchor,
    pub render: Render,
}

#[derive(Debug, Clone, Serialize)]
pub struct WrittenStatement {
    pub party: String,
    pub file_name: String,
    pub receivable_amount: String,
    pub payable_amount: String,
}

/// A stable, path-free category for why one party's statement did not make
/// it into the batch. Kept alongside `reason` so an operator (or anything
/// scripted against the manifest) can act on the failure without having to
/// parse free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementFailureCode {
    /// The statement's own figures could not be built or totalled for this
    /// party.
    Calculation,
    /// The statement could not be rendered into the requested file format.
    Rendering,
    /// The rendered statement could not be written to the destination
    /// folder.
    Write,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatementFailure {
    pub party: String,
    pub code: StatementFailureCode,
    /// A short, operator-facing reason. This is never the raw `Display` of
    /// an underlying IO error -- an IO error names the local file it failed
    /// on, and an absolute path (which can embed the operator's OS
    /// username) must never land in a manifest that may be handed to a
    /// client. Write failures log their full diagnostic, path included,
    /// through `tracing` for local troubleshooting only.
    pub reason: String,
}

#[derive(Serialize)]
struct StatementManifest<'a> {
    company: &'a str,
    as_of_yyyymmdd: &'a str,
    format: &'a str,
    ageing_anchor: OutstandingsAgeingAnchor,
    written: &'a [WrittenStatement],
    failures: &'a [StatementFailure],
}

/// Produces a separate file for every party represented in the complete rows.
///
/// Per-party failures are retained and reported after every remaining party is
/// attempted. That makes a partially completed batch explicit, while still
/// giving the operator usable files for parties whose statements rendered.
pub fn write_bulk_party_statements(
    destination: &ApprovedPartyStatementDestination,
    company: &str,
    as_of_yyyymmdd: &str,
    format: &str,
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
    render: impl Fn(&PartyStatement) -> Result<Vec<u8>, String>,
) -> Result<BulkPartyStatementResult, String> {
    write_bulk_party_statements_with_ageing_anchor(BulkPartyStatementRequest {
        destination,
        company,
        as_of_yyyymmdd,
        format,
        open_bills,
        unallocated_by_party,
        ageing_anchor: OutstandingsAgeingAnchor::DueDate,
        render,
    })
}

/// Writes statements while retaining the selected ageing basis in every file.
pub fn write_bulk_party_statements_with_ageing_anchor<Render>(
    request: BulkPartyStatementRequest<'_, Render>,
) -> Result<BulkPartyStatementResult, String>
where
    Render: for<'statement> Fn(&'statement PartyStatement) -> Result<Vec<u8>, String>,
{
    let BulkPartyStatementRequest {
        destination,
        company,
        as_of_yyyymmdd,
        format,
        open_bills,
        unallocated_by_party,
        ageing_anchor,
        render,
    } = request;
    let destination = destination.path();
    if !destination.is_dir() {
        return Err("Bridge could not use that statement destination folder.".to_string());
    }

    let parties = statement_parties(open_bills, unallocated_by_party);
    let mut written = Vec::with_capacity(parties.len());
    let mut failures = Vec::new();
    for party in parties {
        let statement = match build_party_statement_with_ageing_anchor(
            company,
            as_of_yyyymmdd,
            &party,
            open_bills,
            unallocated_by_party,
            ageing_anchor,
        ) {
            Ok(statement) => statement,
            Err(error) => {
                failures.push(StatementFailure {
                    party,
                    code: StatementFailureCode::Calculation,
                    reason: error.to_string(),
                });
                continue;
            }
        };
        let (receivable_amount, payable_amount) = match statement_directional_totals(&statement) {
            Ok(totals) => totals,
            Err(error) => {
                failures.push(StatementFailure {
                    party,
                    code: StatementFailureCode::Calculation,
                    reason: error,
                });
                continue;
            }
        };
        let bytes = match render(&statement) {
            Ok(bytes) => bytes,
            Err(error) => {
                failures.push(StatementFailure {
                    party,
                    code: StatementFailureCode::Rendering,
                    reason: error,
                });
                continue;
            }
        };
        let stem = format!(
            "statement-{}-{as_of_yyyymmdd}",
            safe_party_slug(&statement.party)
        );
        match write_unique_file(destination, &stem, format, &bytes) {
            Ok(path) => written.push(WrittenStatement {
                party,
                file_name: file_name(&path)?,
                receivable_amount,
                payable_amount,
            }),
            Err(error) => failures.push(StatementFailure {
                party,
                code: StatementFailureCode::Write,
                reason: error,
            }),
        }
    }

    let manifest = StatementManifest {
        company,
        as_of_yyyymmdd,
        format,
        ageing_anchor,
        written: &written,
        failures: &failures,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("Bridge could not build the statement manifest: {error}"))?;
    let manifest_path = write_unique_file(
        destination,
        &format!("statement-manifest-{as_of_yyyymmdd}"),
        "json",
        &manifest_bytes,
    )?;

    Ok(BulkPartyStatementResult {
        destination: destination.to_string_lossy().into_owned(),
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        written,
        failures,
    })
}

fn statement_directional_totals(statement: &PartyStatement) -> Result<(String, String), String> {
    let mut receivable = bridge_tally_core::ExactDecimal::zero();
    let mut payable = bridge_tally_core::ExactDecimal::zero();
    for bill in &statement.bills {
        let total = match bill.kind {
            ExposureDirection::Receivable => &mut receivable,
            ExposureDirection::Payable => &mut payable,
        };
        *total = total
            .checked_add(&bill.amount)
            .map_err(|_| "Bridge could not total a statement direction exactly.".to_string())?;
    }
    if !statement.unallocated.is_zero() {
        let total = match statement.unallocated_direction {
            Some(ExposureDirection::Receivable) => &mut receivable,
            Some(ExposureDirection::Payable) => &mut payable,
            None => {
                return Err("Bridge found an unallocated amount without a direction.".to_string())
            }
        };
        *total = total
            .checked_add(&statement.unallocated)
            .map_err(|_| "Bridge could not total a statement direction exactly.".to_string())?;
    }
    Ok((
        receivable.as_str().to_string(),
        payable.as_str().to_string(),
    ))
}

fn statement_parties(
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
) -> BTreeSet<String> {
    open_bills
        .iter()
        .filter(|row| !row.amount.is_zero())
        .map(|row| row.party.clone())
        .chain(
            unallocated_by_party
                .iter()
                .filter(|entry| !entry.amount.is_zero())
                .map(|entry| entry.party.clone()),
        )
        .collect()
}

/// Counts the non-zero, exact-name parties that a bulk statement would cover.
/// This is shared by the operator preview and the writer so their scopes
/// cannot diverge.
pub fn bulk_party_statement_party_count(
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
) -> usize {
    statement_parties(open_bills, unallocated_by_party).len()
}

/// Converts arbitrary ledger text to a portable ASCII filename component.
/// Separators, controls, Windows-reserved punctuation, leading dots, trailing
/// spaces, and non-ASCII characters all become collapsed hyphens.
fn safe_party_slug(party: &str) -> String {
    let mut slug = String::with_capacity(party.len());
    let mut previous_was_dash = false;
    for ch in party.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_was_dash = false;
        } else if !previous_was_dash {
            slug.push('-');
            previous_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "party".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Creates a previously unused filename. `create_new` closes the race between
/// candidate selection and writing, so neither a same-run slug collision nor
/// a pre-existing file can be silently overwritten.
fn write_unique_file(
    destination: &Path,
    stem: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    if Path::new(stem).components().count() != 1 {
        return Err("Bridge could not build a safe statement filename.".to_string());
    }
    for sequence in 1..=10_000_u32 {
        let suffix = if sequence == 1 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let path = destination.join(format!("{stem}{suffix}.{extension}"));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(unwritable_destination_reason(&path, &error)),
        };
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(unwritable_destination_reason(&path, &error));
        }
        return Ok(path);
    }
    Err("Bridge could not find an unused statement filename after 10,000 attempts.".to_string())
}

/// Logs the full write diagnostic -- including the local filesystem path,
/// which can embed the operator's OS username -- to Bridge's own log, then
/// returns a short reason known not to contain a path. This is the only
/// place a per-file write failure is described, so nothing upstream needs to
/// remember to scrub it before it can reach an exported manifest.
fn unwritable_destination_reason(path: &Path, error: &std::io::Error) -> String {
    tracing::warn!(path = %path.display(), %error, "statement file write failed");
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "Bridge does not have permission to write to the selected folder.".to_string()
        }
        std::io::ErrorKind::NotFound => {
            "Bridge could not find the selected folder any more.".to_string()
        }
        _ => "Bridge could not write the statement file to the selected folder.".to_string(),
    }
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| "Bridge could not represent the statement filename.".to_string())
}

#[cfg(test)]
#[path = "bulk_party_statement_tests.rs"]
mod tests;
