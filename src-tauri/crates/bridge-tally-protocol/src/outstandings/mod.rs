mod completeness;
mod compute;
mod model;
mod parser;
mod request;
mod wire;

pub use completeness::{
    assemble_partitioned_scan, assemble_scan, corroborate_empty_date_partition,
    nearest_non_empty_primary_partition, verify_empty_date_window_with_wider_pair,
    verify_empty_date_window_with_wider_pair_and_encoded_bytes,
    verify_empty_date_window_with_wider_pair_and_wire_evidence,
    verify_empty_partition_witness_pair_with_wire_evidence, verify_segment_pair,
    verify_segment_pair_with_encoded_bytes, verify_segment_pair_with_wire_evidence,
    SegmentWireEvidence,
};
pub use compute::{compute_outstandings, compute_outstandings_with_ageing_anchor};
pub use model::{
    AgeingAnchor, AlterIdRange, BillAllocation, BillReferenceKind, CompanyBookExtent, CompleteScan,
    CompleteSegment, CompleteWitnessPair, CorroboratedDatePartition, CreditPeriod,
    DateBoundaryProfile, DateWindow, EmptyDateWindowVerification, EmptyDateWindowWitness,
    EmptyPartitionControlProvenance, EmptyPartitionWitness, LedgerEntry, LedgerOpeningCoverage,
    MoneyValue, NarrowDateWindow, OutstandingsError, PartialScan, PinnedCompany, ScanResult,
    SegmentVerification, StrictlyWiderDateCover, Voucher, VoucherAlterId, VoucherAlterIdHighWater,
    WitnessPairVerification, WitnessVoucher,
};
// The shared report contract itself (`OutstandingsReport` and its
// constituents) lives ungated in `crate::outstandings_shared` because
// `native_outstandings` -- always compiled -- also produces it. Re-exported
// here too so existing `outstandings::` call sites inside this gated module
// keep working unchanged.
pub use crate::outstandings_shared::parse_company_book_extent;
pub use crate::outstandings_shared::{
    AgeingBillCounts, AgeingBuckets, OutstandingsReport, PartyOutstanding,
};
pub use parser::parse_ledger_opening_coverage;
pub(crate) use request::render_ledger_opening_coverage;
pub(crate) use request::{
    render_empty_partition_witness_template, render_outstandings_template,
    render_outstandings_vouchers,
};
pub use request::{
    voucher_empty_partition_witness_request, voucher_outstandings_request,
    VoucherEmptyPartitionWitnessRequestXml, VoucherOutstandingsRequestXml,
};
