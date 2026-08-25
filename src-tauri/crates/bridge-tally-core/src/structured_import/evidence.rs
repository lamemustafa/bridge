use std::collections::BTreeSet;

use super::context::{valid_identifier, valid_row_text};
use super::StructuredImportError;
use crate::{
    CanonicalPackWindow, CanonicalText, CoreAccountingBatch, RequestContext, SourceCountScope,
    SourceCountScopeDescriptor, SourceIdentityKind,
};

const MAX_GROUP_NAME_BYTES: usize = 1_000;

pub(super) fn validate_core_catalog_evidence(
    context: &RequestContext,
    window: &CanonicalPackWindow,
    core: &CoreAccountingBatch,
) -> Result<(), StructuredImportError> {
    window
        .validate_source_count_evidence()
        .map_err(|_| StructuredImportError::InvalidSourceEvidence)?;
    window
        .validate_record_evidence_binding()
        .map_err(|_| StructuredImportError::InvalidSourceEvidence)?;
    require_complete_master_count(context, window, "group", core.groups.len())?;
    require_complete_master_count(context, window, "ledger", core.ledgers.len())?;
    require_guid_provenance(window)?;
    validate_groups(core)
}

fn require_complete_master_count(
    context: &RequestContext,
    window: &CanonicalPackWindow,
    object_type: &'static str,
    record_count: usize,
) -> Result<(), StructuredImportError> {
    let descriptor = SourceCountScopeDescriptor {
        source_identity: context.company.identity.clone(),
        pack: context.pack,
        pack_schema_version: context.schema_version,
        object_type: CanonicalText::parse(object_type)
            .map_err(|_| StructuredImportError::InvalidSourceEvidence)?,
        query_profile: context.query_profile.clone(),
        filters_sha256: context.filters_sha256.clone(),
        window: None,
    };
    let matching = window
        .source_counts
        .as_deref()
        .ok_or(StructuredImportError::InvalidSourceEvidence)?
        .iter()
        .filter(|count| {
            count.object_type.as_str() == object_type
                && count.source_count_scope == SourceCountScope::Complete
        })
        .collect::<Vec<_>>();
    if matching.len() != 1
        || matching[0].source_reported_count != record_count as u64
        || !matching[0]
            .matches_scope_descriptor(&descriptor)
            .map_err(|_| StructuredImportError::InvalidSourceEvidence)?
    {
        return Err(StructuredImportError::InvalidSourceEvidence);
    }
    Ok(())
}

fn require_guid_provenance(window: &CanonicalPackWindow) -> Result<(), StructuredImportError> {
    let evidence = window
        .record_evidence
        .as_deref()
        .ok_or(StructuredImportError::InvalidSourceEvidence)?;
    if evidence.iter().any(|record| {
        matches!(record.object_type.as_str(), "group" | "ledger")
            && record.identity_kind != SourceIdentityKind::Guid
    }) {
        return Err(StructuredImportError::InvalidSourceEvidence);
    }
    Ok(())
}

fn validate_groups(core: &CoreAccountingBatch) -> Result<(), StructuredImportError> {
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    if core.groups.iter().any(|group| {
        !valid_identifier(&group.source_id)
            || !valid_row_text(&group.name, MAX_GROUP_NAME_BYTES)
            || !ids.insert(group.source_id.as_str())
            || !names.insert(group.name.as_str())
    }) {
        return Err(StructuredImportError::InvalidLedgerCatalog);
    }
    Ok(())
}
