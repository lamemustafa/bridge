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
    render_agent_movement_vouchers_in_span(company, from, to, None)
}

/// [`render_agent_movement_vouchers`], optionally narrowed to an AlterID span.
/// `None` renders the unnarrowed request byte for byte; `Some` is one part of
/// a day too heavy for one read (protocol reference §11c).
pub(super) fn render_agent_movement_vouchers_in_span(
    company: &str,
    from: &str,
    to: &str,
    span: Option<AlterIdSpan>,
) -> Result<String, String> {
    render_windowed_vouchers(
        company,
        from,
        to,
        &span.map(AlterIdSpan::filter).unwrap_or_default(),
        AGENT_MOVEMENT_FETCH,
        "",
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
    render_windowed_vouchers(company, from, to, &alter_filter, AGENT_VOUCHER_FETCH, "")
}

/// [`render_agent_vouchers`], optionally narrowed to an AlterID span. `None`
/// renders the unnarrowed request byte for byte; `Some` is one part of a day
/// too heavy for one read (protocol reference §11c).
pub(super) fn render_agent_vouchers_in_span(
    company: &str,
    from: &str,
    to: &str,
    span: Option<AlterIdSpan>,
) -> Result<String, String> {
    render_windowed_vouchers(
        company,
        from,
        to,
        &span.map(AlterIdSpan::filter).unwrap_or_default(),
        AGENT_VOUCHER_FETCH,
        "",
    )
}

/// [`render_agent_vouchers_in_span`] with each row's voucher type resolved by
/// Tally in the same response: its GUID, its own reserved name and one
/// `$$Is<Class>` answer per measured class (bridge#625). Sent only by a
/// `vouchers` call that filters by type, so every other read is unchanged.
pub(super) fn render_agent_class_vouchers_in_span(
    company: &str,
    from: &str,
    to: &str,
    span: Option<AlterIdSpan>,
) -> Result<String, String> {
    render_windowed_vouchers(
        company,
        from,
        to,
        &span.map(AlterIdSpan::filter).unwrap_or_default(),
        AGENT_VOUCHER_FETCH,
        &voucher_type_class_computes(),
    )
}

/// The pre-flight census of protocol reference §11c: one light row per voucher
/// in the window, optionally only those whose AlterID falls in `span`.
///
/// The `FETCH` is the one §12.7 qualified for the empty-partition witness, and
/// the AlterID range is the segment filter the outstandings scanner sends. The
/// date window keeps a census cheap; the span bounds the response by
/// construction — with distinct AlterIDs it cannot return more than
/// `span.through - span.after` rows — and is used whenever the book's mark is
/// larger than one census (protocol reference §11c.3).
pub(super) fn render_agent_voucher_census(
    company: &str,
    from: &str,
    to: &str,
    span: Option<AlterIdSpan>,
) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    let span_filter = span.map(AlterIdSpan::filter).unwrap_or_default();
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
    computes: &str,
) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    Ok(format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{alter_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>{fetch}</FETCH>{computes}<FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
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
    render_windowed_vouchers(company, from, to, "", AGENT_LAB_INVENTORY_VOUCHER_FETCH, "")
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

/// A read request rendered by one of the profiles below.
///
/// The field is private to this module, so a `ReadRequest` can only come from
/// a function here, and each of those renders a fixed Bridge profile.
/// `Server::post_read` accepts nothing else. `AgentReadRequest::parse` still
/// checks the envelope before dispatch, but it is no longer the only barrier
/// between caller-built XML and the gateway.
#[derive(Clone, Debug)]
pub(super) struct ReadRequest(String);

impl ReadRequest {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn into_xml(self) -> String {
        self.0
    }

    /// Test-only: lets a test hand `post_read` a request no profile renders,
    /// to prove the envelope gate behind the seal still refuses it.
    #[cfg(test)]
    pub(super) fn unrendered_for_test(xml: String) -> Self {
        Self(xml)
    }
}

pub(super) fn company_high_water_read(company: &str) -> ReadRequest {
    ReadRequest(render_agent_company_high_water(company))
}

pub(super) fn master_domain_high_water_read(company: &str, kind: MasterKind) -> ReadRequest {
    ReadRequest(render_agent_master_domain_high_water(company, kind))
}

pub(super) fn changed_vouchers_read(company: &str, checkpoint: u64, snapshot: u64) -> ReadRequest {
    ReadRequest(render_agent_changed_vouchers(company, checkpoint, snapshot))
}

pub(super) fn changed_masters_read(
    company: &str,
    checkpoint: u64,
    snapshot: u64,
    kind: MasterKind,
) -> ReadRequest {
    ReadRequest(render_agent_changed_masters(
        company, checkpoint, snapshot, kind,
    ))
}

pub(super) fn standard_ledger_catalog_read(company: &str) -> anyhow::Result<ReadRequest> {
    crate::tally::standard_ledger_catalog::render_standard_ledger_catalog_request(company)
        .map(ReadRequest)
}

pub(super) fn native_group_snapshot_read(company: &str) -> ReadRequest {
    ReadRequest(
        bridge_tally_protocol::native_outstandings::render_native_group_snapshot_request(company),
    )
}

/// The company's Currency masters: the request the outstandings paths send,
/// verbatim. Its response carries no company GUID, so it is bound only by the
/// identity brackets it is read inside.
pub(super) fn company_currency_read(company: &str) -> ReadRequest {
    ReadRequest(
        bridge_tally_protocol::native_outstandings::render_company_currency_request(company),
    )
}

pub(super) fn voucher_window_part_read(
    shape: super::voucher_window::VoucherReadShape,
    company: &str,
    from: &str,
    to: &str,
    span: Option<AlterIdSpan>,
) -> Result<ReadRequest, String> {
    shape.render(company, from, to, span).map(ReadRequest)
}

pub(super) fn voucher_census_read(
    company: &str,
    from: &str,
    to: &str,
    span: Option<AlterIdSpan>,
) -> Result<ReadRequest, String> {
    render_agent_voucher_census(company, from, to, span).map(ReadRequest)
}

#[cfg(feature = "lab-writes")]
pub(super) fn lab_inventory_vouchers_read(
    company: &str,
    from: &str,
    to: &str,
) -> Result<ReadRequest, String> {
    render_agent_lab_inventory_vouchers(company, from, to).map(ReadRequest)
}

#[cfg(feature = "lab-writes")]
pub(super) fn lab_master_collection_read(
    company: &str,
    kind: super::lab::LabMasterKind,
    period: &bridge_tally_protocol::native_outstandings::NativeLedgerExportPeriod,
) -> Result<ReadRequest, String> {
    super::lab::render_lab_master_collection(company, kind, period).map(ReadRequest)
}

#[cfg(feature = "lab-writes")]
pub(super) fn lab_write_master_collection_read(
    company: &str,
    kind: super::lab::import::MasterKind,
    period: &bridge_tally_protocol::native_outstandings::NativeLedgerExportPeriod,
) -> Result<ReadRequest, String> {
    super::lab::import::render_master_collection_request(company, kind, period).map(ReadRequest)
}

#[cfg(feature = "lab-writes")]
pub(super) fn lab_voucher_window_read(
    company: &str,
    from: &str,
    to: &str,
) -> Result<ReadRequest, String> {
    super::lab::import::render_voucher_window_request(company, from, to).map(ReadRequest)
}

#[cfg(test)]
mod sealed_read_tests {
    use super::*;
    use crate::tally::agent_read_request::AgentReadRequest;

    /// Every sealed profile renders a read envelope the dispatch gate admits.
    /// A profile added here that rendered anything else would fail this test
    /// before it could reach `post_read`.
    #[test]
    fn every_sealed_profile_renders_an_admitted_read_envelope() {
        let company = "Bridge Sealed Read Co";
        let span = Some(AlterIdSpan {
            after: 10,
            through: 20,
        });
        let mut reads = vec![
            company_high_water_read(company),
            master_domain_high_water_read(company, MasterKind::Ledger),
            master_domain_high_water_read(company, MasterKind::Group),
            changed_vouchers_read(company, 1, 2),
            changed_masters_read(company, 1, 2, MasterKind::Ledger),
            standard_ledger_catalog_read(company).unwrap(),
            native_group_snapshot_read(company),
            voucher_census_read(company, "20260401", "20260430", span).unwrap(),
        ];
        for shape in [
            super::super::voucher_window::VoucherReadShape::ImportVerification,
            super::super::voucher_window::VoucherReadShape::Movement,
            super::super::voucher_window::VoucherReadShape::EntryWildcard,
            super::super::voucher_window::VoucherReadShape::ClassEntryWildcard,
        ] {
            for part_span in [None, span] {
                reads.push(
                    voucher_window_part_read(shape, company, "20260401", "20260430", part_span)
                        .unwrap(),
                );
            }
        }
        for read in reads {
            assert!(
                AgentReadRequest::parse(read.as_str().to_string()).is_ok(),
                "{}",
                read.as_str()
            );
        }
    }
}
