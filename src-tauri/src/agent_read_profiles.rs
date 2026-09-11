//! Read profiles for the local MCP adapter.
use super::*;

/// The FETCH list for reads that RETURN bill allocations.
///
/// `ALLLEDGERENTRIES.*`, the shape `IMPLEMENTATION_GUIDE.md` §2.4a proves correct on
/// the instance where curated allocation paths misreport genuine `New Ref`/`Agst Ref`
/// rows as `On Account`. Measured on TallyPrime 7.1 Silver (protocol reference §8.2a),
/// `ALLLEDGERENTRIES.BILLALLOCATIONS.*` recovers the same allocations for 1.12x the
/// curated payload against 7.3x — but that is one instance, and §8.2a says plainly it
/// is **untested on the affected one**.
///
/// The asymmetry decides it. If the narrower shape is wrong there, a reader silently
/// receives incorrect bill types on compliance data. If the wider shape costs too much,
/// that is loud, measurable and fixable. An unverified narrowing is not worth a payload
/// saving when the failure mode is silently-wrong evidence, so the proven shape is used
/// until someone runs the A/B on that corpus.
///
/// The parser handles all 27 nested `*.LIST` types this returns per entry, including
/// `TAXBILLALLOCATIONS.LIST` — a different list from `BILLALLOCATIONS.LIST` — proven by
/// `entry_wildcard_response_parses_with_its_twenty_seven_nested_lists` against a live
/// capture rather than a constructed one.
const AGENT_VOUCHER_FETCH: &str = "DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,\
GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.*";

/// The FETCH list for `ledger_movement`, which DISCARDS bill allocations.
///
/// `MovementEntry` carries no allocation field, so the movement read threw the
/// allocation payload away after paying for it — and, worse, inherited allocation
/// parse failures: one malformed allocation aborted a movement read that never wanted
/// allocations. With the entry wildcard above that cost becomes 7.3x for data the
/// caller cannot see, and dense windows that previously fit could exceed limits while
/// returning no movement at all.
///
/// A read should fetch what it returns. This list is the three entry fields movement
/// actually uses.
const AGENT_MOVEMENT_FETCH: &str = "DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,\
GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.LEDGERNAME,ALLLEDGERENTRIES.AMOUNT,\
ALLLEDGERENTRIES.ISDEEMEDPOSITIVE";

/// Windowed voucher read for `ledger_movement`, which does not return allocations.
pub(super) fn render_agent_movement_vouchers(
    company: &str,
    from: &str,
    to: &str,
) -> Result<String, String> {
    render_windowed_vouchers(company, from, to, None, AGENT_MOVEMENT_FETCH)
}

pub(super) fn render_agent_vouchers(
    company: &str,
    from: &str,
    to: &str,
    alter_id: Option<u64>,
) -> Result<String, String> {
    render_windowed_vouchers(company, from, to, alter_id, AGENT_VOUCHER_FETCH)
}

fn render_windowed_vouchers(
    company: &str,
    from: &str,
    to: &str,
    alter_id: Option<u64>,
    fetch: &str,
) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    let alter_filter = alter_id
        .map(|value| format!(" AND $AlterID > {value}"))
        .unwrap_or_default();
    Ok(format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{alter_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>{fetch}</FETCH><FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company.as_str())
    ))
}

pub(super) fn render_agent_changed_vouchers(
    company: &str,
    checkpoint: u64,
    snapshot: u64,
) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedVoucher\">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>{AGENT_VOUCHER_FETCH}</FETCH><FILTERS>BridgeAgentChangedVoucher</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

#[derive(Clone, Copy)]
pub(super) enum MasterKind {
    Ledger,
    Group,
}

impl MasterKind {
    pub(super) const ALL: [Self; 2] = [Self::Ledger, Self::Group];

    fn tally_type(self) -> &'static str {
        match self {
            Self::Ledger => "Ledger",
            Self::Group => "Group",
        }
    }
}

pub(super) fn render_agent_changed_masters(
    company: &str,
    checkpoint: u64,
    snapshot: u64,
    kind: MasterKind,
) -> String {
    render_master_collection(company, kind, Some((checkpoint, snapshot)))
}

pub(super) fn render_agent_master_domain_high_water(company: &str, kind: MasterKind) -> String {
    render_master_collection(company, kind, None)
}

fn render_master_collection(company: &str, kind: MasterKind, window: Option<(u64, u64)>) -> String {
    let object_type = kind.tally_type();
    let name = format!("Bridge Agent {object_type} Changes");
    let (formula, filter, fields) = match window {
        Some((checkpoint, snapshot)) => (
            format!(
                r#"<SYSTEM TYPE="Formulae" NAME="BridgeAgentChangedMaster">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM>"#
            ),
            "<FILTERS>BridgeAgentChangedMaster</FILTERS><SORT>Default: $AlterID</SORT>",
            "NAME,PARENT,ALTERID,GUID,MASTERID",
        ),
        None => (String::new(), "", "ALTERID"),
    };
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>{name}</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE>{formula}<COLLECTION NAME="{name}" ISMODIFY="No"><TYPE>{object_type}</TYPE><FETCH>{fields}</FETCH>{filter}</COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        xml_escape(company)
    )
}

pub(super) fn render_agent_company_high_water(company: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Company High Water</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME=\"Bridge Agent Company High Water\" ISMODIFY=\"No\"><TYPE>Company</TYPE><FETCH>GUID,ALTVCHID,ALTMSTID</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}
