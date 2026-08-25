//! Pure, non-dispatching planning for bounded Payment and Receipt imports.
//!
//! This module deliberately has no XML renderer, transport, outbox, approval,
//! or persistence dependency. A successful result is still only a dry-run.

mod context;
mod evidence;
mod mapping;
mod model;
mod planner;

pub use context::{CompanyLedgerCatalog, SettlementLedgerRole};
pub use mapping::{ImportLedgerMappingInput, ImportLedgerMappings};
pub use model::{
    DispatchAuthority, DispatchPrecondition, DryRunState, PlannedLedger, PlannedPosting,
    PlannedVoucher, PostingSide, StructuredImportError, StructuredImportManifest,
    StructuredImportPlan, VoucherKind,
};
pub use planner::plan_payment_receipt_json;

pub const STRUCTURED_IMPORT_CONTRACT_VERSION: u16 = 1;
pub const MAX_STRUCTURED_IMPORT_JSON_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_STRUCTURED_IMPORT_ROWS: usize = 10_000;
