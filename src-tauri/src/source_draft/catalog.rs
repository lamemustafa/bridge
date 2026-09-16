//! Ephemeral existing-ledger selections for one open source draft.
//!
//! Opaque catalog identities and selected master bindings are never serialized.
//! Only binding coordinates justified by the latest capture reach the desktop,
//! so it can avoid claiming that a saved proposal is current.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bridge_tally_core::master_binding::{
    self, BindingBasis, BindingStatus, Candidates, MasterCatalog, MasterClass, SourceEntity,
};
use bridge_tally_protocol::{StandardLedgerCatalog, StandardLedgerCatalogBinding};

use crate::{
    commands::SelectedCompanyIdentity,
    tally::{
        standard_ledger_catalog::StandardLedgerCatalogRead, EndpointKey, TallyConfig, TallyRuntime,
        VerifiedCompanyIdentity,
    },
};

use super::{
    current_catalog_bindings, dto, error,
    types::{CommandResult, MAX_TEXT_BYTES},
    validate_proposals, ActiveDraft, SourceDraftDto, SourceDraftProposal, SourceDraftStore,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceDraftCatalogLoadRequest {
    pub(crate) draft_id: String,
    pub(crate) config: TallyConfig,
    pub(crate) selected_company: SelectedCompanyIdentity,
}

/// Names the draft and catalog generation an invalidation is meant for.
///
/// Company-scope changes queue this command; by the time it is processed the
/// operator may already have replaced the draft, or an earlier invalidation
/// may already have advanced the generation. Carrying the identity the
/// request was issued against -- rather than applying to whatever draft
/// happens to be active when it runs -- is what lets the store tell a stale
/// invalidation apart from a current one instead of clobbering a catalogue it
/// was never meant to touch.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceDraftCatalogInvalidateRequest {
    pub(crate) draft_id: String,
    pub(crate) generation: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceDraftCatalogApplyRequest {
    pub(crate) draft_id: String,
    pub(crate) revision: u64,
    pub(crate) capture_id: String,
    pub(crate) config: TallyConfig,
    pub(crate) selected_company: SelectedCompanyIdentity,
    pub(crate) row_position: usize,
    pub(crate) entry_position: usize,
    pub(crate) target_name: String,
    pub(crate) proposals: Vec<SourceDraftProposal>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SourceDraftCatalogTargets {
    pub(crate) capture_id: String,
    pub(crate) source_sha256: String,
    pub(crate) targets: Vec<String>,
    pub(crate) bindings: Vec<SourceDraftCatalogBinding>,
    /// `complete` when every source entry was bound, `unavailable` when the
    /// narrowing pass could not run. An empty `bindings` list is otherwise
    /// indistinguishable from a failed one, and the catalogue read itself still
    /// succeeded.
    pub(crate) bindings_state: &'static str,
    pub(crate) evidence: SourceDraftCatalogEvidence,
}

/// One source entry's deterministic binding against the capture, so an
/// operator sees the few relevant ledgers rather than the whole catalog.
///
/// This narrows a list and grants nothing. `bound_target` names a live ledger
/// only where the rules in `bridge_tally_core::master_binding` decided it
/// outright; a near-miss carries candidates and no target. Applying any of
/// them still goes through the unchanged apply path, which rereads the catalog
/// and proves the selection is current — matching text remains never a
/// selected or approved target.
#[derive(Debug, Serialize)]
pub(crate) struct SourceDraftCatalogBinding {
    pub(crate) row_position: usize,
    pub(crate) entry_position: usize,
    pub(crate) bound_target: Option<String>,
    pub(crate) bound_basis: Option<BindingBasis>,
    pub(crate) unbound_reason: Option<&'static str>,
    pub(crate) candidates: Vec<String>,
    pub(crate) candidate_count: usize,
    /// `true` when unmaterialized identifier families prevent the core from
    /// establishing an exact union. A withheld or truncated listing can still
    /// have an exact count; only this flag requires rendering "at least N".
    pub(crate) candidate_count_is_lower_bound: bool,
    /// Which of the four candidate states this is, in the word the core type
    /// already tags its serialized form with. Carried rather than inferred: an
    /// empty listing beside a nonzero count is two different results — a family
    /// deliberately not sliced, and a report that ran out of room — and they
    /// call for opposite things from the operator.
    pub(crate) candidate_listing: &'static str,
}

#[derive(Debug, Serialize)]
pub(crate) struct SourceDraftCatalogEvidence {
    pub(crate) request_sha256: String,
    pub(crate) response_sha256: String,
    pub(crate) bytes: usize,
    pub(crate) state: &'static str,
}

#[derive(Clone)]
pub(super) struct CatalogCapture {
    id: Uuid,
    draft_id: Uuid,
    source_sha256: String,
    generation: u64,
    pub(super) endpoint: EndpointKey,
    pub(super) identity: VerifiedCompanyIdentity,
    pub(super) catalog: StandardLedgerCatalog,
    bindings: BTreeMap<(usize, usize), SelectedLedgerBinding>,
}

#[derive(Clone)]
struct SelectedLedgerBinding {
    name: String,
    #[allow(dead_code)]
    binding: StandardLedgerCatalogBinding,
}

impl CatalogCapture {
    pub(super) fn current_binding_positions(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.bindings.keys().copied()
    }
}

#[derive(Clone)]
pub(super) struct CatalogLoadSnapshot {
    draft_id: Uuid,
    source_sha256: String,
    generation: u64,
}

#[derive(Clone)]
pub(super) struct CatalogApplySnapshot {
    pub(super) draft_id: Uuid,
    pub(super) revision: u64,
    pub(super) source_sha256: String,
    pub(super) generation: u64,
    pub(super) capture_id: Uuid,
    pub(super) endpoint: EndpointKey,
    pub(super) identity: VerifiedCompanyIdentity,
    pub(super) catalog: StandardLedgerCatalog,
}

/// Binds every source entry's observed ledger name against the captured
/// catalog. Advisory only: an empty or unusable capture narrows nothing rather
/// than failing the read the operator just performed, and every returned name
/// is still revalidated by the apply path before it can become a target. A
/// folded name is a candidate only; this generic catalog has no
/// scope-qualified authority to select it.
fn source_entry_bindings(
    source: &crate::source_draft_xml::ParsedSource,
    targets: &[String],
) -> (Vec<SourceDraftCatalogBinding>, &'static str) {
    let Ok(catalog) = MasterCatalog::new(MasterClass::Ledger, targets) else {
        return (Vec::new(), "unavailable");
    };
    let mut located = Vec::new();
    let mut entities = Vec::new();
    for voucher in &source.vouchers {
        for entry in &voucher.entries {
            // Dropping an unusable entry would return fewer bindings than the
            // source has rows while still claiming completeness, and the row
            // that vanished is exactly the one an operator needs to look at.
            let Ok(entity) = SourceEntity::new(entities.len(), &entry.ledger) else {
                return (Vec::new(), "unavailable");
            };
            located.push((voucher.position, entry.position));
            entities.push(entity);
        }
    }
    let Ok(report) = master_binding::bind(&catalog, &entities) else {
        // A refusal is reported as such. Returning an empty list here would let
        // a failed pass read exactly like a source that narrowed to nothing.
        return (Vec::new(), "unavailable");
    };
    // The report itself is bounded by `MAX_REPORT_CANDIDATE_BYTES`, so this
    // path no longer needs a second budget of its own: capping the copy left
    // the original allocation unbounded, which was the actual stall risk.
    let bindings = report
        .entities()
        .iter()
        .zip(located)
        .map(
            |(binding, (row_position, entry_position))| match &binding.status {
                BindingStatus::Bound {
                    catalog_name,
                    basis,
                } => SourceDraftCatalogBinding {
                    row_position,
                    entry_position,
                    bound_target: Some(catalog_name.clone()),
                    bound_basis: Some(*basis),
                    unbound_reason: None,
                    candidates: Vec::new(),
                    candidate_count: 0,
                    candidate_count_is_lower_bound: false,
                    candidate_listing: Candidates::None.listing(),
                },
                BindingStatus::Ambiguous(unresolved) | BindingStatus::Unmatched(unresolved) => {
                    SourceDraftCatalogBinding {
                        row_position,
                        entry_position,
                        bound_target: None,
                        bound_basis: None,
                        unbound_reason: Some(unresolved.reason.safe_reason_code()),
                        // The state travels; it is not reconstructed on the
                        // other side. Deriving it from an empty list and a
                        // count could not tell a withheld family from an
                        // exhausted budget, and told the operator the report
                        // had run out of room when it had declined to slice.
                        candidate_listing: unresolved.candidates.listing(),
                        candidates: unresolved
                            .candidates
                            .listed()
                            .iter()
                            .map(|candidate| candidate.catalog_name.clone())
                            .collect(),
                        candidate_count: unresolved.candidates.found(),
                        candidate_count_is_lower_bound: unresolved
                            .candidates
                            .count_is_lower_bound(),
                    }
                }
            },
        )
        .collect();
    (bindings, "complete")
}

/// The freshly read catalog must still contain the selected pair. The response
/// is parsed once by the read itself, so this takes the parsed catalog rather
/// than reparsing the body per binding.
pub(super) fn require_current_catalog_binding(
    binding: &StandardLedgerCatalogBinding,
    fresh: &StandardLedgerCatalog,
) -> CommandResult<()> {
    if binding.matches_catalog(fresh) {
        Ok(())
    } else {
        Err(error("source_draft_catalogue_target_changed"))
    }
}

/// Preserve company-verification failures that have an existing source-draft
/// catalog result. Every other verification refusal remains a scope refusal:
/// this service cannot safely infer a more specific source-draft result.
///
/// `error.code` is an `&str`, not an enum, so the compiler cannot force this match to
/// stay exhaustive the way `classify_transport_error` in `standard_ledger_catalog.rs`
/// can be for `TallyTransportError`. The nine codes below are exactly the ones
/// `tally_runtime_command_error` (`src-tauri/src/commands.rs`) can emit -- a string
/// contract enforced only by this comment and by
/// `catalogue_company_verification_preserves_typed_error_codes`. Each is listed and
/// classified individually, including the two that fall to `scope_invalid`
/// (`company_base_currency_changed`, `tally_company_context_failed`): both are genuine
/// company-scope problems, so that is a deliberate choice, not a fallthrough. The `_`
/// arm is a documented conservative default for any other code, current or future.
fn company_verification_error_code(error: &crate::commands::TallyCommandError) -> &'static str {
    match error.code {
        "endpoint_unreachable"
        | "request_cancelled"
        | "tally_request_deadline_exceeded"
        | "tally_runtime_temporarily_unavailable" => "source_draft_catalogue_transport_failed",
        "response_validation_failed" => "source_draft_catalogue_malformed_response",
        "untrusted_discovery_limit_exceeded" => "source_draft_catalogue_bounds_invalid",
        // The endpoint configuration was never valid enough to evaluate a company at
        // all -- reporting this as an invalid company selection tells the operator to
        // fix the wrong thing.
        "endpoint_configuration_invalid" => "source_draft_catalogue_endpoint_invalid",
        "company_base_currency_changed" | "tally_company_context_failed" => {
            "source_draft_catalogue_scope_invalid"
        }
        _ => "source_draft_catalogue_scope_invalid",
    }
}

/// Loads a company-scoped catalog into the current source-draft capture.
///
/// This application service owns the complete admission sequence; callers
/// provide ordinary application handles rather than Tauri state wrappers.
pub(super) async fn load_existing_ledger_targets(
    store: &SourceDraftStore,
    runtime: &TallyRuntime,
    request: SourceDraftCatalogLoadRequest,
) -> CommandResult<SourceDraftCatalogTargets> {
    let snapshot = store.catalog_load_snapshot(&request)?;
    // No company has been evaluated yet at this point, so a rejected endpoint
    // must not read as an invalid company selection -- see
    // `company_verification_error_code`, which makes the same distinction for
    // the verification call just below this one.
    let endpoint = EndpointKey::from_config(&request.config)
        .map_err(|_| error("source_draft_catalogue_endpoint_invalid"))?;
    let identity = crate::commands::verify_observed_company_tuple(
        runtime,
        &request.config,
        &request.selected_company,
    )
    .await
    .map_err(|cause| error(company_verification_error_code(&cause)))?;
    let read = crate::tally::standard_ledger_catalog::read_standard_ledger_catalog(
        runtime,
        request.config,
        &identity,
    )
    .await
    .map_err(|cause| error(cause.command_code()))?;
    store.install_catalog(snapshot, endpoint, identity, read)
}

/// Revalidates and commits one captured existing-ledger selection.
///
/// This application service owns the complete admission sequence; callers
/// provide ordinary application handles rather than Tauri state wrappers.
pub(super) async fn apply_existing_ledger_target(
    store: &SourceDraftStore,
    runtime: &TallyRuntime,
    request: SourceDraftCatalogApplyRequest,
) -> CommandResult<SourceDraftDto> {
    let snapshot = store.catalog_apply_snapshot(&request)?;
    // No company has been evaluated yet at this point, so a rejected endpoint
    // must not read as an invalid company selection -- see
    // `company_verification_error_code`, which makes the same distinction for
    // the verification call just below this one.
    let endpoint = EndpointKey::from_config(&request.config)
        .map_err(|_| error("source_draft_catalogue_endpoint_invalid"))?;
    if endpoint != snapshot.endpoint {
        return Err(error("source_draft_catalogue_invalidated"));
    }
    let identity = crate::commands::verify_observed_company_tuple(
        runtime,
        &request.config,
        &request.selected_company,
    )
    .await
    .map_err(|cause| error(company_verification_error_code(&cause)))?;
    if identity != snapshot.identity {
        return Err(error("source_draft_catalogue_invalidated"));
    }
    let binding = snapshot
        .catalog
        .bind_selected([request.target_name.clone()])
        .map_err(|_| error("source_draft_catalogue_target_invalid"))?;
    let fresh = crate::tally::standard_ledger_catalog::read_standard_ledger_catalog(
        runtime,
        request.config.clone(),
        &identity,
    )
    .await
    .map_err(|cause| error(cause.command_code()))?;
    // One response decides both questions, so the currency of every retained
    // binding is settled even when this selection is refused. Deciding them
    // separately would let a refusal leave older bindings claiming a currency
    // this very read disproves.
    store.commit_catalog_target(snapshot, request, binding, &fresh.catalog)
}

impl SourceDraftStore {
    pub(super) fn catalog_load_snapshot(
        &self,
        request: &SourceDraftCatalogLoadRequest,
    ) -> CommandResult<CatalogLoadSnapshot> {
        let draft_id = Uuid::parse_str(&request.draft_id)
            .map_err(|_| error("source_draft_identifier_invalid"))?;
        let active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?;
        let active = active
            .as_ref()
            .ok_or_else(|| error("source_draft_not_active"))?;
        if active.id != draft_id {
            return Err(error("source_draft_revision_conflict"));
        }
        Ok(CatalogLoadSnapshot {
            draft_id,
            source_sha256: active.source.sha256.clone(),
            generation: active.catalog_generation,
        })
    }

    pub(super) fn install_catalog(
        &self,
        snapshot: CatalogLoadSnapshot,
        endpoint: EndpointKey,
        identity: VerifiedCompanyIdentity,
        read: StandardLedgerCatalogRead,
    ) -> CommandResult<SourceDraftCatalogTargets> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?;
        let current = active
            .as_mut()
            .ok_or_else(|| error("source_draft_not_active"))?;
        if current.id != snapshot.draft_id
            || current.source.sha256 != snapshot.source_sha256
            || current.catalog_generation != snapshot.generation
        {
            return Err(error("source_draft_catalogue_invalidated"));
        }
        let targets = read.catalog.names().map(str::to_owned).collect::<Vec<_>>();
        let (bindings, bindings_state) = source_entry_bindings(&current.source, &targets);
        let capture = CatalogCapture {
            id: Uuid::new_v4(),
            draft_id: current.id,
            source_sha256: current.source.sha256.clone(),
            generation: current.catalog_generation,
            endpoint,
            identity,
            catalog: read.catalog,
            bindings: BTreeMap::new(),
        };
        let output = SourceDraftCatalogTargets {
            capture_id: capture.id.to_string(),
            source_sha256: capture.source_sha256.clone(),
            targets,
            bindings,
            bindings_state,
            evidence: SourceDraftCatalogEvidence {
                request_sha256: read.request_sha256,
                response_sha256: read.response_sha256,
                bytes: read.bytes,
                state: "complete",
            },
        };
        current.catalog = Some(capture);
        Ok(output)
    }

    pub(super) fn catalog_apply_snapshot(
        &self,
        request: &SourceDraftCatalogApplyRequest,
    ) -> CommandResult<CatalogApplySnapshot> {
        let draft_id = Uuid::parse_str(&request.draft_id)
            .map_err(|_| error("source_draft_identifier_invalid"))?;
        let capture_id = Uuid::parse_str(&request.capture_id)
            .map_err(|_| error("source_draft_catalogue_invalidated"))?;
        let active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?;
        let active = active
            .as_ref()
            .ok_or_else(|| error("source_draft_not_active"))?;
        if active.id != draft_id || active.revision != request.revision {
            return Err(error("source_draft_revision_conflict"));
        }
        if request.target_name.is_empty() || request.target_name.len() > MAX_TEXT_BYTES {
            return Err(error("source_draft_catalogue_target_invalid"));
        }
        validate_proposals(&active.source, &request.proposals)?;
        let capture = active
            .catalog
            .as_ref()
            .ok_or_else(|| error("source_draft_catalogue_invalidated"))?;
        if capture.id != capture_id
            || capture.draft_id != active.id
            || capture.source_sha256 != active.source.sha256
            || capture.generation != active.catalog_generation
        {
            return Err(error("source_draft_catalogue_invalidated"));
        }
        let row = request
            .row_position
            .checked_sub(1)
            .ok_or_else(|| error("source_draft_catalogue_target_invalid"))?;
        let entry = request
            .entry_position
            .checked_sub(1)
            .ok_or_else(|| error("source_draft_catalogue_target_invalid"))?;
        if active
            .proposals
            .get(row)
            .and_then(|proposal| proposal.entries.get(entry))
            .is_none()
        {
            return Err(error("source_draft_catalogue_target_invalid"));
        }
        Ok(CatalogApplySnapshot {
            draft_id,
            revision: active.revision,
            source_sha256: active.source.sha256.clone(),
            generation: active.catalog_generation,
            capture_id,
            endpoint: capture.endpoint.clone(),
            identity: capture.identity.clone(),
            catalog: capture.catalog.clone(),
        })
    }

    pub(super) fn commit_catalog_target(
        &self,
        snapshot: CatalogApplySnapshot,
        request: SourceDraftCatalogApplyRequest,
        binding: StandardLedgerCatalogBinding,
        fresh: &StandardLedgerCatalog,
    ) -> CommandResult<SourceDraftDto> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?;
        let current = active
            .as_mut()
            .ok_or_else(|| error("source_draft_not_active"))?;
        if current.id != snapshot.draft_id
            || current.revision != snapshot.revision
            || current.source.sha256 != snapshot.source_sha256
            || current.catalog_generation != snapshot.generation
        {
            return Err(error("source_draft_catalogue_invalidated"));
        }
        let generation = current.catalog_generation;
        let capture = current
            .catalog
            .as_mut()
            .ok_or_else(|| error("source_draft_catalogue_invalidated"))?;
        if capture.id != snapshot.capture_id || capture.generation != generation {
            return Err(error("source_draft_catalogue_invalidated"));
        }
        // Settle every retained binding against this response before deciding the
        // requested target, so a refused selection cannot leave older bindings
        // claiming a currency the same read disproves.
        SourceDraftStore::revalidate_retained_bindings(current, fresh);
        // Past this point the store has already been changed by that settle, so
        // *every* way of failing owes the renderer the survivors -- not just the
        // refusal. Attaching them here rather than at each `?` makes that a
        // property of the boundary instead of something each new error path has
        // to remember, which is how the validation paths below were missed once
        // already.
        match Self::commit_settled_target(current, request, binding, fresh) {
            Ok(draft) => Ok(draft),
            Err(cause) => {
                Err(cause.with_current_catalog_bindings(current_catalog_bindings(current)))
            }
        }
    }

    /// The half of the commit that runs after the retained bindings have been
    /// settled. Split out so the caller can attach the survivors to anything
    /// this returns; it must not be called from anywhere else.
    fn commit_settled_target(
        current: &mut ActiveDraft,
        mut request: SourceDraftCatalogApplyRequest,
        binding: StandardLedgerCatalogBinding,
        fresh: &StandardLedgerCatalog,
    ) -> CommandResult<SourceDraftDto> {
        require_current_catalog_binding(&binding, fresh)?;
        let row = request.row_position - 1;
        let entry = request.entry_position - 1;
        request.proposals[row].entries[entry].ledger = Some(request.target_name.clone());
        validate_proposals(&current.source, &request.proposals)?;
        let next_revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| error("source_draft_revision_exhausted"))?;
        SourceDraftStore::remove_changed_bindings(current, &request.proposals);
        let capture = current
            .catalog
            .as_mut()
            .ok_or_else(|| error("source_draft_catalogue_invalidated"))?;
        capture.bindings.insert(
            (request.row_position, request.entry_position),
            SelectedLedgerBinding {
                name: request.target_name,
                binding,
            },
        );
        current.proposals = request.proposals;
        current.revision = next_revision;
        Ok(dto(current))
    }

    /// Returns the draft's catalog generation *after* this call, so a caller
    /// that only learns of a scope change can still name the right value the
    /// next time it invalidates -- see the no-op paths below for why that
    /// return value matters even when this call changes nothing.
    pub(super) fn invalidate_catalogue(
        &self,
        request: &SourceDraftCatalogInvalidateRequest,
    ) -> CommandResult<u64> {
        let draft_id = Uuid::parse_str(&request.draft_id)
            .map_err(|_| error("source_draft_identifier_invalid"))?;
        let mut active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?;
        let Some(current) = active.as_mut() else {
            // No draft is active at all, so nothing this request names still
            // exists. 0 is what every freshly loaded draft's generation
            // starts at, so a caller that folds this back in sees a value
            // consistent with whatever draft it opens next, rather than a
            // number left over from one that is already gone.
            return Ok(0);
        };
        // The draft or generation this invalidation names may already be gone
        // -- replaced by a newer draft, or already cleared by an earlier
        // invalidation -- because it was queued before either happened. What
        // it wanted invalidated is already gone, so this is a no-op success,
        // not a failure: an error here would make the frontend surface a
        // refusal for a request that has nothing left to refuse. Reporting
        // this draft's actual generation here -- instead of echoing back the
        // stale value the request named -- is what lets a caller that missed
        // an earlier advance self-correct on this very call, rather than
        // repeating the same stale request every time it invalidates again.
        if current.id != draft_id || current.catalog_generation != request.generation {
            return Ok(current.catalog_generation);
        }
        current.catalog_generation = current
            .catalog_generation
            .checked_add(1)
            .ok_or_else(|| error("source_draft_revision_exhausted"))?;
        current.catalog = None;
        Ok(current.catalog_generation)
    }

    pub(super) fn remove_changed_bindings(
        active: &mut ActiveDraft,
        proposals: &[SourceDraftProposal],
    ) {
        let Some(capture) = active.catalog.as_mut() else {
            return;
        };
        capture.bindings.retain(|(row, entry), binding| {
            proposals
                .get(row.saturating_sub(1))
                .and_then(|proposal| proposal.entries.get(entry.saturating_sub(1)))
                .and_then(|entry| entry.ledger.as_deref())
                == Some(binding.name.as_str())
        });
    }

    /// A retained binding can claim current-session status only when the same
    /// fresh response still contains its exact observed name and GUID.
    fn revalidate_retained_bindings(active: &mut ActiveDraft, fresh: &StandardLedgerCatalog) {
        let Some(capture) = active.catalog.as_mut() else {
            return;
        };
        capture
            .bindings
            .retain(|_, selected| selected.binding.matches_catalog(fresh));
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
