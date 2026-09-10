use serde::{Deserialize, Serialize};

use crate::source_draft_xml::SourceXmlError;

pub(super) const MAX_DRAFT_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_PROPOSAL_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_TEXT_BYTES: usize = 4_096;

#[derive(Debug, Serialize)]
pub(crate) struct SourceDraftCommandError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) remediation: &'static str,
}

pub(super) type CommandResult<T> = Result<T, SourceDraftCommandError>;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceDraftDto {
    pub(crate) draft_id: String,
    pub(crate) revision: u64,
    pub(crate) source_filename: String,
    pub(crate) source_sha256: String,
    pub(crate) source_notices: Vec<SourceDraftSourceNotice>,
    pub(crate) rows: Vec<SourceDraftRow>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceDraftSourceNotice {
    pub(crate) kind: String,
    pub(crate) count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceDraftRow {
    pub(crate) position: usize,
    pub(crate) source_remote_id: String,
    pub(crate) source_date: String,
    pub(crate) source_voucher_type: String,
    pub(crate) source_narration: Option<String>,
    pub(crate) entries: Vec<SourceDraftEntry>,
    pub(crate) source_omitted_fields: Vec<String>,
    pub(crate) proposal: SourceDraftProposal,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceDraftEntry {
    pub(crate) position: usize,
    pub(crate) source_ledger: String,
    pub(crate) source_amount: String,
    pub(crate) source_polarity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceDraftProposal {
    #[serde(default, deserialize_with = "deserialize_optional_text")]
    pub(crate) date: Option<String>,
    pub(crate) voucher_type: Option<SourceDraftVoucherType>,
    #[serde(default, deserialize_with = "deserialize_optional_text")]
    pub(crate) narration: Option<String>,
    pub(crate) notes: String,
    pub(crate) entries: Vec<SourceDraftEntryProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) enum SourceDraftVoucherType {
    Payment,
    Receipt,
    Journal,
    Contra,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceDraftEntryProposal {
    #[serde(default, deserialize_with = "deserialize_optional_text")]
    pub(crate) ledger: Option<String>,
    pub(crate) side: Option<SourceDraftSide>,
    #[serde(default, deserialize_with = "deserialize_optional_text")]
    pub(crate) amount: Option<String>,
}

fn deserialize_optional_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(|value| value.filter(|text| !text.is_empty()))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum SourceDraftSide {
    Dr,
    Cr,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceDraftSaveRequest {
    pub(crate) draft_id: String,
    pub(crate) revision: u64,
    pub(crate) proposals: Vec<SourceDraftProposal>,
}

pub(super) fn error(code: &'static str) -> SourceDraftCommandError {
    let (message, remediation) = match code {
        "source_draft_revision_conflict" => (
            "This draft changed while it was open.",
            "Review the current draft, then save again.",
        ),
        "source_draft_file_too_large" | "source_draft_source_too_large" => (
            "The selected file is too large for a source draft.",
            "Choose a smaller supported local file.",
        ),
        "source_draft_xml_shape_unsupported" => (
            "The selected XML is not a supported voucher-import document.",
            "Choose the original source XML without modifying it.",
        ),
        "source_draft_notice_limit_exceeded" => (
            "The selected XML has too many distinct non-voucher record types.",
            "Choose a supported source XML with fewer non-voucher record types.",
        ),
        "source_draft_omitted_field_limit_exceeded" => (
            "A source row has too many distinct omitted field names.",
            "Choose a supported source XML with fewer distinct fields per voucher.",
        ),
        "source_draft_lifecycle_request_not_pending" => (
            "The requested close action is no longer pending.",
            "Continue editing or request the close action again.",
        ),
        "source_draft_lifecycle_unavailable" => (
            "Bridge could not complete the requested native close action.",
            "Continue editing and try the close action again.",
        ),
        "source_draft_catalogue_scope_invalid" => (
            "Bridge could not verify the current Tally company selection.",
            "Check Tally and select the intended current company, then load existing ledgers again.",
        ),
        "source_draft_catalogue_invalidated" => (
            "The existing-ledger capture is no longer current for this draft or company.",
            "Load existing ledgers again before selecting a target.",
        ),
        "source_draft_catalogue_read_failed" => (
            "Bridge could not read a complete current existing-ledger list.",
            "Check Tally and retry the read; no target was applied.",
        ),
        "source_draft_catalogue_transport_failed" => (
            "Bridge could not reach Tally for the current existing-ledger list.",
            "Check Tally and retry the read; no target was applied.",
        ),
        "source_draft_catalogue_unstable" => (
            "The existing-ledger list changed while Bridge was reading it.",
            "Wait for Tally to settle, then load existing ledgers again; no target was applied.",
        ),
        "source_draft_catalogue_identity_mismatch" => (
            "Tally did not confirm the selected company for the existing-ledger list.",
            "Check Tally and select the intended current company, then load existing ledgers again.",
        ),
        "source_draft_catalogue_duplicate_identity" => (
            "Tally returned an ambiguous existing-ledger identity.",
            "Resolve the duplicate ledger identity in Tally, then load existing ledgers again.",
        ),
        "source_draft_catalogue_bounds_invalid" => (
            "The existing-ledger list exceeded a safety limit.",
            "Reduce the list or contact support; no target was applied.",
        ),
        "source_draft_catalogue_malformed_response" => (
            "Tally returned an unusable existing-ledger list.",
            "Check Tally and retry the read; no target was applied.",
        ),
        "source_draft_catalogue_target_invalid" => (
            "The requested target was not a valid current existing ledger.",
            "Choose a listed target and try again.",
        ),
        "source_draft_catalogue_target_changed" => (
            "The selected existing ledger changed before Bridge could apply it.",
            "Load existing ledgers again and make a fresh selection.",
        ),
        _ => (
            "Bridge could not prepare this source draft.",
            "Review the selected local file and try again.",
        ),
    };
    SourceDraftCommandError {
        code,
        message,
        remediation,
    }
}

pub(super) fn command_error(xml_error: SourceXmlError) -> SourceDraftCommandError {
    error(xml_error.code())
}
