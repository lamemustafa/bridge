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
    pub(crate) date: Option<String>,
    pub(crate) voucher_type: Option<SourceDraftVoucherType>,
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
    pub(crate) ledger: Option<String>,
    pub(crate) side: Option<SourceDraftSide>,
    pub(crate) amount: Option<String>,
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
            "The selected XML is not a supported source export.",
            "Choose the original source XML without modifying it.",
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
