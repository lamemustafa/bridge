//! XML request builders for Tally's native Bills Receivable/Payable reports
//! plus `List of Ledgers` and `List of Groups` collection snapshots.
//!
//! These render exact request strings; nothing in this module dispatches
//! them. The Bills Receivable/Payable shape is the WORKING shape verified
//! live against TallyPrime (TALLY_PROTOCOL_REFERENCE ground truth captured
//! 2026-08-07): `SVTODATE` controls the report's as-of date and must always
//! be present.

use bridge_tally_primitives::TallyDate;

use crate::outstandings_shared::DateBoundaryProfile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBillsReportKind {
    Receivable,
    Payable,
}

/// A master export period whose opening boundary has been admitted by the
/// endpoint's compatibility profile.
///
/// `OPENINGBALANCE` is scoped by `SVFROMDATE`, so `from` must be the
/// profile-supported `BOOKSFROM`; otherwise Education mode can silently
/// substitute its display period. The export does not fetch
/// `CLOSINGBALANCE`, whose as-of semantics would require proving `SVTODATE`.
/// `to` is therefore retained only as the observed `LASTVOUCHERDATE` needed
/// to form a non-inverted request range, and may be an ordinary calendar day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLedgerExportPeriod {
    from: TallyDate,
    to: TallyDate,
}

impl NativeLedgerExportPeriod {
    pub fn new(
        boundary_profile: DateBoundaryProfile,
        from: TallyDate,
        to: TallyDate,
    ) -> Result<Self, NativeLedgerExportPeriodError> {
        if from > to {
            return Err(NativeLedgerExportPeriodError::InvalidRange);
        }
        if !boundary_profile.accepts_boundary(&from) {
            return Err(NativeLedgerExportPeriodError::UnsupportedBoundary);
        }
        Ok(Self { from, to })
    }

    pub fn from(&self) -> &TallyDate {
        &self.from
    }

    pub fn to(&self) -> &TallyDate {
        &self.to
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLedgerExportPeriodError {
    InvalidRange,
    UnsupportedBoundary,
}

/// A ledger snapshot period whose opening and as-of boundaries have both been
/// admitted by the endpoint's compatibility profile.
///
/// `CLOSINGBALANCE` is as-of scoped, unlike the export's `OPENINGBALANCE`.
/// TALLY_PROTOCOL_REFERENCE §7 (corrected 2026-08-24) records two production
/// collection shapes whose balances changed with `SVTODATE`; a silently
/// refused `to` boundary can therefore produce a plausible but wrong residual.
/// This is intentionally distinct from [`NativeLedgerExportPeriod`], whose
/// `to` is safe to leave ordinary because that request does not fetch
/// `CLOSINGBALANCE`.
///
/// This Collection also returns a byte-identical empty `STATUS 1` response for
/// a closed company and a nonexistent company (measured 2026-08-24: 2,994
/// bytes, zero rows). The enclosing GUID-pinned extent read currently rejects
/// a closed company before this request runs; that independent guard does not
/// establish this request's period semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLedgerSnapshotPeriod {
    from: TallyDate,
    to: TallyDate,
}

impl NativeLedgerSnapshotPeriod {
    pub fn new(
        boundary_profile: DateBoundaryProfile,
        from: TallyDate,
        to: TallyDate,
    ) -> Result<Self, NativeLedgerSnapshotPeriodError> {
        if from > to {
            return Err(NativeLedgerSnapshotPeriodError::InvalidRange);
        }
        if !boundary_profile.accepts_boundary(&from) || !boundary_profile.accepts_boundary(&to) {
            return Err(NativeLedgerSnapshotPeriodError::UnsupportedBoundary);
        }
        Ok(Self { from, to })
    }

    pub fn from(&self) -> &TallyDate {
        &self.from
    }

    pub fn to(&self) -> &TallyDate {
        &self.to
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeLedgerSnapshotPeriodError {
    InvalidRange,
    UnsupportedBoundary,
}

impl NativeBillsReportKind {
    const fn report_id(self) -> &'static str {
        match self {
            Self::Receivable => "Bills Receivable",
            Self::Payable => "Bills Payable",
        }
    }
}

/// Renders the exact working request shape for the flat Bills
/// Receivable/Payable `Data` report.
pub fn render_native_bills_request(
    kind: NativeBillsReportKind,
    company: &str,
    from: &TallyDate,
    to: &TallyDate,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Data</TYPE><ID>{id}</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES></DESC></BODY></ENVELOPE>"#,
        id = kind.report_id(),
        company = xml_escape(company),
        from = from.as_str(),
        to = to.as_str(),
    )
}

/// Renders a request for the `List of Ledgers` collection, overridden to
/// fetch exactly the fields the on-account residual computation needs, plus
/// Tally's computed `BRIDGECOMPANYGUID` so the party/ledger export can bind
/// this specific response to its selected company: `NAME`, `PARENT`, `CLOSINGBALANCE`,
/// `OPENINGBALANCE`, `ISBILLWISEON`, and `CURRENCYNAME`, the ledger's own
/// currency (the Currency master's NAME, e.g. `I₹`, `Rs.` or `$`; bridge#551).
/// A foreign-currency ledger's bills and a zero foreign balance arrive as
/// plain amounts, so only this field tells such a ledger from a base one.
/// Measured on 7.1 (TALLY_PROTOCOL_REFERENCE §8.2d): present on every row of
/// every book read; on the FOREX book, the response was otherwise
/// byte-identical to the same request without the field.
///
/// **`SVFROMDATE`/`SVTODATE` are load-bearing here and must match the bills
/// request exactly.** `CLOSINGBALANCE` is as-of scoped; see
/// TALLY_PROTOCOL_REFERENCE §7 (corrected 2026-08-24). The bills reports are
/// as-of scoped too, so if this request omitted the period the residual
/// `CLOSINGBALANCE - sum(BILLCL)` would subtract historical bills from a
/// current balance and silently report a wrong on-account figure at every
/// as-of except today's -- the failure would be invisible in a test that only
/// ever asks for now.
///
/// (An earlier revision of this function omitted the period and appeared
/// correct precisely because it was only exercised at the current date.)
pub fn render_native_ledger_snapshot_request(
    company: &str,
    period: &NativeLedgerSnapshotPeriod,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>NAME, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON, CURRENCYNAME</FETCH><COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = period.from().as_str(),
        to = period.to().as_str(),
    )
}

/// Renders a native `List of Ledgers` collection for ordinary ledger export.
///
/// This mirrors the retired report's fetch list exactly, but reads the master
/// values directly rather than rendering them through a report `FIELD`.
/// `OPENINGBALANCE` is load-bearingly pinned to the company's own book range:
/// measured on 2026-08-21, omitting the period returned the opening at the
/// current loaded display period, while `SVFROMDATE=BOOKSFROM` returns the
/// ledger master's own opening. See TALLY_PROTOCOL_REFERENCE §5.5 for the
/// discriminating live observations and confidence boundary. Callers must
/// supply a `NativeLedgerExportPeriod`, which binds the validated book extent
/// to the endpoint compatibility profile before any ledger request is sent.
pub fn render_native_ledger_export_request(
    company: &str,
    period: &NativeLedgerExportPeriod,
) -> String {
    render_native_ledger_collection_request(
        company,
        period,
        "NAME, GUID, REMOTEID, MASTERID, ALTERID, PARENT, PARTYGSTIN, OPENINGBALANCE",
        false,
    )
}

/// Renders the dedicated collection used only by the party/ledger master
/// workbook and compliance readers. Sensitive master values are fetched here;
/// ordinary ledger readers use `render_native_ledger_export_request`.
pub fn render_party_ledger_master_request(
    company: &str,
    period: &NativeLedgerExportPeriod,
) -> String {
    render_native_ledger_collection_request(
        company,
        period,
        "NAME, GUID, REMOTEID, MASTERID, ALTERID, PARENT, PARTYGSTIN, INCOMETAXNUMBER, NAMEONPAN, LEDPINCODE, LEDGSTPINCODE, MSMEREGNUMBER, LEDUDYAMREGNUMBER, BANKACCHOLDERNAME, BANKDETAILS, IFSCODE, EMAIL, LEDGERPHONE, STATENAME, LEDADDRESS.LIST, TAXTYPE, GSTDUTYHEAD, OPENINGBALANCE",
        true,
    )
}

fn render_native_ledger_collection_request(
    company: &str,
    period: &NativeLedgerExportPeriod,
    fetch: &str,
    include_response_company_guid: bool,
) -> String {
    let response_company_guid = if include_response_company_guid {
        "<COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE>"
    } else {
        ""
    };
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>{fetch}</FETCH>{response_company_guid}</COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = period.from().as_str(),
        to = period.to().as_str(),
        fetch = fetch,
        response_company_guid = response_company_guid,
    )
}

/// Renders a native `List of VoucherTypes` collection. The identity fields are
/// master data, so no period variables are applied.
pub fn render_native_voucher_type_export_request(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of VoucherTypes</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of VoucherTypes" ISMODIFY="Yes"><TYPE>VoucherType</TYPE><FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
    )
}

/// Filter name for the voucher window predicate below. TALLY_PROTOCOL_REFERENCE
/// section 5.1 (VERIFIED) established that `SVFROMDATE`/`SVTODATE` alone do
/// not bound collection membership -- they select which display period Tally
/// loads, not which rows match, and a collection with no date variables
/// returns only the current display period. Section 5.2 (VERIFIED) established
/// the `<SYSTEM TYPE="Formulae">` / `<FILTERS>` mechanism as the fix.
///
/// Section 5.3 (VERIFIED, 23 data points) supersedes comparing `$Date`
/// against `##SVFromDate`/`##SVToDate`: a refused period boundary silently
/// widens the period, and with a predicate that depends on `##SVToDate` the
/// widened window then resolves to zero rows -- a response byte-identical
/// (same sha256) to a genuinely empty window. Measured live on WR2 Unicode
/// Lab: `SVTODATE=20260830` (day 30, refused) with the `##SVFromDate`/
/// `##SVToDate` predicate returned 0 rows for a 3-voucher window; the same
/// refused boundary with literal dates in the predicate (below) returned all
/// 3 rows, correctly dated, while still returning 0 for genuinely empty and
/// out-of-range windows. The predicate below therefore compares `$Date`
/// against literal `$$Date:"YYYYMMDD"` bounds built from this function's own
/// `from`/`to` arguments instead of `##SVFromDate`/`##SVToDate`.
/// `SVFROMDATE`/`SVTODATE` are still sent in `STATICVARIABLES` -- harmless,
/// and matching existing precedent -- but no longer participate in the
/// predicate.
///
/// No spaces: section 6.1 (VERIFIED) established that a `$$` function
/// argument containing a space terminates the Tally process, and this
/// repository's hazard gate (`scripts/check-tally-request-builder-hazards.mjs`)
/// fails the build on any such argument. `$$Date:"YYYYMMDD"` is a quoted
/// argument whose contents are digits only, so it carries no space.
const VOUCHER_WINDOW_FILTER_NAME: &str = "BridgeVoucherWindowFilter";

/// Renders a native Voucher collection with the dotted entry fields Tally
/// requires to include accounting rows. `SVFROMDATE` and `SVTODATE` scope the
/// export and must remain paired with the requested canonical window, but per
/// TALLY_PROTOCOL_REFERENCE section 5.1 they do not by themselves bound which
/// rows come back -- the `<FILTERS>` predicate below does that, using literal
/// dates per section 5.3 (see `VOUCHER_WINDOW_FILTER_NAME` doc comment).
///
/// `from`/`to` are interpolated into the `<SYSTEM TYPE="Formulae">` predicate
/// as **quoted `$$Date:"..."` arguments**, not as ordinary XML character
/// data. `xml_escape` is not sufficient there: it turns a literal `"` into
/// `&quot;`, but Tally's XML parser decodes `&quot;` back into a literal `"`
/// before the formula text is evaluated, so an escaped quote can still close
/// the quoted argument and inject arbitrary TDL into the formula. The fix is
/// therefore not escaping but a closed input alphabet: `from`/`to` are
/// required to already be validated `TallyDate`s -- exactly 8 ASCII digits,
/// `YYYYMMDD` -- so no byte that could terminate the quoted argument (a `"`,
/// whitespace, or any non-ASCII-digit character) can ever reach the formula.
/// `SVFROMDATE`/`SVTODATE` remain ordinary XML character content, where
/// `TallyDate`'s digit-only contents are trivially safe either way.
pub fn render_native_voucher_export_request(
    company: &str,
    from: &TallyDate,
    to: &TallyDate,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>BridgeVoucherExport</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE="Formulae" NAME="{filter}">$Date &gt;= $$Date:"{from}" AND $Date &lt;= $$Date:"{to}"</SYSTEM><COLLECTION NAME="BridgeVoucherExport" ISMODIFY="No"><TYPE>Voucher</TYPE><FETCH>DATE, GUID, MASTERID, ALTERID, VOUCHERTYPENAME, VOUCHERNUMBER, ISCANCELLED, ISOPTIONAL, ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH><FILTERS>{filter}</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = from.as_str(),
        to = to.as_str(),
        filter = VOUCHER_WINDOW_FILTER_NAME,
    )
}

/// Renders a request for the `List of Groups` collection, overridden to fetch
/// the complete group ancestry and durable master identity the core reader
/// needs. `GUID`, `MASTERID`, and `ALTERID` were captured from both supported
/// Tally Education books; mutable `NAME` is never an identity fallback.
/// `RESERVEDNAME` is explicitly requested too, so predefined-party
/// classification survives a user renaming "Sundry Debtors"/"Sundry
/// Creditors" -- measured live, Tally already emits `RESERVEDNAME` as a row
/// attribute even without being asked (both `group_snapshot_wr2.xml` and the
/// pre-GUID-widening `group_snapshot_aarav.xml` fixture carry it on every
/// row), but it is listed here anyway, the same way `NAME` is listed despite
/// being emitted unconditionally too: this FETCH is the explicit contract of
/// what the reader depends on, not merely what happens to already arrive.
/// `BRIDGECOMPANYGUID` is a response-bound value computed from the selected
/// company, so every consumed Group row can prove which company answered.
///
/// Unlike the legacy export profile, this stays in Tally's native Collection
/// family: it defines no report/form/part/line/field stack and invokes no TDL
/// function. Paired byte-identical reads plus the enclosing book-extent
/// bracket establish completeness for this snapshot.
pub fn render_native_group_snapshot_request(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Groups</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Groups" ISMODIFY="Yes"><FETCH>NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME</FETCH><COMPUTE>BRIDGECOMPANYGUID:$GUID:Company:##SVCurrentCompany</COMPUTE></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Renders a request for the company's currency masters.
///
/// A company's base currency is a fact Tally holds, so asking the operator to
/// assert it is a step the product can answer for itself. Measured
/// 2026-08-07 on three lab companies: one `CURRENCY` row each, `NAME` `"Rs."`,
/// `MAILINGNAME` `"Indian Rupees"` or `"INR"`.
pub fn render_company_currency_request(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>BridgeCompanyCurrencies</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="BridgeCompanyCurrencies" ISMODIFY="No"><TYPE>Currency</TYPE><FETCH>NAME, MAILINGNAME, DECIMALPLACES</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
    )
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
