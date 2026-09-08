//! Local, source-linked draft preparation. This is deliberately disconnected
//! from Tally transport, posting, approvals, and saved Journal batches.

#[path = "source_draft/files.rs"]
mod files;
#[path = "source_draft/types.rs"]
mod types;

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use tauri::State;
use uuid::Uuid;

use crate::source_draft_xml::{parse_source_xml, ParsedSource, MAX_SOURCE_BYTES};

use self::{
    files::{open_saved_draft, pick_file, save_path, serialize_draft, write_private_file},
    types::{
        command_error, error, CommandResult, SourceDraftDto, SourceDraftEntry,
        SourceDraftEntryProposal, SourceDraftProposal, SourceDraftRow, SourceDraftSaveRequest,
        SourceDraftSourceNotice, MAX_PROPOSAL_BYTES, MAX_TEXT_BYTES,
    },
};

#[derive(Default)]
pub(crate) struct SourceDraftStore {
    active: Arc<Mutex<Option<ActiveDraft>>>,
}

#[derive(Clone)]
struct ActiveDraft {
    id: Uuid,
    revision: u64,
    source: ParsedSource,
    proposals: Vec<SourceDraftProposal>,
}

#[tauri::command]
pub(crate) async fn desktop_pick_source_draft(
    store: State<'_, SourceDraftStore>,
) -> CommandResult<Option<SourceDraftDto>> {
    let Some((filename, bytes)) =
        pick_file("Choose source XML", &["xml"], MAX_SOURCE_BYTES).await?
    else {
        return Ok(None);
    };
    if !filename.to_ascii_lowercase().ends_with(".xml") {
        return Err(error("source_draft_extension_invalid"));
    }
    let source = tokio::task::spawn_blocking(move || {
        parse_source_xml(&bytes, filename).map_err(command_error)
    })
    .await
    .map_err(|_| error("source_draft_source_parse_failed"))??;
    let active = ActiveDraft {
        id: Uuid::new_v4(),
        revision: 1,
        proposals: empty_proposals(&source),
        source,
    };
    store.replace(active).map(Some)
}

#[tauri::command]
pub(crate) async fn desktop_open_source_draft(
    store: State<'_, SourceDraftStore>,
) -> CommandResult<Option<SourceDraftDto>> {
    let Some((filename, bytes)) = pick_file(
        "Open Bridge source draft",
        &["bridge-draft.json"],
        types::MAX_DRAFT_BYTES,
    )
    .await?
    else {
        return Ok(None);
    };
    if !filename.ends_with(".bridge-draft.json") {
        return Err(error("source_draft_extension_invalid"));
    }
    let (source, proposals) = tokio::task::spawn_blocking(move || open_saved_draft(bytes))
        .await
        .map_err(|_| error("source_draft_saved_json_invalid"))??;
    validate_proposals(&source, &proposals)?;
    store
        .replace(ActiveDraft {
            id: Uuid::new_v4(),
            revision: 1,
            source,
            proposals,
        })
        .map(Some)
}

#[tauri::command]
pub(crate) async fn desktop_save_source_draft(
    store: State<'_, SourceDraftStore>,
    request: SourceDraftSaveRequest,
) -> CommandResult<Option<SourceDraftDto>> {
    let snapshot = store.snapshot(&request)?;
    validate_proposals(&snapshot.source, &request.proposals)?;
    let Some(path) = save_path().await? else {
        return Ok(None);
    };
    // The dialog is an asynchronous boundary: re-check before any persistent write.
    store.snapshot(&request)?;
    let active = Arc::clone(&store.active);
    tokio::task::spawn_blocking(move || commit_after_persist(&active, request, path))
        .await
        .map_err(|_| error("source_draft_state_unavailable"))?
        .map(Some)
}

impl SourceDraftStore {
    fn replace(&self, active: ActiveDraft) -> CommandResult<SourceDraftDto> {
        let dto = dto(&active);
        *self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))? = Some(active);
        Ok(dto)
    }

    fn snapshot(&self, request: &SourceDraftSaveRequest) -> CommandResult<ActiveDraft> {
        let id = Uuid::parse_str(&request.draft_id)
            .map_err(|_| error("source_draft_identifier_invalid"))?;
        let active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?
            .clone()
            .ok_or_else(|| error("source_draft_not_active"))?;
        if active.id != id || active.revision != request.revision {
            return Err(error("source_draft_revision_conflict"));
        }
        Ok(active)
    }
}

fn dto(active: &ActiveDraft) -> SourceDraftDto {
    SourceDraftDto {
        draft_id: active.id.to_string(),
        revision: active.revision,
        source_filename: active.source.filename.clone(),
        source_sha256: active.source.sha256.clone(),
        source_notices: active
            .source
            .source_notices
            .iter()
            .map(|notice| SourceDraftSourceNotice {
                kind: notice.kind.clone(),
                count: notice.count,
            })
            .collect(),
        rows: active
            .source
            .vouchers
            .iter()
            .zip(&active.proposals)
            .map(|(source, proposal)| SourceDraftRow {
                position: source.position,
                source_remote_id: source.remote_id.clone(),
                source_date: source.date.clone(),
                source_voucher_type: source.voucher_type.clone(),
                source_narration: source.narration.clone(),
                entries: source
                    .entries
                    .iter()
                    .map(|entry| SourceDraftEntry {
                        position: entry.position,
                        source_ledger: entry.ledger.clone(),
                        source_amount: entry.amount.clone(),
                        source_polarity: entry.polarity.clone(),
                    })
                    .collect(),
                source_omitted_fields: source.omitted_fields.clone(),
                proposal: proposal.clone(),
            })
            .collect(),
    }
}

fn empty_proposals(source: &ParsedSource) -> Vec<SourceDraftProposal> {
    source
        .vouchers
        .iter()
        .map(|voucher| SourceDraftProposal {
            date: None,
            voucher_type: None,
            narration: None,
            notes: String::new(),
            entries: voucher
                .entries
                .iter()
                .map(|_| SourceDraftEntryProposal {
                    ledger: None,
                    side: None,
                    amount: None,
                })
                .collect(),
        })
        .collect()
}

fn validate_proposals(
    source: &ParsedSource,
    proposals: &[SourceDraftProposal],
) -> CommandResult<()> {
    if proposals.len() != source.vouchers.len() {
        return Err(error("source_draft_proposal_shape_invalid"));
    }
    let mut total = 0usize;
    for (source_row, proposal) in source.vouchers.iter().zip(proposals) {
        if proposal.entries.len() != source_row.entries.len() {
            return Err(error("source_draft_proposal_shape_invalid"));
        }
        validate_optional_date(proposal.date.as_deref())?;
        for value in proposal
            .narration
            .iter()
            .chain(std::iter::once(&proposal.notes))
            .chain(
                proposal
                    .entries
                    .iter()
                    .flat_map(|entry| entry.ledger.iter().chain(entry.amount.iter())),
            )
        {
            if value.len() > MAX_TEXT_BYTES {
                return Err(error("source_draft_proposal_text_too_large"));
            }
            total = total
                .checked_add(value.len())
                .ok_or_else(|| error("source_draft_proposal_text_too_large"))?;
        }
    }
    if total > MAX_PROPOSAL_BYTES {
        Err(error("source_draft_proposal_too_large"))
    } else {
        Ok(())
    }
}

fn validate_optional_date(value: Option<&str>) -> CommandResult<()> {
    if value.is_some_and(|date| {
        date.len() != 8
            || !date.bytes().all(|b| b.is_ascii_digit())
            || NaiveDate::parse_from_str(date, "%Y%m%d")
                .map(|parsed| parsed.format("%Y%m%d").to_string() != date)
                .unwrap_or(true)
    }) {
        Err(error("source_draft_proposal_date_invalid"))
    } else {
        Ok(())
    }
}

fn commit_after_persist(
    active: &Arc<Mutex<Option<ActiveDraft>>>,
    request: SourceDraftSaveRequest,
    path: std::path::PathBuf,
) -> CommandResult<SourceDraftDto> {
    let mut guard = active
        .lock()
        .map_err(|_| error("source_draft_state_unavailable"))?;
    let current = guard
        .as_ref()
        .ok_or_else(|| error("source_draft_not_active"))?;
    if current.id.to_string() != request.draft_id || current.revision != request.revision {
        return Err(error("source_draft_revision_conflict"));
    }
    validate_proposals(&current.source, &request.proposals)?;
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| error("source_draft_revision_exhausted"))?;
    let bytes = serialize_draft(&current.source, &request.proposals)?;
    write_private_file(&path, &bytes)?;
    let current = guard.as_mut().expect("active draft checked");
    current.proposals = request.proposals;
    current.revision = next_revision;
    Ok(dto(current))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn source() -> ParsedSource {
        parse_source_xml(b"<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE><VOUCHER REMOTEID=\"x\" VCHTYPE=\"Receipt\"><DATE>20260901</DATE><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>1</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>", "source.xml".into()).unwrap()
    }

    #[test]
    fn fresh_proposal_preserves_unknown_accounting_choices() {
        let source = source();
        let proposal = empty_proposals(&source);
        assert!(proposal[0].date.is_none());
        assert!(proposal[0].entries[0].side.is_none());
        assert!(proposal[0].notes.is_empty());
    }

    #[test]
    fn saved_schema_rejects_unknown_fields_and_shape_drift() {
        let source = source();
        let proposal = empty_proposals(&source);
        let bytes = serialize_draft(&source, &proposal).unwrap();
        assert!(serde_json::from_slice::<files::StoredDraft>(&bytes).is_ok());
        assert!(
            serde_json::from_slice::<files::StoredDraft>(br#"{\"version\":1,\"extra\":true}"#)
                .is_err()
        );
        assert!(validate_proposals(&source, &[]).is_err());
    }

    #[test]
    fn saved_draft_reauthenticates_source_hash_and_preserves_unresolved_proposals() {
        let source = source();
        let proposals = empty_proposals(&source);
        let mut saved: serde_json::Value =
            serde_json::from_slice(&serialize_draft(&source, &proposals).unwrap()).unwrap();
        assert!(open_saved_draft(serde_json::to_vec(&saved).unwrap()).is_ok());
        saved["source_sha256"] = serde_json::Value::String("0".repeat(64));
        assert_eq!(
            open_saved_draft(serde_json::to_vec(&saved).unwrap())
                .unwrap_err()
                .code,
            "source_draft_saved_source_hash_mismatch"
        );
    }

    #[test]
    fn stale_identifier_revision_and_invalid_calendar_dates_refuse_before_save() {
        let store = SourceDraftStore::default();
        let source_data = source();
        let active = ActiveDraft {
            id: Uuid::new_v4(),
            revision: 1,
            proposals: empty_proposals(&source_data),
            source: source_data,
        };
        let dto = store.replace(active).unwrap();
        let stale = SourceDraftSaveRequest {
            draft_id: dto.draft_id.clone(),
            revision: 0,
            proposals: dto.rows.iter().map(|row| row.proposal.clone()).collect(),
        };
        assert!(matches!(
            store.snapshot(&stale),
            Err(types::SourceDraftCommandError {
                code: "source_draft_revision_conflict",
                ..
            })
        ));
        let invalid = SourceDraftProposal {
            date: Some("20269999".into()),
            voucher_type: None,
            narration: None,
            notes: String::new(),
            entries: vec![SourceDraftEntryProposal {
                ledger: None,
                side: None,
                amount: None,
            }],
        };
        assert_eq!(
            validate_proposals(&source(), &[invalid]).unwrap_err().code,
            "source_draft_proposal_date_invalid"
        );
    }

    #[test]
    fn regular_draft_overwrite_is_atomic_and_xml_filename_is_not_a_draft_destination() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("draft.bridge-draft.json");
        fs::write(&path, b"old").unwrap();
        write_private_file(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert!(!"original.xml".ends_with(".bridge-draft.json"));
    }
}
