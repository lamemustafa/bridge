//! Local, source-linked draft preparation. This is deliberately disconnected
//! from Tally transport, posting, approvals, and saved Journal batches.

#[path = "source_draft/catalog.rs"]
mod catalog;
#[path = "source_draft/files.rs"]
mod files;
#[path = "source_draft/lifecycle.rs"]
mod lifecycle;
#[path = "source_draft/types.rs"]
mod types;

pub(crate) use lifecycle::{
    SourceDraftLifecycleGuard, SourceDraftLifecycleKind, SourceDraftLifecycleRequest,
};

use std::sync::{Arc, Mutex};

use chrono::NaiveDate;
use tauri::{Manager, State};
use uuid::Uuid;

use crate::source_draft_xml::{parse_source_xml, ParsedSource, MAX_SOURCE_BYTES};

use self::{
    files::{open_saved_draft, pick_file, save_path, serialize_draft, write_private_file},
    types::{
        command_error, error, CommandResult, SourceDraftCurrentCatalogBinding, SourceDraftDto,
        SourceDraftEntry, SourceDraftEntryProposal, SourceDraftProposal, SourceDraftRow,
        SourceDraftSaveRequest, SourceDraftSourceNotice, MAX_PROPOSAL_BYTES, MAX_TEXT_BYTES,
    },
};
use catalog::{
    CatalogCapture, SourceDraftCatalogApplyRequest, SourceDraftCatalogInvalidateRequest,
    SourceDraftCatalogLoadRequest, SourceDraftCatalogTargets,
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
    catalog_generation: u64,
    catalog: Option<CatalogCapture>,
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
        catalog_generation: 0,
        catalog: None,
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
        &["json"],
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
            catalog_generation: 0,
            catalog: None,
        })
        .map(Some)
}

#[tauri::command]
pub(crate) async fn desktop_load_source_draft_existing_ledger_targets(
    store: State<'_, SourceDraftStore>,
    runtime: State<'_, crate::tally::TallyRuntime>,
    request: SourceDraftCatalogLoadRequest,
) -> CommandResult<SourceDraftCatalogTargets> {
    catalog::load_existing_ledger_targets(store.inner(), runtime.inner(), request).await
}

#[tauri::command]
pub(crate) async fn desktop_apply_source_draft_existing_ledger_target(
    store: State<'_, SourceDraftStore>,
    runtime: State<'_, crate::tally::TallyRuntime>,
    request: SourceDraftCatalogApplyRequest,
) -> CommandResult<SourceDraftDto> {
    catalog::apply_existing_ledger_target(store.inner(), runtime.inner(), request).await
}

#[tauri::command]
pub(crate) fn desktop_invalidate_source_draft_existing_ledger_targets(
    store: State<'_, SourceDraftStore>,
    request: SourceDraftCatalogInvalidateRequest,
) -> CommandResult<u64> {
    // The resulting generation, not `()`, is the point: it is what lets the
    // frontend keep naming the right generation on its next invalidation
    // even when this one is a no-op. See `SourceDraftStore::invalidate_catalogue`.
    store.invalidate_catalogue(&request)
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

#[tauri::command]
pub(crate) fn desktop_register_source_draft_lifecycle_renderer(
    guard: State<'_, SourceDraftLifecycleGuard>,
    token: uuid::Uuid,
) {
    guard.renderer_registered(token);
}

#[tauri::command]
pub(crate) fn desktop_unregister_source_draft_lifecycle_renderer(
    guard: State<'_, SourceDraftLifecycleGuard>,
    token: uuid::Uuid,
) {
    guard.renderer_unregistered(token);
}

#[tauri::command]
pub(crate) fn desktop_pending_source_draft_lifecycle_request(
    guard: State<'_, SourceDraftLifecycleGuard>,
) -> Option<SourceDraftLifecycleRequest> {
    guard.pending()
}

#[tauri::command]
pub(crate) fn desktop_cancel_source_draft_lifecycle_request(
    guard: State<'_, SourceDraftLifecycleGuard>,
    request: SourceDraftLifecycleRequest,
) -> CommandResult<()> {
    if guard.cancel(&request) {
        Ok(())
    } else {
        Err(error("source_draft_lifecycle_request_not_pending"))
    }
}

#[tauri::command]
pub(crate) fn desktop_complete_source_draft_lifecycle_request(
    app: tauri::AppHandle,
    guard: State<'_, SourceDraftLifecycleGuard>,
    request: SourceDraftLifecycleRequest,
) -> CommandResult<()> {
    let window = match request.kind {
        SourceDraftLifecycleKind::Close => app
            .get_webview_window("main")
            .ok_or_else(|| error("source_draft_lifecycle_unavailable"))?,
        SourceDraftLifecycleKind::Exit => {
            if !guard.authorize(&request) {
                return Err(error("source_draft_lifecycle_request_not_pending"));
            }
            app.exit(0);
            return Ok(());
        }
    };
    if !guard.authorize(&request) {
        return Err(error("source_draft_lifecycle_request_not_pending"));
    }
    if window.close().is_err() {
        guard.restore_after_failed_close(request);
        return Err(error("source_draft_lifecycle_unavailable"));
    }
    Ok(())
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
        current_catalog_bindings: current_catalog_bindings(active),
        catalog_generation: active.catalog_generation,
    }
}

/// Coordinates whose proposed targets the most recent catalogue read justifies.
///
/// Shared by the success DTO and by a refused selection, which settles the same
/// bindings from the same response and so must report them identically.
fn current_catalog_bindings(active: &ActiveDraft) -> Vec<SourceDraftCurrentCatalogBinding> {
    active
        .catalog
        .as_ref()
        .map(|capture| {
            capture
                .current_binding_positions()
                .map(
                    |(row_position, entry_position)| SourceDraftCurrentCatalogBinding {
                        row_position,
                        entry_position,
                    },
                )
                .collect()
        })
        .unwrap_or_default()
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
    SourceDraftStore::remove_changed_bindings(current, &request.proposals);
    current.proposals = request.proposals;
    current.revision = next_revision;
    Ok(dto(current))
}

#[cfg(test)]
#[path = "source_draft_tests.rs"]
mod tests;
