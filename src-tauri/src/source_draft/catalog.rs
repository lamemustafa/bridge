//! Ephemeral existing-ledger selections for one open source draft.
//!
//! Opaque catalog identities and selected master bindings are never serialized.
//! Only binding coordinates justified by the latest capture reach the desktop,
//! so it can avoid claiming that a saved proposal is current.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bridge_tally_protocol::{StandardLedgerCatalog, StandardLedgerCatalogBinding};

use crate::{
    commands::SelectedCompanyIdentity,
    tally::{
        standard_ledger_catalog::{StandardLedgerCatalogRead, StandardLedgerCatalogReadError},
        EndpointKey, TallyConfig, TallyRuntime, VerifiedCompanyIdentity,
    },
};

use super::{
    dto, error,
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
    pub(crate) evidence: SourceDraftCatalogEvidence,
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

pub(super) fn require_current_catalog_binding(
    binding: &StandardLedgerCatalogBinding,
    fresh_body: &str,
    identity: &VerifiedCompanyIdentity,
) -> CommandResult<()> {
    let still_current = binding
        .matches(fresh_body, identity.display_name(), identity.company_guid())
        .map_err(|cause| error(StandardLedgerCatalogReadError::from(cause).command_code()))?;
    if still_current {
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
    require_current_catalog_binding(&binding, &fresh.body, &identity)?;
    store.commit_catalog_target(snapshot, request, binding, &fresh.body)
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
        mut request: SourceDraftCatalogApplyRequest,
        binding: StandardLedgerCatalogBinding,
        fresh_body: &str,
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
        let row = request.row_position - 1;
        let entry = request.entry_position - 1;
        request.proposals[row].entries[entry].ledger = Some(request.target_name.clone());
        validate_proposals(&current.source, &request.proposals)?;
        let next_revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| error("source_draft_revision_exhausted"))?;
        SourceDraftStore::revalidate_retained_bindings(current, fresh_body);
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
    /// fresh response still contains its exact observed name and GUID. A
    /// malformed or otherwise doubtful match is not currency evidence.
    fn revalidate_retained_bindings(active: &mut ActiveDraft, fresh_body: &str) {
        let Some(capture) = active.catalog.as_mut() else {
            return;
        };
        let identity = capture.identity.clone();
        capture.bindings.retain(|_, selected| {
            selected
                .binding
                .matches(fresh_body, identity.display_name(), identity.company_guid())
                .unwrap_or(false)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        source_draft::{dto, empty_proposals, files::serialize_draft},
        source_draft_xml::parse_source_xml,
    };
    use bridge_tally_protocol::parse_standard_ledger_catalog_with_identities;
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    const CAPTURED_COMPANY: &str = "WR2 Unicode Lab";
    const CAPTURED_GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

    fn captured_catalog_and_xml() -> (StandardLedgerCatalog, String) {
        let bytes = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue.utf16le.xml"
        );
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let xml = String::from_utf16(&words).expect("captured catalogue is UTF-16LE");
        let catalog =
            parse_standard_ledger_catalog_with_identities(&xml, CAPTURED_COMPANY, CAPTURED_GUID)
                .expect("captured catalogue remains parser-admitted");
        (catalog, xml)
    }

    /// The ledger renamed between the two captured responses below.
    const RENAMED_FROM: &str = "WR2 Sales";
    const RENAMED_TO: &str = "WR2 Sales Renamed";

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
        let bytes = include_bytes!(
            "../../crates/bridge-tally-protocol/tests/fixtures/agent/native-ledger-catalogue-renamed.utf16le.xml"
        );
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&words).expect("captured renamed catalogue is UTF-16LE")
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
        let (id, capture_id, names, xml) = install_active_catalog(&store);
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
            .commit_catalog_target(snapshot, request, second_binding, &xml)
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
        let (catalog, body) = captured_catalog_and_xml();
        let read = StandardLedgerCatalogRead {
            catalog,
            body,
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
        let (catalog, body) = captured_catalog_and_xml();
        assert_eq!(
            store
                .install_catalog(
                    snapshot,
                    EndpointKey::from_config(&TallyConfig::default()).unwrap(),
                    VerifiedCompanyIdentity::test_fixture(CAPTURED_COMPANY, CAPTURED_GUID),
                    StandardLedgerCatalogRead {
                        catalog,
                        body,
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
        let before =
            serde_json::to_vec(&store.active.lock().unwrap().as_ref().unwrap().proposals).unwrap();
        store.invalidate_catalogue().unwrap();
        assert_eq!(
            store
                .commit_catalog_target(snapshot, request, binding, "")
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
        store.replace(replacement).unwrap();
        assert_eq!(
            store
                .commit_catalog_target(snapshot, request, binding, "")
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
        for value in [&response, &saved] {
            let object = value.as_object().unwrap();
            assert!(!object.contains_key("catalog"));
            assert!(!object.contains_key("capture_id"));
            assert!(!object.contains_key("binding"));
            assert!(!object.contains_key("company_guid"));
        }
    }

    #[test]
    fn changed_observed_catalogue_refuses_bound_target_before_commit() {
        let (catalog, xml) = captured_catalog_and_xml();
        let name = catalog.names().next().unwrap().to_owned();
        let binding = catalog.bind_selected([name.clone()]).unwrap();
        let identity = VerifiedCompanyIdentity::test_fixture(CAPTURED_COMPANY, CAPTURED_GUID);
        assert!(require_current_catalog_binding(&binding, &xml, &identity).is_ok());
        let expected = format!("NAME=\"{name}\"");
        assert_eq!(
            xml.matches(&expected).count(),
            1,
            "captured target occurs once"
        );
        let changed = xml.replacen(&expected, "NAME=\"renamed before apply\"", 1);
        assert_ne!(changed, xml, "control changes captured response in memory");
        assert_eq!(
            require_current_catalog_binding(&binding, &changed, &identity)
                .unwrap_err()
                .code,
            "source_draft_catalogue_target_changed"
        );
    }
}
