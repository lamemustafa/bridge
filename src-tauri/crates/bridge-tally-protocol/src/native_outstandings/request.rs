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
/// `GUID` so the party/ledger export can bind this specific response to its
/// selected company: `NAME`, `GUID`, `PARENT`, `CLOSINGBALANCE`,
/// `OPENINGBALANCE`, `ISBILLWISEON`.
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
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>NAME, GUID, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
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
    )
}

/// Renders the dedicated collection used only by the party/ledger master
/// workbook. Sensitive master values are fetched here because this report
/// renders them; ordinary ledger readers use `render_native_ledger_export_request`.
pub fn render_party_ledger_master_request(
    company: &str,
    period: &NativeLedgerExportPeriod,
) -> String {
    render_native_ledger_collection_request(
        company,
        period,
        "NAME, GUID, REMOTEID, MASTERID, ALTERID, PARENT, PARTYGSTIN, INCOMETAXNUMBER, NAMEONPAN, LEDPINCODE, LEDGSTPINCODE, MSMEREGNUMBER, LEDUDYAMREGNUMBER, BANKACCHOLDERNAME, BANKDETAILS, IFSCODE, EMAIL, LEDGERPHONE, STATENAME, LEDADDRESS.LIST, OPENINGBALANCE",
    )
}

fn render_native_ledger_collection_request(
    company: &str,
    period: &NativeLedgerExportPeriod,
    fetch: &str,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>{fetch}</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = period.from().as_str(),
        to = period.to().as_str(),
        fetch = fetch,
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
///
/// Unlike the legacy export profile, this stays in Tally's native Collection
/// family: it defines no report/form/part/line/field stack and invokes no TDL
/// function. Paired byte-identical reads plus the enclosing book-extent
/// bracket establish completeness for this snapshot.
pub fn render_native_group_snapshot_request(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Groups</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Groups" ISMODIFY="Yes"><FETCH>NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
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
mod tests {
    use super::*;

    #[test]
    fn renders_the_verified_working_bills_request_shape() {
        let from = TallyDate::parse("20240401").unwrap();
        let to = TallyDate::parse("20260731").unwrap();
        let xml = render_native_bills_request(
            NativeBillsReportKind::Receivable,
            "Bridge Billwise Lab",
            &from,
            &to,
        );
        assert_eq!(
            xml,
            r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Data</TYPE><ID>Bills Receivable</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>Bridge Billwise Lab</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">20240401</SVFROMDATE><SVTODATE TYPE="Date">20260731</SVTODATE></STATICVARIABLES></DESC></BODY></ENVELOPE>"#
        );
        let payable = render_native_bills_request(
            NativeBillsReportKind::Payable,
            "Bridge Billwise Lab",
            &from,
            &to,
        );
        assert!(payable.contains("<ID>Bills Payable</ID>"));
    }

    #[test]
    fn master_export_period_validates_only_the_opening_balance_boundary() {
        let legal_books_from = TallyDate::parse("20240101").unwrap();
        let ordinary_last_voucher_date = TallyDate::parse("20240115").unwrap();
        assert_eq!(
            NativeLedgerExportPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                legal_books_from,
                ordinary_last_voucher_date,
            ),
            Ok(NativeLedgerExportPeriod {
                from: TallyDate::parse("20240101").unwrap(),
                to: TallyDate::parse("20240115").unwrap(),
            }),
            "Education mode accepts an ordinary LASTVOUCHERDATE because this export fetches OPENINGBALANCE, not CLOSINGBALANCE"
        );

        let illegal_books_from = TallyDate::parse("20240115").unwrap();
        let legal_book_end = TallyDate::parse("20240131").unwrap();
        assert_eq!(
            NativeLedgerExportPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                illegal_books_from.clone(),
                legal_book_end,
            ),
            Err(NativeLedgerExportPeriodError::UnsupportedBoundary),
            "the observed Education profile must reject an unsupported BOOKSFROM before a silently ignored opening-balance read"
        );
        assert_eq!(
            NativeLedgerExportPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                TallyDate::parse("20240201").unwrap(),
                TallyDate::parse("20240115").unwrap(),
            ),
            Err(NativeLedgerExportPeriodError::InvalidRange),
            "the range remains invalid even when its opening boundary is profile-supported"
        );
        assert!(
            NativeLedgerExportPeriod::new(
                DateBoundaryProfile::ModeAgnostic,
                illegal_books_from,
                TallyDate::parse("20260701").unwrap(),
            )
            .is_ok(),
            "licensed and unknown modes retain arbitrary calendar boundaries"
        );
    }

    #[test]
    fn ledger_snapshot_period_validates_both_closing_balance_boundaries() {
        let legal_from = TallyDate::parse("20240101").unwrap();
        let ordinary_to = TallyDate::parse("20240115").unwrap();
        assert_eq!(
            NativeLedgerSnapshotPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                legal_from.clone(),
                ordinary_to,
            ),
            Err(NativeLedgerSnapshotPeriodError::UnsupportedBoundary),
            "a snapshot CLOSINGBALANCE must not be requested with an Education-refused as-of boundary"
        );
        assert_eq!(
            NativeLedgerSnapshotPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                TallyDate::parse("20240115").unwrap(),
                TallyDate::parse("20240131").unwrap(),
            ),
            Err(NativeLedgerSnapshotPeriodError::UnsupportedBoundary),
            "the opening boundary remains independently required"
        );
        assert_eq!(
            NativeLedgerSnapshotPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                TallyDate::parse("20240201").unwrap(),
                TallyDate::parse("20240131").unwrap(),
            ),
            Err(NativeLedgerSnapshotPeriodError::InvalidRange),
            "inverted snapshot ranges remain invalid"
        );
        assert!(
            NativeLedgerSnapshotPeriod::new(
                DateBoundaryProfile::ModeAgnostic,
                legal_from,
                TallyDate::parse("20240115").unwrap(),
            )
            .is_ok(),
            "licensed and unknown modes retain arbitrary calendar boundaries"
        );
    }

    #[test]
    fn escapes_company_names_in_both_requests() {
        let from = TallyDate::parse("20240401").unwrap();
        let to = TallyDate::parse("20260731").unwrap();
        let xml = render_native_bills_request(
            NativeBillsReportKind::Receivable,
            "A & B <Co>",
            &from,
            &to,
        );
        assert!(xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(!xml.contains("A & B <Co>"));

        let snapshot_period = NativeLedgerSnapshotPeriod::new(
            DateBoundaryProfile::ModeAgnostic,
            from.clone(),
            to.clone(),
        )
        .expect("mode-agnostic profile accepts valid calendar dates");
        let ledger_xml = render_native_ledger_snapshot_request("A & B <Co>", &snapshot_period);
        assert!(ledger_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(ledger_xml.contains(
            "<FETCH>NAME, GUID, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON</FETCH>"
        ));
        assert!(ledger_xml.contains(r#"<SVFROMDATE TYPE="Date">20240401</SVFROMDATE>"#));
        assert!(ledger_xml.contains(r#"<SVTODATE TYPE="Date">20260731</SVTODATE>"#));

        let group_xml = render_native_group_snapshot_request("A & B <Co>");
        assert!(group_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(group_xml.contains(r#"<ID>List of Groups</ID>"#));
        assert!(group_xml
            .contains(r#"<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID, RESERVEDNAME</FETCH>"#));
        assert!(!group_xml.contains("<REPORT>"));
        assert!(!group_xml.contains("<FORM>"));
        assert!(!group_xml.contains("<PART>"));
        assert!(!group_xml.contains("<LINE>"));
        assert!(!group_xml.contains("<FIELD>"));
        assert!(!group_xml.contains("$$NumItems"));

        let export_period = NativeLedgerExportPeriod::new(
            DateBoundaryProfile::ModeAgnostic,
            from.clone(),
            to.clone(),
        )
        .expect("mode-agnostic profile accepts valid calendar dates");
        let export_xml = render_native_ledger_export_request("A & B <Co>", &export_period);
        assert!(export_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(!export_xml.contains("INCOMETAXNUMBER"));
        let party_master_xml = render_party_ledger_master_request("A & B <Co>", &export_period);
        assert!(party_master_xml.contains("INCOMETAXNUMBER, NAMEONPAN, LEDPINCODE, LEDGSTPINCODE, MSMEREGNUMBER, LEDUDYAMREGNUMBER, BANKACCHOLDERNAME, BANKDETAILS, IFSCODE, EMAIL, LEDGERPHONE, STATENAME, LEDADDRESS.LIST"));
        assert!(!export_xml.contains("<REPORT>"));
        assert!(!export_xml.contains("<FORM>"));
        assert!(!export_xml.contains("<PART>"));
        assert!(!export_xml.contains("<LINE>"));
        assert!(!export_xml.contains("<FIELD>"));
        assert!(!export_xml.contains("$$NumItems"));
        assert!(export_xml.contains(r#"<SVFROMDATE TYPE="Date">20240401</SVFROMDATE>"#));
        assert!(export_xml.contains(r#"<SVTODATE TYPE="Date">20260731</SVTODATE>"#));

        let voucher_type_xml = render_native_voucher_type_export_request("A & B <Co>");
        assert!(voucher_type_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(voucher_type_xml.contains("<ID>List of VoucherTypes</ID>"));
        assert!(voucher_type_xml.contains("<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH>"));
        assert!(!voucher_type_xml.contains("<REPORT>"));
        assert!(!voucher_type_xml.contains("$$NumItems"));

        let voucher_from = TallyDate::parse("20260401").unwrap();
        let voucher_to = TallyDate::parse("20260930").unwrap();
        let voucher_xml =
            render_native_voucher_export_request("A & B <Co>", &voucher_from, &voucher_to);
        assert!(voucher_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(voucher_xml.contains("<TYPE>Collection</TYPE>"));
        assert!(voucher_xml.contains("ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE"));
        assert!(!voucher_xml.contains("<REPORT>"));
        assert!(!voucher_xml.contains("<FORM>"));
        assert!(!voucher_xml.contains("<FIELD>"));
        assert!(!voucher_xml.contains("$$NumItems"));
    }

    /// SECURITY: `from`/`to` are interpolated into the voucher window
    /// predicate as *quoted* `$$Date:"..."` TDL arguments. `xml_escape` alone
    /// is not sufficient there -- it turns `"` into `&quot;`, but Tally's XML
    /// parser decodes `&quot;` back into a literal `"` before the formula
    /// text is evaluated, so an escaped quote could still close the quoted
    /// argument and inject arbitrary TDL. The fix is a closed input alphabet:
    /// `render_native_voucher_export_request` only accepts already-validated
    /// `TallyDate`s (exactly 8 ASCII digits), so this module asserts that
    /// `TallyDate::parse` -- the only way to construct one, and therefore the
    /// only way to reach the predicate -- rejects every value shaped like an
    /// injection attempt, whitespace, wrong-length digit runs, and non-ASCII
    /// digits. Each of these failed before the fix (the old signature
    /// accepted `&str` and rendered whatever was given through `xml_escape`).
    #[test]
    fn voucher_window_bounds_reject_a_quote_breakout_injection_attempt() {
        // A value shaped to close the quoted $$Date:"..." argument and
        // splice in an alternative predicate clause.
        let injection = r#"20260930" OR $Date>=$$Date:"19000101"#;
        let result = TallyDate::parse(injection);
        assert!(
            result.is_err(),
            "an injection-shaped date must be REJECTED, not rendered: {result:?}"
        );
    }

    #[test]
    fn voucher_window_bounds_reject_whitespace() {
        assert!(TallyDate::parse("2026 0401").is_err());
        assert!(TallyDate::parse("20260401 ").is_err());
        assert!(TallyDate::parse(" 20260401").is_err());
    }

    #[test]
    fn voucher_window_bounds_reject_wrong_length_digit_runs() {
        assert!(TallyDate::parse("2026040").is_err()); // 7 digits
        assert!(TallyDate::parse("202604011").is_err()); // 9 digits
    }

    #[test]
    fn voucher_window_bounds_reject_non_ascii_digits() {
        // U+FF10..U+FF19 are fullwidth digit code points -- not ASCII, and
        // must not be accepted as a stand-in for '0'..'9'.
        assert!(TallyDate::parse("２０２６０４０１").is_err());
    }

    /// A well-formed 8-digit date still renders exactly today's predicate
    /// shape -- the fix rejects malformed input, it does not change accepted
    /// behavior.
    #[test]
    fn voucher_window_bounds_accept_a_valid_date_and_render_the_existing_predicate() {
        let from = TallyDate::parse("20260401").unwrap();
        let to = TallyDate::parse("20260930").unwrap();
        let voucher_xml = render_native_voucher_export_request("Bridge Billwise Lab", &from, &to);
        assert!(voucher_xml.contains(
            r#"<SYSTEM TYPE="Formulae" NAME="BridgeVoucherWindowFilter">$Date &gt;= $$Date:"20260401" AND $Date &lt;= $$Date:"20260930"</SYSTEM>"#
        ));
    }

    /// TALLY_PROTOCOL_REFERENCE section 5.1/5.2/5.3: `SVFROMDATE`/`SVTODATE`
    /// alone do not bound collection membership; a `<SYSTEM TYPE="Formulae">`
    /// predicate referenced from `<FILTERS>` is the proven fix; and section
    /// 5.3 established that the predicate must use literal `$$Date:"..."`
    /// bounds rather than `##SVFromDate`/`##SVToDate`, because a refused
    /// period boundary silently widens the period and a predicate depending
    /// on `##SVToDate` then resolves to a response indistinguishable from a
    /// genuinely empty window. This test pins the request-side filter onto
    /// the native voucher export.
    #[test]
    fn native_voucher_export_request_is_bounded_by_a_date_filter() {
        let from = TallyDate::parse("20260401").unwrap();
        let to = TallyDate::parse("20260930").unwrap();
        let voucher_xml = render_native_voucher_export_request("Bridge Billwise Lab", &from, &to);

        // The SYSTEM Formulae predicate is present and compares $Date against
        // literal date bounds built from the function's own from/to
        // arguments, not against ##SVFromDate/##SVToDate.
        assert!(voucher_xml.contains(
            r#"<SYSTEM TYPE="Formulae" NAME="BridgeVoucherWindowFilter">$Date &gt;= $$Date:"20260401" AND $Date &lt;= $$Date:"20260930"</SYSTEM>"#
        ));
        assert!(voucher_xml.contains(r#"$$Date:"20260401""#));
        assert!(voucher_xml.contains(r#"$$Date:"20260930""#));
        assert!(!voucher_xml.contains("##SVToDate"));
        assert!(!voucher_xml.contains("##SVFromDate"));

        // The COLLECTION references the filter via <FILTERS>.
        assert!(voucher_xml.contains("<FILTERS>BridgeVoucherWindowFilter</FILTERS>"));

        // The filter name itself carries no whitespace anywhere it appears.
        assert!(!"BridgeVoucherWindowFilter".contains(' '));
        for occurrence in voucher_xml.match_indices("BridgeVoucherWindowFilter") {
            let (start, _) = occurrence;
            assert!(
                !voucher_xml[start..start + "BridgeVoucherWindowFilter".len()]
                    .chars()
                    .any(char::is_whitespace)
            );
        }

        // The FETCH list is exactly the pre-existing, load-bearing dotted
        // ALLLEDGERENTRIES field list -- unchanged by adding the filter.
        assert!(voucher_xml.contains(
            "<FETCH>DATE, GUID, MASTERID, ALTERID, VOUCHERTYPENAME, VOUCHERNUMBER, ISCANCELLED, ISOPTIONAL, ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH>"
        ));

        // No <REPORT> element is introduced.
        assert!(!voucher_xml.contains("<REPORT>"));
        assert!(!voucher_xml.contains("<REPORT "));
    }
}
