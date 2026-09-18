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
///
/// `ISPOSTDATED` is requested here for the same reason `ISCANCELLED`/
/// `ISOPTIONAL` are: Tally will not include a field in COLLECTION XML that
/// this FETCH list does not name, regardless of what the parser is willing
/// to accept. Allow-listing the tag in `agent_voucher_scalars.rs` without
/// asking for it here would make `post_dated` permanently absent — a
/// wired-looking field that never fires.
///
/// `REFERENCE`, `ISINVOICE` and `PARTYGSTIN` are requested for the same
/// reason, added after `ALLLEDGERENTRIES.*` to match the order proven on the
/// wire in protocol reference §8.2c (TallyPrime 7.1 Silver, licensed,
/// `BRIDGE SHAPE LAB`, 2026-09-18): all three round-trip cleanly alongside
/// the existing fields, `ISINVOICE` is the one logical here Tally emits
/// without a `TYPE="Logical"` attribute (irrelevant to parsing, which
/// matches on tag name only), and `PARTYGSTIN` was structurally present but
/// empty on every voucher in that capture — its population is unverified,
/// not its presence.
const AGENT_VOUCHER_FETCH: &str = "DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,\
GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ISPOSTDATED,ALLLEDGERENTRIES.*,\
REFERENCE,ISINVOICE,PARTYGSTIN";

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
    render_agent_movement_vouchers_sample(company, from, to, None)
}

/// [`render_agent_movement_vouchers`], optionally narrowed to an AlterID span.
/// `None` renders the unnarrowed request byte for byte; `Some` is only the
/// bounded calibration sample of protocol reference §11c.
pub(super) fn render_agent_movement_vouchers_sample(
    company: &str,
    from: &str,
    to: &str,
    sample: Option<AlterIdSpan>,
) -> Result<String, String> {
    render_windowed_vouchers(
        company,
        from,
        to,
        &sample.map(AlterIdSpan::filter).unwrap_or_default(),
        AGENT_MOVEMENT_FETCH,
    )
}

pub(super) fn render_agent_vouchers(
    company: &str,
    from: &str,
    to: &str,
    alter_id: Option<u64>,
) -> Result<String, String> {
    let alter_filter = alter_id
        .map(|value| format!(" AND $AlterID > {value}"))
        .unwrap_or_default();
    render_windowed_vouchers(company, from, to, &alter_filter, AGENT_VOUCHER_FETCH)
}

/// [`render_agent_vouchers`], optionally narrowed to an AlterID span. `None`
/// renders the unnarrowed request byte for byte; `Some` is only the bounded
/// calibration sample of protocol reference §11c.
pub(super) fn render_agent_vouchers_sample(
    company: &str,
    from: &str,
    to: &str,
    sample: Option<AlterIdSpan>,
) -> Result<String, String> {
    render_windowed_vouchers(
        company,
        from,
        to,
        &sample.map(AlterIdSpan::filter).unwrap_or_default(),
        AGENT_VOUCHER_FETCH,
    )
}

/// The pre-flight census of protocol reference §11c: one light row per voucher
/// in the window whose AlterID falls in `span`, and nothing else.
///
/// The `FETCH` is the one §12.7 qualified for the empty-partition witness, and
/// the AlterID range is the segment filter the outstandings scanner sends. The
/// span is what bounds this request: with distinct AlterIDs it cannot return
/// more than `span.through - span.after` rows, however dense the window is.
pub(super) fn render_agent_voucher_census(
    company: &str,
    from: &str,
    to: &str,
    span: AlterIdSpan,
) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    let span_filter = span.filter();
    Ok(format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Voucher Census</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentCensus\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{span_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Voucher Census\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>GUID,ALTERID,DATE</FETCH><FILTERS>BridgeAgentCensus</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company.as_str())
    ))
}

fn render_windowed_vouchers(
    company: &str,
    from: &str,
    to: &str,
    alter_filter: &str,
    fetch: &str,
) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    Ok(format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{alter_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>{fetch}</FETCH><FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company.as_str())
    ))
}

/// The FETCH list for LAB-ONLY inventory-entry reads (`lab_read_inventory`,
/// feature `lab-writes`). Unlike [`AGENT_VOUCHER_FETCH`], this asks for
/// `ALLINVENTORYENTRIES.*` instead of `ALLLEDGERENTRIES.*` -- no shipped tool
/// reads inventory today (see the plan-research note §4.2), so this shape is
/// exploratory pending a live capture, not a qualified/compatibility-evidenced
/// read.
#[cfg(feature = "lab-writes")]
const AGENT_LAB_INVENTORY_VOUCHER_FETCH: &str = "DATE,VOUCHERNUMBER,VOUCHERTYPENAME,\
PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ALLINVENTORYENTRIES.*";

/// Windowed voucher read for the LAB-ONLY `lab_read_inventory` tool. Reuses
/// the same [`render_windowed_vouchers`] windowing machinery (and therefore
/// the same `window_honoured` corroboration path) as `vouchers`/
/// `ledger_movement` -- only the FETCH list differs.
#[cfg(feature = "lab-writes")]
pub(super) fn render_agent_lab_inventory_vouchers(
    company: &str,
    from: &str,
    to: &str,
) -> Result<String, String> {
    render_windowed_vouchers(company, from, to, "", AGENT_LAB_INVENTORY_VOUCHER_FETCH)
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
