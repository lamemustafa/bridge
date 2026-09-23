//! Native `Bills Receivable`/`Bills Payable` + `List of Ledgers` outstandings
//! path.
//!
//! This is a second, independent way to reach [`crate::outstandings_shared::OutstandingsReport`]:
//! instead of scanning vouchers, it reads Tally's own bill-level reports
//! directly. Everything here was measured live against TallyPrime
//! (TALLY_PROTOCOL_REFERENCE ground truth captured 2026-08-07) and is
//! documented at each module:
//!
//! - [`request`] — exact request XML for both native reports.
//! - [`date`] — Tally's `D-MMM-YY` display dates: a bill date resolved in the
//!   pinned company's book window, a due date against its bill date.
//! - [`wire`] — the flat, inverted-`STATUS` Bills grammar and the
//!   `DATA`-scoped Ledger collection grammar (`CMPINFO` counter trap).
//! - [`model`] — row and result types; reuses `OutstandingsReport` so this
//!   path is a drop-in for the UI.
//! - [`compute`] — assembles the report and the on-account residual
//!   cross-check.

mod compute;
mod date;
mod ledger_currency;
mod model;
mod request;
mod wire;

pub use compute::{
    age_in_days, compute_native_outstandings, compute_native_outstandings_with_exclusions,
    NativeGroupSnapshot, NativeMasterSnapshot,
};
pub use date::{parse_native_bill_date, parse_native_due_date};
pub use ledger_currency::{
    classify_ledger_currencies, BaseCurrencyName, ForeignCurrencyLedger, LedgerCurrencies,
    LedgerCurrencyRefusal,
};
pub use model::{
    AgeingAnchor, CompanyCurrency, CurrencyMaster, LedgerSnapshotEntry, NativeBillRow,
    NativeOutstandingsError, NativeOutstandingsResult, NativeOverdueCrosscheck, PartyResidual,
};
pub use request::{
    render_company_currency_request, render_native_bills_request,
    render_native_group_snapshot_request, render_native_ledger_export_request,
    render_native_ledger_snapshot_request, render_native_voucher_export_request,
    render_native_voucher_type_export_request, render_party_ledger_master_request,
    NativeBillsReportKind, NativeLedgerExportPeriod, NativeLedgerExportPeriodError,
    NativeLedgerSnapshotPeriod, NativeLedgerSnapshotPeriodError,
};
pub use wire::{
    parse_company_currency, parse_currency_masters, parse_native_bill_rows,
    parse_native_group_snapshot, parse_native_group_snapshot_with_evidence,
    parse_native_ledger_snapshot, parse_native_ledger_snapshot_classified,
    parse_native_ledger_snapshot_for_company, ClassifiedLedgerSnapshot, NativeGroupSnapshotEntry,
};
