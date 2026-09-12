//! Ephemeral existing-ledger selections for one open source draft.
//!
//! Opaque catalog identities and selected master bindings are never serialized.
//! Only binding coordinates justified by the latest capture reach the desktop,
//! so it can avoid claiming that a saved proposal is current.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bridge_tally_core::master_binding::{
    self, BindingBasis, BindingStatus, MasterCatalog, MasterClass, SourceEntity,
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
    pub(crate) candidates_truncated: bool,
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
/// is still revalidated by the apply path before it can become a target.
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
                    candidates_truncated: false,
                },
                BindingStatus::Ambiguous(unresolved) | BindingStatus::Unmatched(unresolved) => {
                    SourceDraftCatalogBinding {
                        row_position,
                        entry_position,
                        bound_target: None,
                        bound_basis: None,
                        unbound_reason: Some(unresolved.reason.safe_reason_code()),
                        // The screen distinguishes the three cases from
                        // `candidate_count` against an empty list and is tested
                        // on each, so the DTO stays flat and this projection is
                        // the only place the typed shape is flattened.
                        candidates_truncated: unresolved.candidates.is_incomplete(),
                        candidates: unresolved
                            .candidates
                            .listed()
                            .iter()
                            .map(|candidate| candidate.catalog_name.clone())
                            .collect(),
                        candidate_count: unresolved.candidates.found(),
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
fn company_verification_error_code(error: &crate::commands::TallyCommandError) -> &'static str {
    match error.code {
        "endpoint_unreachable"
        | "request_cancelled"
        | "tally_request_deadline_exceeded"
        | "tally_runtime_temporarily_unavailable" => "source_draft_catalogue_transport_failed",
        "response_validation_failed" => "source_draft_catalogue_malformed_response",
        "untrusted_discovery_limit_exceeded" => "source_draft_catalogue_bounds_invalid",
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
    let endpoint = EndpointKey::from_config(&request.config)
        .map_err(|_| error("source_draft_catalogue_scope_invalid"))?;
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
    let endpoint = EndpointKey::from_config(&request.config)
        .map_err(|_| error("source_draft_catalogue_scope_invalid"))?;
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

    pub(super) fn invalidate_catalogue(&self) -> CommandResult<()> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| error("source_draft_state_unavailable"))?;
        if let Some(active) = active.as_mut() {
            active.catalog_generation = active
                .catalog_generation
                .checked_add(1)
                .ok_or_else(|| error("source_draft_revision_exhausted"))?;
            active.catalog = None;
        }
        Ok(())
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
mod tests {
    use super::*;
    use crate::{
        source_draft::{dto, empty_proposals, files::serialize_draft},
        source_draft_xml::parse_source_xml,
    };
    use bridge_tally_protocol::{
        decode_tally_xml_response_bytes_limited, parse_standard_ledger_catalog_with_identities,
        ExpectedTallyTextEncoding,
    };
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    /// Fabricated from a placeholder alphabet; nothing here is edited down
    /// from an observed book.
    fn fabricated_source() -> crate::source_draft_xml::ParsedSource {
        parse_source_xml(
            concat!(
                "<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE>",
                "<VOUCHER REMOTEID=\"ph-1\" VCHTYPE=\"Receipt\"><DATE>20260901</DATE>",
                "<ALLLEDGERENTRIES.LIST><LEDGERNAME>alpha traders</LEDGERNAME><AMOUNT>1</AMOUNT></ALLLEDGERENTRIES.LIST>",
                "<ALLLEDGERENTRIES.LIST><LEDGERNAME>GAMMA. EPSILON 5550000001</LEDGERNAME><AMOUNT>-1</AMOUNT></ALLLEDGERENTRIES.LIST>",
                "<ALLLEDGERENTRIES.LIST><LEDGERNAME>Zeta Placeholder</LEDGERNAME><AMOUNT>0</AMOUNT></ALLLEDGERENTRIES.LIST>",
                "</VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>"
            )
            .as_bytes(),
            "source.xml".into(),
        )
        .expect("fabricated source parses")
    }

    #[test]
    fn a_capture_narrows_each_source_entry_without_deciding_a_near_miss() {
        let targets = [
            "Alpha Traders".to_string(),
            "GAMMA (5550000001)".to_string(),
            "GAMMA ALPHA".to_string(),
            "Beta Supply".to_string(),
        ];
        let (bindings, state) = source_entry_bindings(&fabricated_source(), &targets);
        assert_eq!(state, "complete");
        assert_eq!(bindings.len(), 3);

        // Case alone does not defeat a bind, and the live spelling is named.
        assert_eq!(bindings[0].row_position, 1);
        assert_eq!(bindings[0].entry_position, 1);
        assert_eq!(bindings[0].bound_target.as_deref(), Some("Alpha Traders"));
        assert_eq!(bindings[0].bound_basis, Some(BindingBasis::NormalizedName));

        // The number the operator buried in the ledger name decides where the
        // name offers a wrong candidate.
        assert_eq!(
            bindings[1].bound_target.as_deref(),
            Some("GAMMA (5550000001)")
        );
        assert_eq!(bindings[1].bound_basis, Some(BindingBasis::Identifier));

        // Nothing defensible stays unbound with no target of any kind.
        assert!(bindings[2].bound_target.is_none());
        assert_eq!(
            bindings[2].unbound_reason,
            Some("master_binding_no_candidate")
        );
        assert!(bindings[2].candidates.is_empty());
    }

    #[test]
    fn a_narrowing_pass_that_could_not_run_says_so_rather_than_looking_empty() {
        // An unusable capture narrows nothing rather than discarding a read the
        // operator just performed — but "no bindings" and "binding failed" must
        // not read alike, because the catalogue read itself still succeeded.
        let (bindings, state) = source_entry_bindings(&fabricated_source(), &[]);
        assert!(bindings.is_empty());
        assert_eq!(state, "unavailable");
    }

    const CAPTURED_COMPANY: &str = "WR2 Unicode Lab";
    const CAPTURED_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

    /// Decode a captured response through the same reader production uses.
    ///
    /// The captures carry no BOM, because Tally declares the encoding in
    /// `Content-Type` rather than in the body. Supplying it the way the
    /// transport does keeps a fixture production would reject from quietly
    /// passing a test: a bespoke UTF-16LE loop here would decode bytes the real
    /// reader never accepts.
    fn decode_captured_response(bytes: &[u8]) -> String {
        decode_tally_xml_response_bytes_limited(
            bytes,
            "text/xml; charset=utf-16",
            ExpectedTallyTextEncoding::Utf16Le,
            bytes.len(),
        )
        .expect("captured catalogue decodes through the production reader")
        .text
    }

    fn captured_catalog_and_xml() -> (StandardLedgerCatalog, String) {
        let xml = decode_captured_response(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
        ));
        let catalog =
            parse_standard_ledger_catalog_with_identities(&xml, CAPTURED_COMPANY, CAPTURED_GUID)
                .expect("captured catalogue remains parser-admitted");
        (catalog, xml)
    }

    /// The ledger renamed between the two captured responses below.
    const RENAMED_FROM: &str = "WR2 Sales";
    const RENAMED_TO: &str = "WR2 Sales Renamed";
    /// A ledger the captured rename left alone, so a test can tell "settled
    /// because this read disproved it" apart from "cleared everything".
    const SURVIVES_RENAME: &str = "Cash";

    /// The rows the store currently treats as bound, in a stable order.
    fn binding_positions(store: &SourceDraftStore) -> Vec<(usize, usize)> {
        let active = store.active.lock().unwrap();
        let mut positions = active
            .as_ref()
            .and_then(|draft| draft.catalog.as_ref())
            .map(|capture| capture.current_binding_positions().collect::<Vec<_>>())
            .unwrap_or_default();
        positions.sort_unstable();
        positions
    }

    /// The same company's catalogue captured again after `WR2 Sales` was renamed
    /// to `WR2 Sales Renamed` in Tally, through the same production read path.
    ///
    /// This is the captured evidence for the behaviour already decided in
    /// `docs/tally/TALLY_PROTOCOL_REFERENCE.md` §12a.9 — Tally can retain a GUID
    /// while changing a visible ledger name — which is why
    /// `StandardLedgerCatalogBinding::matches` binds the selected pair rather
    /// than a name. The reference remains the decision; this fixture only shows
    /// it observed.
    ///
    /// Measured across the pair on TallyPrime 7.1: the ledger keeps GUID
    /// `…-000000d0` and only its name changes, the ledger count is unchanged, and
    /// no other ledger's GUID moves. The book was restored afterwards. Neither
    /// response is hand-mutated.
    fn captured_renamed_catalog_xml() -> String {
        decode_captured_response(include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue-renamed.utf16le.xml"
        ))
    }

    fn source() -> crate::source_draft_xml::ParsedSource {
        parse_source_xml(
            br#"<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE><VOUCHER REMOTEID="one" VCHTYPE="Receipt"><DATE>20260901</DATE><ALLLEDGERENTRIES.LIST><LEDGERNAME>Source one</LEDGERNAME><AMOUNT>1</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER><VOUCHER REMOTEID="two" VCHTYPE="Receipt"><DATE>20260902</DATE><ALLLEDGERENTRIES.LIST><LEDGERNAME>Source two</LEDGERNAME><AMOUNT>2</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>"#,
            "source.xml".into(),
        )
        .expect("two-row source control")
    }

    fn selected_company() -> SelectedCompanyIdentity {
        SelectedCompanyIdentity {
            display_name: CAPTURED_COMPANY.into(),
            company_guid: CAPTURED_GUID.into(),
            company_number: "1".into(),
            books_from_yyyymmdd: "20260401".into(),
        }
    }

    fn company_plan(name: &str, guid: &str) -> ScenarioPlan {
        ScenarioPlan::new(Fixture::SyntheticXml(format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"{name}\"><GUID>{guid}</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"
        )))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
    }

    fn catalog_plan(xml: String) -> ScenarioPlan {
        ScenarioPlan::new(Fixture::SyntheticXml(xml))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    }

    fn status_plan() -> ScenarioPlan {
        ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
            .with_framing(ResponseFraming::ContentLength)
    }

    fn append_catalog_read_plans(plans: &mut Vec<ScenarioPlan>, xml: String) {
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        plans.push(catalog_plan(xml.clone()));
        plans.push(status_plan());
        plans.push(catalog_plan(xml));
        plans.push(status_plan());
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
    }

    fn install_active_catalog(store: &SourceDraftStore) -> (Uuid, Uuid, Vec<String>, String) {
        let parsed_source = source();
        let (catalog, xml) = captured_catalog_and_xml();
        let names = catalog
            .names()
            .take(2)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 2, "captured catalogue has two bindable names");
        let id = Uuid::new_v4();
        let capture = CatalogCapture {
            id: Uuid::new_v4(),
            draft_id: id,
            source_sha256: parsed_source.sha256.clone(),
            generation: 0,
            endpoint: EndpointKey::from_config(&TallyConfig::default()).expect("loopback control"),
            identity: VerifiedCompanyIdentity::test_fixture(CAPTURED_COMPANY, CAPTURED_GUID),
            catalog,
            bindings: BTreeMap::new(),
        };
        let capture_id = capture.id;
        store
            .replace(ActiveDraft {
                id,
                revision: 1,
                proposals: empty_proposals(&parsed_source),
                source: parsed_source,
                catalog_generation: 0,
                catalog: Some(capture),
            })
            .expect("active captured catalogue");
        (id, capture_id, names, xml)
    }

    fn apply_request(
        id: Uuid,
        revision: u64,
        capture_id: Uuid,
        target_name: String,
        proposals: Vec<SourceDraftProposal>,
    ) -> SourceDraftCatalogApplyRequest {
        SourceDraftCatalogApplyRequest {
            draft_id: id.to_string(),
            revision,
            capture_id: capture_id.to_string(),
            config: TallyConfig::default(),
            selected_company: selected_company(),
            row_position: 1,
            entry_position: 1,
            target_name,
            proposals,
        }
    }

    fn install_active_draft_without_catalog(store: &SourceDraftStore) -> Uuid {
        let parsed_source = source();
        let draft_id = Uuid::new_v4();
        store
            .replace(ActiveDraft {
                id: draft_id,
                revision: 1,
                proposals: empty_proposals(&parsed_source),
                source: parsed_source,
                catalog_generation: 0,
                catalog: None,
            })
            .expect("active source draft");
        draft_id
    }

    fn set_active_catalog_endpoint(store: &SourceDraftStore, config: &TallyConfig) {
        store
            .active
            .lock()
            .expect("active store")
            .as_mut()
            .expect("active source draft")
            .catalog
            .as_mut()
            .expect("active catalog")
            .endpoint = EndpointKey::from_config(config).expect("loopback test endpoint");
    }

    fn unreachable_config() -> TallyConfig {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve a loopback test port");
        let port = listener
            .local_addr()
            .expect("read reserved test port")
            .port();
        drop(listener);
        TallyConfig {
            host: "127.0.0.1".into(),
            port,
        }
    }

    #[test]
    fn catalogue_company_verification_preserves_typed_error_codes() {
        let command_error = |code| crate::commands::TallyCommandError {
            code,
            category: "Operation",
            message: String::new(),
            retry: "after_change",
            local_state_changed: false,
            tally_state_may_have_changed: false,
            remediation: "Retry.",
        };
        for code in [
            "endpoint_unreachable",
            "request_cancelled",
            "tally_request_deadline_exceeded",
            "tally_runtime_temporarily_unavailable",
        ] {
            assert_eq!(
                company_verification_error_code(&command_error(code)),
                "source_draft_catalogue_transport_failed"
            );
        }

        assert_eq!(
            company_verification_error_code(&command_error("response_validation_failed")),
            "source_draft_catalogue_malformed_response"
        );

        assert_eq!(
            company_verification_error_code(&command_error("untrusted_discovery_limit_exceeded")),
            "source_draft_catalogue_bounds_invalid"
        );

        assert_eq!(
            company_verification_error_code(&command_error("reviewed_company_scope_changed")),
            "source_draft_catalogue_scope_invalid"
        );

        assert_eq!(
            company_verification_error_code(&command_error("endpoint_configuration_invalid")),
            "source_draft_catalogue_scope_invalid"
        );
    }

    #[tokio::test]
    async fn catalogue_services_classify_unreachable_company_verification_as_transport_failure() {
        let config = unreachable_config();

        let load_store = SourceDraftStore::default();
        let load_draft_id = install_active_draft_without_catalog(&load_store);
        let load_error = load_existing_ledger_targets(
            &load_store,
            &TallyRuntime::default(),
            SourceDraftCatalogLoadRequest {
                draft_id: load_draft_id.to_string(),
                config: config.clone(),
                selected_company: selected_company(),
            },
        )
        .await
        .expect_err("an unreachable company list must not become a scope refusal");
        assert_eq!(load_error.code, "source_draft_catalogue_transport_failed");

        let apply_store = SourceDraftStore::default();
        let (draft_id, capture_id, names, _) = install_active_catalog(&apply_store);
        set_active_catalog_endpoint(&apply_store, &config);
        let proposals = apply_store
            .active
            .lock()
            .expect("active store")
            .as_ref()
            .expect("active source draft")
            .proposals
            .clone();
        let mut request = apply_request(draft_id, 1, capture_id, names[0].clone(), proposals);
        request.config = config;
        let apply_error =
            apply_existing_ledger_target(&apply_store, &TallyRuntime::default(), request)
                .await
                .expect_err("an unreachable company list must not become a scope refusal");
        assert_eq!(apply_error.code, "source_draft_catalogue_transport_failed");
    }

    #[tokio::test]
    async fn catalogue_services_classify_observed_company_mismatch_as_scope_invalid() {
        let alternate_guid = "11111111-1111-4111-8111-111111111111";

        let load_store = SourceDraftStore::default();
        let load_draft_id = install_active_draft_without_catalog(&load_store);
        let load_simulator =
            SequenceSimulator::spawn(vec![company_plan("WR3 Separate Lab", alternate_guid)])
                .expect("load scope simulator");
        let load_config = TallyConfig {
            host: load_simulator.address().ip().to_string(),
            port: load_simulator.address().port(),
        };
        let load_error = load_existing_ledger_targets(
            &load_store,
            &TallyRuntime::default(),
            SourceDraftCatalogLoadRequest {
                draft_id: load_draft_id.to_string(),
                config: load_config,
                selected_company: selected_company(),
            },
        )
        .await
        .expect_err("a returned company list that lacks the tuple must refuse scope");
        assert_eq!(load_error.code, "source_draft_catalogue_scope_invalid");
        assert_eq!(
            load_simulator
                .finish()
                .expect("load company request observed")
                .len(),
            1
        );

        let apply_store = SourceDraftStore::default();
        let (draft_id, capture_id, names, _) = install_active_catalog(&apply_store);
        let apply_simulator =
            SequenceSimulator::spawn(vec![company_plan("WR3 Separate Lab", alternate_guid)])
                .expect("apply scope simulator");
        let apply_config = TallyConfig {
            host: apply_simulator.address().ip().to_string(),
            port: apply_simulator.address().port(),
        };
        set_active_catalog_endpoint(&apply_store, &apply_config);
        let proposals = apply_store
            .active
            .lock()
            .expect("active store")
            .as_ref()
            .expect("active source draft")
            .proposals
            .clone();
        let mut request = apply_request(draft_id, 1, capture_id, names[0].clone(), proposals);
        request.config = apply_config;
        let apply_error =
            apply_existing_ledger_target(&apply_store, &TallyRuntime::default(), request)
                .await
                .expect_err("a returned company list that lacks the tuple must refuse scope");
        assert_eq!(apply_error.code, "source_draft_catalogue_scope_invalid");
        assert_eq!(
            apply_simulator
                .finish()
                .expect("apply company request observed")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn catalog_services_load_then_refuse_an_identity_invalidated_apply_without_tauri_state() {
        let store = SourceDraftStore::default();
        let parsed_source = source();
        let draft_id = Uuid::new_v4();
        store
            .replace(ActiveDraft {
                id: draft_id,
                revision: 1,
                proposals: empty_proposals(&parsed_source),
                source: parsed_source,
                catalog_generation: 0,
                catalog: None,
            })
            .expect("active source draft");
        let (_, catalog_xml) = captured_catalog_and_xml();
        let alternate_guid = "11111111-1111-4111-8111-111111111111";
        let simulator = SequenceSimulator::spawn(vec![
            company_plan(CAPTURED_COMPANY, CAPTURED_GUID),
            company_plan(CAPTURED_COMPANY, CAPTURED_GUID),
            catalog_plan(catalog_xml.clone()),
            status_plan(),
            catalog_plan(catalog_xml),
            status_plan(),
            company_plan(CAPTURED_COMPANY, CAPTURED_GUID),
            company_plan("WR3 Separate Lab", alternate_guid),
        ])
        .expect("catalog service simulator");
        let config = TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::default();
        let loaded = load_existing_ledger_targets(
            &store,
            &runtime,
            SourceDraftCatalogLoadRequest {
                draft_id: draft_id.to_string(),
                config: config.clone(),
                selected_company: selected_company(),
            },
        )
        .await
        .expect("service load admits the current captured catalog");
        assert!(!loaded.targets.is_empty());
        let proposals = store
            .active
            .lock()
            .expect("active store")
            .as_ref()
            .expect("active source draft")
            .proposals
            .clone();
        let error = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: 1,
                capture_id: loaded.capture_id,
                config,
                selected_company: SelectedCompanyIdentity {
                    display_name: "WR3 Separate Lab".into(),
                    company_guid: alternate_guid.into(),
                    company_number: "1".into(),
                    books_from_yyyymmdd: "20260401".into(),
                },
                row_position: 1,
                entry_position: 1,
                target_name: loaded.targets[0].clone(),
                proposals,
            },
        )
        .await
        .expect_err("a reverified different identity invalidates the captured catalog");
        assert_eq!(error.code, "source_draft_catalogue_invalidated");
        assert_eq!(simulator.finish().expect("all requests observed").len(), 8);
    }

    #[tokio::test]
    async fn catalog_services_revalidate_retained_binding_after_catalogue_change_without_tauri_state(
    ) {
        let store = SourceDraftStore::default();
        let draft_id = install_active_draft_without_catalog(&store);
        let (_, catalog_xml) = captured_catalog_and_xml();
        let catalog = parse_standard_ledger_catalog_with_identities(
            &catalog_xml,
            CAPTURED_COMPANY,
            CAPTURED_GUID,
        )
        .expect("captured catalogue remains parser-admitted");
        // Target A is the ledger that a real rename in Tally moved between the two
        // captured responses; target B is any other ledger, present in both.
        let all = catalog.names().map(str::to_owned).collect::<Vec<_>>();
        assert!(
            all.iter().any(|name| name == RENAMED_FROM),
            "captured catalogue still contains the ledger the rename capture moved"
        );
        let other = all
            .iter()
            .find(|name| name.as_str() != RENAMED_FROM)
            .expect("captured catalogue has a second bindable name")
            .clone();
        let names = [RENAMED_FROM.to_owned(), other];

        let changed_catalog_xml = captured_renamed_catalog_xml();
        let changed_catalog = parse_standard_ledger_catalog_with_identities(
            &changed_catalog_xml,
            CAPTURED_COMPANY,
            CAPTURED_GUID,
        )
        .expect("captured renamed catalogue remains parser-admitted");
        let changed_names = changed_catalog.names().collect::<Vec<_>>();
        // The captured pair really is a rename, not a hand-edited string: A is gone
        // under its old name, present under the new one, and the ledger count holds.
        assert!(
            !changed_names.contains(&names[0].as_str()),
            "the live rename removed target A's old name"
        );
        assert!(
            changed_names.contains(&RENAMED_TO),
            "the live rename introduced target A's new name"
        );
        assert!(
            changed_names.contains(&names[1].as_str()),
            "target B survives the live rename untouched"
        );
        assert_eq!(
            changed_names.len(),
            all.len(),
            "a rename changes no ledger count"
        );
        let mut plans = vec![company_plan(CAPTURED_COMPANY, CAPTURED_GUID)];
        append_catalog_read_plans(&mut plans, catalog_xml.clone());
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, catalog_xml);
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, changed_catalog_xml);
        let simulator = SequenceSimulator::spawn(plans).expect("catalog service simulator");
        let config = TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::default();
        let loaded = load_existing_ledger_targets(
            &store,
            &runtime,
            SourceDraftCatalogLoadRequest {
                draft_id: draft_id.to_string(),
                config: config.clone(),
                selected_company: selected_company(),
            },
        )
        .await
        .expect("service load admits the captured catalog");
        // A must be the ledger the live rename actually moved, otherwise this
        // exercises a target Tally never touched and proves nothing.
        let a = loaded
            .targets
            .iter()
            .find(|target| target.as_str() == names[0])
            .expect("the renamed ledger is offered as a target")
            .clone();
        let b = loaded
            .targets
            .iter()
            .find(|target| target.as_str() == names[1])
            .expect("a second, untouched ledger is offered as a target")
            .clone();
        let initial_proposals = store
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proposals
            .clone();
        let first = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: 1,
                capture_id: loaded.capture_id.clone(),
                config: config.clone(),
                selected_company: selected_company(),
                row_position: 1,
                entry_position: 1,
                target_name: a.clone(),
                proposals: initial_proposals,
            },
        )
        .await
        .expect("target A applies after its fresh read");
        assert_eq!(
            first
                .current_catalog_bindings
                .iter()
                .map(|binding| (binding.row_position, binding.entry_position))
                .collect::<Vec<_>>(),
            vec![(1, 1)],
            "A is current after its own fresh read"
        );
        let second = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: first.revision,
                capture_id: loaded.capture_id,
                config,
                selected_company: selected_company(),
                row_position: 2,
                entry_position: 1,
                target_name: b.clone(),
                proposals: first.rows.iter().map(|row| row.proposal.clone()).collect(),
            },
        )
        .await
        .expect("target B applies after its fresh read");
        assert_eq!(
            second
                .current_catalog_bindings
                .iter()
                .map(|binding| (binding.row_position, binding.entry_position))
                .collect::<Vec<_>>(),
            vec![(2, 1)],
            "only B is current after Tally removed A"
        );
        assert_eq!(
            second.rows[0].proposal.entries[0].ledger.as_deref(),
            Some(a.as_str()),
            "A's operator proposal text survives without a currency claim"
        );
        assert_eq!(
            second.rows[1].proposal.entries[0].ledger.as_deref(),
            Some(b.as_str())
        );
        assert_eq!(simulator.finish().expect("all requests observed").len(), 21);
    }

    /// A refused selection must not leave older bindings claiming a currency the
    /// same response disproves. Binding the renamed ledger a second time fails,
    /// and that failure has to settle the first binding too.
    #[tokio::test]
    async fn a_refused_selection_still_settles_retained_bindings_without_tauri_state() {
        let store = SourceDraftStore::default();
        let draft_id = install_active_draft_without_catalog(&store);
        let (_, catalog_xml) = captured_catalog_and_xml();
        let renamed_catalog_xml = captured_renamed_catalog_xml();

        let mut plans = vec![company_plan(CAPTURED_COMPANY, CAPTURED_GUID)];
        append_catalog_read_plans(&mut plans, catalog_xml.clone());
        for _ in 0..2 {
            plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
            append_catalog_read_plans(&mut plans, catalog_xml.clone());
        }
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, renamed_catalog_xml);
        let simulator = SequenceSimulator::spawn(plans).expect("catalog service simulator");
        let config = TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::default();

        let loaded = load_existing_ledger_targets(
            &store,
            &runtime,
            SourceDraftCatalogLoadRequest {
                draft_id: draft_id.to_string(),
                config: config.clone(),
                selected_company: selected_company(),
            },
        )
        .await
        .expect("service load admits the captured catalog");
        for expected in [RENAMED_FROM, SURVIVES_RENAME] {
            assert!(
                loaded.targets.iter().any(|target| target == expected),
                "the captured catalogue offers {expected}"
            );
        }

        let mut proposals = store
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proposals
            .clone();
        let mut revision = 1;
        // Row 1 binds the ledger the live rename moved; row 2 binds one it left
        // alone. Both are current against the catalogue read at the time.
        for (row_position, target_name) in [(1, RENAMED_FROM), (2, SURVIVES_RENAME)] {
            let applied = apply_existing_ledger_target(
                &store,
                &runtime,
                SourceDraftCatalogApplyRequest {
                    draft_id: draft_id.to_string(),
                    revision,
                    capture_id: loaded.capture_id.clone(),
                    config: config.clone(),
                    selected_company: selected_company(),
                    row_position,
                    entry_position: 1,
                    target_name: target_name.to_owned(),
                    proposals,
                },
            )
            .await
            .expect("the target applies while Tally still offers it");
            revision = applied.revision;
            proposals = applied
                .rows
                .iter()
                .map(|row| row.proposal.clone())
                .collect();
        }
        assert_eq!(
            binding_positions(&store),
            vec![(1, 1), (2, 1)],
            "both rows are current before Tally moves"
        );

        // Tally renames the row 1 ledger, and the operator selects it again.
        let refused = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision,
                capture_id: loaded.capture_id,
                config,
                selected_company: selected_company(),
                row_position: 1,
                entry_position: 1,
                target_name: RENAMED_FROM.to_owned(),
                proposals,
            },
        )
        .await
        .expect_err("the renamed target is refused");
        assert_eq!(refused.code, "source_draft_catalogue_target_changed");

        // The refusing read settles row 1, which it disproves, and leaves row 2,
        // which it upholds -- so this is evidence, not a blanket clear.
        assert_eq!(
            binding_positions(&store),
            vec![(2, 1)],
            "the refusing read settles only what it disproves"
        );
        assert_eq!(
            refused
                .current_catalog_bindings
                .as_deref()
                .expect("a refusal reports what the same read settled")
                .iter()
                .map(|binding| (binding.row_position, binding.entry_position))
                .collect::<Vec<_>>(),
            vec![(2, 1)],
            "the refusal carries the survivors, so the renderer need not guess"
        );
        assert_eq!(simulator.finish().expect("all requests observed").len(), 28);
    }

    /// The refusal is not the only way to fail after the bindings have been
    /// settled: the commit still validates the *completed* proposal and still
    /// bumps the revision. Any of those reaches the renderer with the store
    /// already changed, so it owes the same evidence.
    ///
    /// This drives the revision path because it is the one reachable from a
    /// small fixture. The proposal-size path needs a source large enough for the
    /// inserted target name to cross `MAX_PROPOSAL_BYTES`, which the per-field
    /// `MAX_TEXT_BYTES` bound puts several megabytes out of reach here. Both are
    /// covered by the same boundary rather than individually, which is the point
    /// of attaching the survivors once.
    #[tokio::test]
    async fn a_post_settle_failure_other_than_refusal_also_reports_the_surviving_bindings() {
        let store = SourceDraftStore::default();
        let draft_id = install_active_draft_without_catalog(&store);
        let (_, catalog_xml) = captured_catalog_and_xml();

        let mut plans = vec![company_plan(CAPTURED_COMPANY, CAPTURED_GUID)];
        append_catalog_read_plans(&mut plans, catalog_xml.clone());
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, catalog_xml.clone());
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, catalog_xml);
        let simulator = SequenceSimulator::spawn(plans).expect("catalog service simulator");
        let config = TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::default();

        let loaded = load_existing_ledger_targets(
            &store,
            &runtime,
            SourceDraftCatalogLoadRequest {
                draft_id: draft_id.to_string(),
                config: config.clone(),
                selected_company: selected_company(),
            },
        )
        .await
        .expect("service load admits the captured catalog");
        let proposals = store
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proposals
            .clone();

        let bound = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: 1,
                capture_id: loaded.capture_id.clone(),
                config: config.clone(),
                selected_company: selected_company(),
                row_position: 1,
                entry_position: 1,
                target_name: SURVIVES_RENAME.to_owned(),
                proposals,
            },
        )
        .await
        .expect("the surviving target applies");

        // Exhaust the revision so the commit fails after the settle, on a path
        // that is not the refusal.
        store.active.lock().unwrap().as_mut().unwrap().revision = u64::MAX;
        let rejected = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: u64::MAX,
                capture_id: loaded.capture_id,
                config,
                selected_company: selected_company(),
                row_position: 2,
                entry_position: 1,
                target_name: SURVIVES_RENAME.to_owned(),
                proposals: bound.rows.iter().map(|row| row.proposal.clone()).collect(),
            },
        )
        .await
        .expect_err("the revision cannot advance");
        assert_eq!(rejected.code, "source_draft_revision_exhausted");
        assert_eq!(
            rejected
                .current_catalog_bindings
                .as_deref()
                .expect("a failure after the settle reports what survived")
                .iter()
                .map(|binding| (binding.row_position, binding.entry_position))
                .collect::<Vec<_>>(),
            vec![(1, 1)],
            "row 1 is still current, and the renderer is told so despite the failure"
        );
        assert_eq!(simulator.finish().expect("all requests observed").len(), 21);
    }

    #[tokio::test]
    async fn catalog_services_keep_identical_retained_bindings_current_without_tauri_state() {
        let store = SourceDraftStore::default();
        let draft_id = install_active_draft_without_catalog(&store);
        let (_, catalog_xml) = captured_catalog_and_xml();
        let mut plans = vec![company_plan(CAPTURED_COMPANY, CAPTURED_GUID)];
        append_catalog_read_plans(&mut plans, catalog_xml.clone());
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, catalog_xml.clone());
        plans.push(company_plan(CAPTURED_COMPANY, CAPTURED_GUID));
        append_catalog_read_plans(&mut plans, catalog_xml);
        let simulator = SequenceSimulator::spawn(plans).expect("catalog service simulator");
        let config = TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        };
        let runtime = TallyRuntime::default();
        let loaded = load_existing_ledger_targets(
            &store,
            &runtime,
            SourceDraftCatalogLoadRequest {
                draft_id: draft_id.to_string(),
                config: config.clone(),
                selected_company: selected_company(),
            },
        )
        .await
        .expect("service load admits the captured catalog");
        let initial_proposals = store
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proposals
            .clone();
        let first = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: 1,
                capture_id: loaded.capture_id.clone(),
                config: config.clone(),
                selected_company: selected_company(),
                row_position: 1,
                entry_position: 1,
                target_name: loaded.targets[0].clone(),
                proposals: initial_proposals,
            },
        )
        .await
        .expect("target A applies after its fresh read");
        let second = apply_existing_ledger_target(
            &store,
            &runtime,
            SourceDraftCatalogApplyRequest {
                draft_id: draft_id.to_string(),
                revision: first.revision,
                capture_id: loaded.capture_id,
                config,
                selected_company: selected_company(),
                row_position: 2,
                entry_position: 1,
                target_name: loaded.targets[1].clone(),
                proposals: first.rows.iter().map(|row| row.proposal.clone()).collect(),
            },
        )
        .await
        .expect("target B applies after its fresh read");
        assert_eq!(
            second
                .current_catalog_bindings
                .iter()
                .map(|binding| (binding.row_position, binding.entry_position))
                .collect::<Vec<_>>(),
            vec![(1, 1), (2, 1)],
            "an unchanged A remains current alongside B"
        );
        assert_eq!(simulator.finish().expect("all requests observed").len(), 21);
    }

    #[test]
    fn full_apply_prunes_another_changed_binding_but_keeps_the_new_binding() {
        let store = SourceDraftStore::default();
        let (id, capture_id, names, _) = install_active_catalog(&store);
        let (catalog, _) = captured_catalog_and_xml();
        let first_binding = catalog
            .bind_selected([names[0].clone()])
            .expect("first captured target binds");
        let second_binding = catalog
            .bind_selected([names[1].clone()])
            .expect("second captured target binds");
        let mut proposals = store
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proposals
            .clone();
        proposals[0].entries[0].ledger = Some(names[0].clone());
        {
            let mut active = store.active.lock().unwrap();
            let capture = active.as_mut().unwrap().catalog.as_mut().unwrap();
            capture.bindings.insert(
                (1, 1),
                SelectedLedgerBinding {
                    name: names[0].clone(),
                    binding: first_binding,
                },
            );
        }
        proposals[0].entries[0].ledger = Some("manually changed target".into());
        let mut request = apply_request(id, 1, capture_id, names[1].clone(), proposals);
        request.row_position = 2;
        let snapshot = store
            .catalog_apply_snapshot(&request)
            .expect("full proposal vector is admissible before fresh read");
        store
            .commit_catalog_target(snapshot, request, second_binding, &catalog)
            .expect("target B commits after its fresh read");

        let active = store.active.lock().unwrap();
        let bindings = &active.as_ref().unwrap().catalog.as_ref().unwrap().bindings;
        assert!(
            !bindings.contains_key(&(1, 1)),
            "changed target A is no longer bound"
        );
        assert_eq!(bindings.get(&(2, 1)).unwrap().name, names[1]);
    }

    #[test]
    fn stale_load_cannot_install_after_invalidation_or_draft_replacement() {
        let store = SourceDraftStore::default();
        let initial_source = source();
        let first_id = Uuid::new_v4();
        store
            .replace(ActiveDraft {
                id: first_id,
                revision: 1,
                proposals: empty_proposals(&initial_source),
                source: initial_source,
                catalog_generation: 0,
                catalog: None,
            })
            .unwrap();
        let request = SourceDraftCatalogLoadRequest {
            draft_id: first_id.to_string(),
            config: TallyConfig::default(),
            selected_company: selected_company(),
        };
        let snapshot = store.catalog_load_snapshot(&request).unwrap();
        store.invalidate_catalogue().unwrap();
        let (catalog, _) = captured_catalog_and_xml();
        let read = StandardLedgerCatalogRead {
            catalog,
            request_sha256: "request".into(),
            response_sha256: "response".into(),
            bytes: 2,
        };
        assert_eq!(
            store
                .install_catalog(
                    snapshot,
                    EndpointKey::from_config(&TallyConfig::default()).unwrap(),
                    VerifiedCompanyIdentity::test_fixture(CAPTURED_COMPANY, CAPTURED_GUID),
                    read,
                )
                .unwrap_err()
                .code,
            "source_draft_catalogue_invalidated"
        );
        let active = store.active.lock().unwrap().clone().unwrap();
        assert!(active.catalog.is_none());
        assert_eq!(active.revision, 1);

        let snapshot = store.catalog_load_snapshot(&request).unwrap();
        let replacement_source = source();
        let replacement = ActiveDraft {
            id: Uuid::new_v4(),
            revision: 1,
            proposals: empty_proposals(&replacement_source),
            source: replacement_source,
            catalog_generation: 0,
            catalog: None,
        };
        let expected = serde_json::to_vec(&replacement.proposals).unwrap();
        store.replace(replacement).unwrap();
        let (catalog, _) = captured_catalog_and_xml();
        assert_eq!(
            store
                .install_catalog(
                    snapshot,
                    EndpointKey::from_config(&TallyConfig::default()).unwrap(),
                    VerifiedCompanyIdentity::test_fixture(CAPTURED_COMPANY, CAPTURED_GUID),
                    StandardLedgerCatalogRead {
                        catalog,
                        request_sha256: "request".into(),
                        response_sha256: "response".into(),
                        bytes: 2,
                    },
                )
                .unwrap_err()
                .code,
            "source_draft_catalogue_invalidated"
        );
        let active = store.active.lock().unwrap().clone().unwrap();
        assert!(active.catalog.is_none());
        assert_eq!(serde_json::to_vec(&active.proposals).unwrap(), expected);
    }

    #[test]
    fn stale_apply_revision_capture_and_replacement_leave_proposals_unchanged() {
        let store = SourceDraftStore::default();
        let (id, capture_id, names, _) = install_active_catalog(&store);
        let proposals = store
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .proposals
            .clone();
        let stale_revision = apply_request(id, 0, capture_id, names[0].clone(), proposals.clone());
        assert_eq!(
            store
                .catalog_apply_snapshot(&stale_revision)
                .err()
                .expect("stale revision refuses")
                .code,
            "source_draft_revision_conflict"
        );
        let stale_capture =
            apply_request(id, 1, Uuid::new_v4(), names[0].clone(), proposals.clone());
        assert_eq!(
            store
                .catalog_apply_snapshot(&stale_capture)
                .err()
                .expect("stale capture refuses")
                .code,
            "source_draft_catalogue_invalidated"
        );
        let request = apply_request(id, 1, capture_id, names[0].clone(), proposals);
        let snapshot = store.catalog_apply_snapshot(&request).unwrap();
        let binding = snapshot.catalog.bind_selected([names[0].clone()]).unwrap();
        // Rejected on the snapshot check before the fresh catalog is consulted.
        let fresh = snapshot.catalog.clone();
        let before =
            serde_json::to_vec(&store.active.lock().unwrap().as_ref().unwrap().proposals).unwrap();
        store.invalidate_catalogue().unwrap();
        assert_eq!(
            store
                .commit_catalog_target(snapshot, request, binding, &fresh)
                .unwrap_err()
                .code,
            "source_draft_catalogue_invalidated"
        );
        assert_eq!(
            serde_json::to_vec(&store.active.lock().unwrap().as_ref().unwrap().proposals).unwrap(),
            before
        );

        let (replacement_id, replacement_capture, replacement_names, _) =
            install_active_catalog(&store);
        let request = apply_request(
            replacement_id,
            1,
            replacement_capture,
            replacement_names[0].clone(),
            store
                .active
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .proposals
                .clone(),
        );
        let snapshot = store.catalog_apply_snapshot(&request).unwrap();
        let binding = snapshot
            .catalog
            .bind_selected([replacement_names[0].clone()])
            .unwrap();
        let replacement_source = source();
        let replacement = ActiveDraft {
            id: Uuid::new_v4(),
            revision: 1,
            proposals: empty_proposals(&replacement_source),
            source: replacement_source,
            catalog_generation: 0,
            catalog: None,
        };
        let expected = serde_json::to_vec(&replacement.proposals).unwrap();
        // Rejected on the snapshot check before the fresh catalog is consulted.
        let fresh = snapshot.catalog.clone();
        store.replace(replacement).unwrap();
        assert_eq!(
            store
                .commit_catalog_target(snapshot, request, binding, &fresh)
                .unwrap_err()
                .code,
            "source_draft_catalogue_invalidated"
        );
        let active = store.active.lock().unwrap().clone().unwrap();
        assert_eq!(serde_json::to_vec(&active.proposals).unwrap(), expected);
        assert!(active.catalog.is_none());
    }

    #[test]
    fn catalog_binding_identities_are_not_serialized_in_dto_or_saved_draft() {
        let store = SourceDraftStore::default();
        let (_, _, names, _) = install_active_catalog(&store);
        let (catalog, _) = captured_catalog_and_xml();
        let binding = catalog.bind_selected([names[0].clone()]).unwrap();
        {
            let mut locked = store.active.lock().unwrap();
            locked
                .as_mut()
                .unwrap()
                .catalog
                .as_mut()
                .unwrap()
                .bindings
                .insert(
                    (1, 1),
                    SelectedLedgerBinding {
                        name: names[0].clone(),
                        binding,
                    },
                );
        }
        let active = store.active.lock().unwrap().clone().unwrap();
        let response = serde_json::to_value(dto(&active)).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&serialize_draft(&active.source, &active.proposals).unwrap())
                .unwrap();
        assert_eq!(
            response["current_catalog_bindings"],
            serde_json::json!([{"row_position": 1, "entry_position": 1}]),
            "the desktop receives only the current binding coordinate"
        );
        // A refusal now carries binding coordinates too, so it is the same wire
        // surface and gets the same guarantee.
        let refusal = serde_json::to_value(
            error("source_draft_catalogue_target_changed")
                .with_current_catalog_bindings(current_catalog_bindings(&active)),
        )
        .unwrap();
        assert_eq!(
            refusal["current_catalog_bindings"],
            serde_json::json!([{"row_position": 1, "entry_position": 1}]),
            "a refusal reports coordinates and nothing identifying"
        );
        // An unrelated failure proves nothing about any binding, and must be
        // distinguishable from a refusal that disproved them all.
        let unrelated = serde_json::to_value(error("source_draft_not_active")).unwrap();
        assert!(
            unrelated
                .as_object()
                .unwrap()
                .get("current_catalog_bindings")
                .is_none(),
            "an error carrying no binding evidence omits the field entirely"
        );

        for value in [&response, &saved, &refusal] {
            let object = value.as_object().unwrap();
            assert!(!object.contains_key("catalog"));
            assert!(!object.contains_key("capture_id"));
            assert!(!object.contains_key("binding"));
            assert!(!object.contains_key("company_guid"));
        }
    }

    /// Both sides are captured responses either side of a real rename in Tally,
    /// so this refuses on observed behaviour rather than on an edited string.
    #[test]
    fn changed_observed_catalogue_refuses_bound_target_before_commit() {
        let (catalog, _) = captured_catalog_and_xml();
        let binding = catalog.bind_selected([RENAMED_FROM.to_owned()]).unwrap();
        assert!(require_current_catalog_binding(&binding, &catalog).is_ok());

        let renamed = parse_standard_ledger_catalog_with_identities(
            &captured_renamed_catalog_xml(),
            CAPTURED_COMPANY,
            CAPTURED_GUID,
        )
        .expect("captured renamed catalogue remains parser-admitted");
        assert_eq!(
            require_current_catalog_binding(&binding, &renamed)
                .unwrap_err()
                .code,
            "source_draft_catalogue_target_changed"
        );
    }
}
