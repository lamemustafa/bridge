//! XML request builders for Tally's native Bills Receivable/Payable reports
//! and the `List of Ledgers` collection snapshot.
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
    }
}
