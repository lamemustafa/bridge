//! XML request builders for Tally's native Bills Receivable/Payable reports
//! plus `List of Ledgers` and `List of Groups` collection snapshots.
//!
//! These render exact request strings; nothing in this module dispatches
//! them. The Bills Receivable/Payable shape is the WORKING shape verified
//! live against TallyPrime (TALLY_PROTOCOL_REFERENCE ground truth captured
//! 2026-08-07): `SVTODATE` controls the report's as-of date and must always
//! be present.

use bridge_tally_primitives::TallyDate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBillsReportKind {
    Receivable,
    Payable,
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
/// fetch exactly the fields the on-account residual computation needs:
/// `NAME`, `PARENT`, `CLOSINGBALANCE`, `OPENINGBALANCE`, `ISBILLWISEON`.
///
/// **`SVFROMDATE`/`SVTODATE` are load-bearing here and must match the bills
/// request exactly.** `CLOSINGBALANCE` *is* as-of scoped -- measured
/// 2026-08-07, the same collection returned a Sundry total of Rs -44,09,597 at
/// `SVTODATE=20260731` and Rs -21,19,377 at `20250401`. The bills reports are
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
    from: &TallyDate,
    to: &TallyDate,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>NAME, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = from.as_str(),
        to = to.as_str(),
    )
}

/// Renders a native `List of Ledgers` collection for ordinary ledger export.
///
/// This mirrors the retired report's fetch list exactly, but reads the master
/// values directly rather than rendering them through a report `FIELD`. There
/// are intentionally no period variables: the legacy request carried only
/// `SVEXPORTFORMAT` and `SVCURRENTCOMPANY`; adding a date would change the
/// previously established semantics of `OPENINGBALANCE`.
pub fn render_native_ledger_export_request(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Ledgers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Ledgers" ISMODIFY="Yes"><FETCH>NAME, GUID, REMOTEID, MASTERID, ALTERID, PARENT, PARTYGSTIN, OPENINGBALANCE</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
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

/// Renders a native Voucher collection with the dotted entry fields Tally
/// requires to include accounting rows. `SVFROMDATE` and `SVTODATE` scope the
/// export and must remain paired with the requested canonical window.
pub fn render_native_voucher_export_request(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>BridgeVoucherExport</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY><SVFROMDATE TYPE="Date">{from}</SVFROMDATE><SVTODATE TYPE="Date">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="BridgeVoucherExport" ISMODIFY="No"><TYPE>Voucher</TYPE><FETCH>DATE, GUID, MASTERID, ALTERID, VOUCHERTYPENAME, VOUCHERNUMBER, ISCANCELLED, ISOPTIONAL, ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        company = xml_escape(company),
        from = xml_escape(from),
        to = xml_escape(to),
    )
}

/// Renders a request for the `List of Groups` collection, overridden to fetch
/// the complete group ancestry and durable master identity the core reader
/// needs. `GUID`, `MASTERID`, and `ALTERID` were captured from both supported
/// Tally Education books; mutable `NAME` is never an identity fallback.
///
/// Unlike the legacy export profile, this stays in Tally's native Collection
/// family: it defines no report/form/part/line/field stack and invokes no TDL
/// function. Paired byte-identical reads plus the enclosing book-extent
/// bracket establish completeness for this snapshot.
pub fn render_native_group_snapshot_request(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>List of Groups</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="List of Groups" ISMODIFY="Yes"><FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
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
/// `MAILINGNAME` `"Indian Rupees"`.
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

        let ledger_xml = render_native_ledger_snapshot_request("A & B <Co>", &from, &to);
        assert!(ledger_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(ledger_xml
            .contains("<FETCH>NAME, PARENT, CLOSINGBALANCE, OPENINGBALANCE, ISBILLWISEON</FETCH>"));
        assert!(ledger_xml.contains(r#"<SVFROMDATE TYPE="Date">20240401</SVFROMDATE>"#));
        assert!(ledger_xml.contains(r#"<SVTODATE TYPE="Date">20260731</SVTODATE>"#));

        let group_xml = render_native_group_snapshot_request("A & B <Co>");
        assert!(group_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(group_xml.contains(r#"<ID>List of Groups</ID>"#));
        assert!(group_xml.contains(r#"<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH>"#));
        assert!(!group_xml.contains("<REPORT>"));
        assert!(!group_xml.contains("<FORM>"));
        assert!(!group_xml.contains("<PART>"));
        assert!(!group_xml.contains("<LINE>"));
        assert!(!group_xml.contains("<FIELD>"));
        assert!(!group_xml.contains("$$NumItems"));

        let export_xml = render_native_ledger_export_request("A & B <Co>");
        assert!(export_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(export_xml.contains(r#"<FETCH>NAME, GUID, REMOTEID, MASTERID, ALTERID, PARENT, PARTYGSTIN, OPENINGBALANCE</FETCH>"#));
        assert!(!export_xml.contains("<REPORT>"));
        assert!(!export_xml.contains("<FORM>"));
        assert!(!export_xml.contains("<PART>"));
        assert!(!export_xml.contains("<LINE>"));
        assert!(!export_xml.contains("<FIELD>"));
        assert!(!export_xml.contains("$$NumItems"));
        assert!(!export_xml.contains("SVFROMDATE"));
        assert!(!export_xml.contains("SVTODATE"));

        let voucher_type_xml = render_native_voucher_type_export_request("A & B <Co>");
        assert!(voucher_type_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(voucher_type_xml.contains("<ID>List of VoucherTypes</ID>"));
        assert!(voucher_type_xml.contains("<FETCH>NAME, PARENT, GUID, MASTERID, ALTERID</FETCH>"));
        assert!(!voucher_type_xml.contains("<REPORT>"));
        assert!(!voucher_type_xml.contains("$$NumItems"));

        let voucher_xml =
            render_native_voucher_export_request("A & B <Co>", "2026<0401", "2026&0930");
        assert!(voucher_xml.contains("A &amp; B &lt;Co&gt;"));
        assert!(voucher_xml.contains("2026&lt;0401"));
        assert!(voucher_xml.contains("2026&amp;0930"));
        assert!(voucher_xml.contains("<TYPE>Collection</TYPE>"));
        assert!(voucher_xml.contains("ALLLEDGERENTRIES.LEDGERNAME, ALLLEDGERENTRIES.AMOUNT, ALLLEDGERENTRIES.ISDEEMEDPOSITIVE"));
        assert!(!voucher_xml.contains("<REPORT>"));
        assert!(!voucher_xml.contains("<FORM>"));
        assert!(!voucher_xml.contains("<FIELD>"));
        assert!(!voucher_xml.contains("$$NumItems"));
    }
}
