mod completeness;
mod compute;
mod model;
mod parser;
mod request;
mod tolerant_xml;
mod wire;

pub use completeness::{
    assemble_partitioned_scan, assemble_scan, verify_empty_date_window_with_wider_pair,
    verify_empty_date_window_with_wider_pair_and_encoded_bytes,
    verify_empty_date_window_with_wider_pair_and_wire_evidence, verify_segment_pair,
    verify_segment_pair_with_encoded_bytes, verify_segment_pair_with_wire_evidence,
    SegmentWireEvidence,
};
pub use compute::compute_outstandings;
pub use model::{
    AgeingBillCounts, AgeingBuckets, AlterIdRange, BillAllocation, CompanyBookExtent, CompleteScan,
    CompleteSegment, DateBoundaryProfile, DateWindow, EmptyDateWindowVerification,
    EmptyDateWindowWitness, LedgerEntry, LedgerOpeningCoverage, MoneyValue, NarrowDateWindow,
    OutstandingsError, OutstandingsReport, PartialScan, PartyOutstanding, PinnedCompany,
    ScanResult, SegmentVerification, Voucher, VoucherAlterId, VoucherAlterIdHighWater,
};
pub use parser::{parse_company_book_extent, parse_ledger_opening_coverage};
pub(crate) use request::render_ledger_opening_coverage;
pub(crate) use request::{
    render_company_book_extent, render_outstandings_template, render_outstandings_vouchers,
};
pub use request::{voucher_outstandings_request, VoucherOutstandingsRequestXml};
