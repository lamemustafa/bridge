use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{ExactDecimal, TallyDate};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StructuredImportError {
    #[error("structured import JSON exceeds the bounded limit")]
    InputTooLarge,
    #[error("structured import JSON is invalid")]
    InvalidJson,
    #[error("structured import contract version is unsupported")]
    UnsupportedContractVersion,
    #[error("structured import contains no rows")]
    EmptyRows,
    #[error("structured import contains too many rows")]
    TooManyRows,
    #[error("structured import company identity is invalid")]
    InvalidCompanyIdentity,
    #[error("structured import ledger catalog is invalid")]
    InvalidLedgerCatalog,
    #[error("structured import source evidence is incomplete or mismatched")]
    InvalidSourceEvidence,
    #[error("structured import settlement ledger is invalid")]
    InvalidSettlementLedger,
    #[error("structured import ledger mapping is invalid")]
    InvalidLedgerMapping,
    #[error("structured import ledger mapping is stale")]
    StaleLedgerMapping,
    #[error("structured import row identity is invalid at ordinal {ordinal}")]
    InvalidRowIdentity { ordinal: usize },
    #[error("structured import row identity is duplicated at ordinal {ordinal}")]
    DuplicateRowIdentity { ordinal: usize },
    #[error("structured import row amount must be positive at ordinal {ordinal}")]
    NonPositiveAmount { ordinal: usize },
    #[error(
        "structured import row date is outside the allowed source window at ordinal {ordinal}"
    )]
    VoucherDateOutsideAllowedWindow { ordinal: usize },
    #[error("structured import row text is invalid at ordinal {ordinal}")]
    InvalidRowText { ordinal: usize },
    #[error("structured import row has no exact ledger mapping at ordinal {ordinal}")]
    UnknownLedgerMapping { ordinal: usize },
    #[error("structured import planning serialization failed")]
    Serialization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoucherKind {
    Payment,
    Receipt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostingSide {
    Debit,
    Credit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DryRunState {
    NotDispatched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchAuthority {
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchPrecondition {
    ExactVoucherTypeIdentityUnverified,
    ManualNumberingPreflightUnverified,
    PreventDuplicatesPreflightUnverified,
    CompanyBookPeriodAcceptanceUnverified,
    TallyModeDateAcceptanceUnverified,
    XmlPayloadNotRendered,
    WriteReadbackNotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedLedger {
    pub(super) ledger_guid: String,
    pub(super) exact_name: String,
}

impl PlannedLedger {
    pub fn ledger_guid(&self) -> &str {
        &self.ledger_guid
    }

    pub fn exact_name(&self) -> &str {
        &self.exact_name
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedPosting {
    pub(super) ledger: PlannedLedger,
    pub(super) side: PostingSide,
    pub(super) amount: ExactDecimal,
}

impl PlannedPosting {
    pub fn ledger(&self) -> &PlannedLedger {
        &self.ledger
    }

    pub fn side(&self) -> PostingSide {
        self.side
    }

    pub fn amount(&self) -> &ExactDecimal {
        &self.amount
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedVoucher {
    pub(super) source_row_id: String,
    pub(super) row_sha256: String,
    pub(super) voucher_kind: VoucherKind,
    pub(super) date: TallyDate,
    pub(super) narration: Option<String>,
    pub(super) postings: Vec<PlannedPosting>,
    pub(super) debits_equal_credits: bool,
}

impl PlannedVoucher {
    pub fn source_row_id(&self) -> &str {
        &self.source_row_id
    }

    pub fn row_sha256(&self) -> &str {
        &self.row_sha256
    }

    pub fn voucher_kind(&self) -> VoucherKind {
        self.voucher_kind
    }

    pub fn date(&self) -> &TallyDate {
        &self.date
    }

    pub fn narration(&self) -> Option<&str> {
        self.narration.as_deref()
    }

    pub fn postings(&self) -> &[PlannedPosting] {
        &self.postings
    }

    pub fn debits_equal_credits(&self) -> bool {
        self.debits_equal_credits
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructuredImportManifest {
    pub(super) contract_version: u16,
    pub(super) dry_run_state: DryRunState,
    pub(super) dispatch_authority: DispatchAuthority,
    pub(super) unresolved_dispatch_preconditions: Vec<DispatchPrecondition>,
    pub(super) company_guid: String,
    pub(super) source_run_id: String,
    pub(super) source_snapshot_sha256: String,
    pub(super) input_sha256: String,
    pub(super) ledger_catalog_sha256: String,
    pub(super) mapping_sha256: String,
    pub(super) vouchers: Vec<PlannedVoucher>,
}

impl StructuredImportManifest {
    pub fn contract_version(&self) -> u16 {
        self.contract_version
    }

    pub fn dry_run_state(&self) -> DryRunState {
        self.dry_run_state
    }

    pub fn dispatch_authority(&self) -> DispatchAuthority {
        self.dispatch_authority
    }

    pub fn unresolved_dispatch_preconditions(&self) -> &[DispatchPrecondition] {
        &self.unresolved_dispatch_preconditions
    }

    pub fn company_guid(&self) -> &str {
        &self.company_guid
    }

    pub fn source_run_id(&self) -> &str {
        &self.source_run_id
    }

    pub fn source_snapshot_sha256(&self) -> &str {
        &self.source_snapshot_sha256
    }

    pub fn input_sha256(&self) -> &str {
        &self.input_sha256
    }

    pub fn ledger_catalog_sha256(&self) -> &str {
        &self.ledger_catalog_sha256
    }

    pub fn mapping_sha256(&self) -> &str {
        &self.mapping_sha256
    }

    pub fn vouchers(&self) -> &[PlannedVoucher] {
        &self.vouchers
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructuredImportPlan {
    pub(super) manifest_sha256: String,
    pub(super) manifest: StructuredImportManifest,
}

impl StructuredImportPlan {
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub fn manifest(&self) -> &StructuredImportManifest {
        &self.manifest
    }
}

pub(super) fn hash_serializable(
    domain: &[u8],
    value: &impl Serialize,
) -> Result<String, StructuredImportError> {
    let canonical = serde_json::to_vec(value).map_err(|_| StructuredImportError::Serialization)?;
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(canonical);
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub(super) fn hash_bytes(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
