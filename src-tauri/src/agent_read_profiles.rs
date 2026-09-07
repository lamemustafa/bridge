//! Read profiles for the local MCP adapter.
use super::*;

pub(super) fn render_agent_vouchers(
    company: &str,
    from: &str,
    to: &str,
    alter_id: Option<u64>,
) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    let alter_filter = alter_id
        .map(|value| format!(" AND $AlterID > {value}"))
        .unwrap_or_default();
    Ok(format!("<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{alter_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.LEDGERNAME,ALLLEDGERENTRIES.AMOUNT,ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH><FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>", xml_escape(company.as_str())))
}

pub(super) fn render_agent_changed_vouchers(
    company: &str,
    checkpoint: u64,
    snapshot: u64,
) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedVoucher\">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.LEDGERNAME,ALLLEDGERENTRIES.AMOUNT,ALLLEDGERENTRIES.ISDEEMEDPOSITIVE</FETCH><FILTERS>BridgeAgentChangedVoucher</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
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
