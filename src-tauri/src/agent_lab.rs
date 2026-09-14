//! LAB-ONLY additive surface (audit-sprint 2026-09-14, Phase 3.1/3.2).
//!
//! This entire file is compiled only behind the `lab-writes` Cargo feature
//! (not default; never enabled in a release/CI workflow -- see
//! `tests/lab_writes_ci_gate.rs`), and every tool it registers additionally
//! refuses at runtime unless `BRIDGE_LAB_WRITES=1` is set. It never modifies
//! the production write guards (`agent_import.rs`, `agent_import_post.rs`,
//! `approved_import.rs`); this module adds a parallel, narrowly-scoped
//! surface rather than changing those. Scope for this pass (3.1/3.2): the
//! shared lab guard machinery (a later lab writer will reuse
//! `admit_lab_target`) and one read-only tool, `lab_read_inventory`. No write
//! path exists anywhere in this file.
//!
//! No signed compatibility evidence exists for any inventory field read here
//! on any Tally release/mode (the plan-research note's §4.2/§4.5 findings):
//! every value this tool returns is exploratory, not a qualified claim.

use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

// Phase 3.4/3.5: the lab writer tools (`lab_import_masters`,
// `lab_import_vouchers`). Kept in its own file for size; reuses this
// module's guard/evidence machinery (`admit_lab_target`, `lab_post_read`,
// `persist_lab_exchange`, `lab_evidence_dir`, `parse_lab_master_rows`) via
// the same private-item-visible-to-descendant-module path this file itself
// uses for `agent.rs`'s items.
#[path = "agent_lab_import.rs"]
mod import;
pub(super) use import::{lab_import_masters, lab_import_vouchers};

// ---------------------------------------------------------------------------
// Env gates
// ---------------------------------------------------------------------------

/// `BRIDGE_LAB_WRITES=1` gate. Read fresh on every call -- this is a lab
/// safety gate, not a cached setting, so flipping the env var takes effect on
/// the very next call rather than only after a restart.
pub(super) fn env_lab_writes_enabled() -> bool {
    matches!(
        env::var("BRIDGE_LAB_WRITES").as_deref(),
        Ok("1") | Ok("true")
    )
}

pub(super) fn require_lab_writes_env() -> Result<(), String> {
    env_lab_writes_enabled()
        .then_some(())
        .ok_or_else(|| "lab_writes_disabled".to_string())
}

/// The lab guard configuration: the one company lab tools may touch, and the
/// companies they must never see loaded. Every field is required; any
/// absence or malformed value fails closed before any Tally request is made.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct LabGuardConfig {
    target_guid: String,
    deny_guids: Vec<String>,
}

impl LabGuardConfig {
    fn from_env() -> Result<Self, String> {
        let target = env::var("BRIDGE_LAB_TARGET_GUID")
            .map_err(|_| "lab_target_guid_required".to_string())?;
        let deny_raw =
            env::var("BRIDGE_LAB_DENY_GUIDS").map_err(|_| "lab_deny_guids_required".to_string())?;
        Self::from_values(&target, &deny_raw)
    }

    /// Pure parser over already-read env values -- kept separate from
    /// [`Self::from_env`] so tests exercise parsing without mutating the
    /// process-wide `std::env`, which is unsound to do from parallel test
    /// threads.
    fn from_values(target: &str, deny_raw: &str) -> Result<Self, String> {
        let target_guid = parse_native_company_guid(target.trim())
            .map_err(|_| "lab_target_guid_invalid".to_string())?
            .hyphenated()
            .to_string();
        let mut deny_guids = Vec::new();
        for candidate in deny_raw.split(',') {
            let candidate = candidate.trim();
            if candidate.is_empty() {
                continue;
            }
            let guid = parse_native_company_guid(candidate)
                .map_err(|_| "lab_deny_guids_invalid".to_string())?
                .hyphenated()
                .to_string();
            deny_guids.push(guid);
        }
        if deny_guids.is_empty() {
            return Err("lab_deny_guids_required".to_string());
        }
        if deny_guids
            .iter()
            .any(|guid| guid.eq_ignore_ascii_case(&target_guid))
        {
            return Err("lab_deny_guids_invalid".to_string());
        }
        Ok(Self {
            target_guid,
            deny_guids,
        })
    }
}

// ---------------------------------------------------------------------------
// Guard admission
// ---------------------------------------------------------------------------

/// The always-on lab preconditions: `BRIDGE_LAB_WRITES=1`, the Tally endpoint
/// pinned to port 9001, and `BRIDGE_LAB_TARGET_GUID`/`BRIDGE_LAB_DENY_GUIDS`
/// both present and well-formed. Every lab tool call requires these,
/// `lab_read_inventory` included -- but a *read* does not require the target
/// to be the company being read, or to be the company currently loaded (that
/// stronger requirement, [`admit_lab_target`], is scoped to a write batch:
/// "before every write batch, observed loaded companies must include target
/// and exclude every deny GUID").
fn require_lab_read_guards(server: &Server) -> Result<LabGuardConfig, ToolFailure> {
    require_lab_writes_env().map_err(ToolFailure::from)?;
    if server.settings.endpoint.port != 9001 {
        return Err("lab_port_not_9001".to_string().into());
    }
    LabGuardConfig::from_env().map_err(ToolFailure::from)
}

/// Re-verifies every lab guard immediately before a lab **write batch**: the
/// port, the deny/target loaded-company set, and the target's 4-field
/// identity. Two independent company-list reads are deliberate -- one for
/// the loaded-set check below, one inside `verified_company` for identity --
/// matching the paranoia the production write path already applies
/// immediately before dispatch (`require_unique_company_scope`).
///
/// The returned `VerifiedCompanyIdentity`'s exact `display_name()` is the
/// only company name a lab writer may render into `SVCURRENTCOMPANY` --
/// that makes "SVCURRENTCOMPANY = target's exact name" a structural property
/// of any request built from this identity, not a separate check that could
/// drift out of sync with it.
///
/// Reserved for the Phase 3.5 lab writer (not yet built); `lab_read_inventory`
/// (Phase 3.2, read-only) intentionally does not call this -- see
/// [`require_lab_read_guards`].
#[allow(dead_code)]
pub(super) async fn admit_lab_target(
    server: &Server,
) -> Result<(TallyCompany, VerifiedCompanyIdentity, Evidence), ToolFailure> {
    let config = require_lab_read_guards(server)?;
    let (companies, evidence) = server.companies().await?;
    let denied = companies.iter().any(|company| {
        company.guid.as_deref().is_some_and(|guid| {
            config
                .deny_guids
                .iter()
                .any(|deny| deny.eq_ignore_ascii_case(guid))
        })
    });
    if denied {
        return Err(ToolFailure::from("lab_source_company_loaded".to_string())
            .with_prior_evidence(evidence));
    }
    let loaded = companies.iter().any(|company| {
        company
            .guid
            .as_deref()
            .is_some_and(|guid| guid.eq_ignore_ascii_case(&config.target_guid))
    });
    if !loaded {
        return Err(
            ToolFailure::from("lab_target_not_loaded".to_string()).with_prior_evidence(evidence)
        );
    }
    let (company, identity, verify_evidence) =
        server
            .verified_company(&config.target_guid)
            .await
            .map_err(|failure| failure.with_prior_evidence(evidence.clone()))?;
    Ok((
        company,
        identity,
        combine_evidence(evidence, verify_evidence),
    ))
}

// ---------------------------------------------------------------------------
// Local evidence persistence -- every lab request/response, with sha256
// ---------------------------------------------------------------------------

fn lab_evidence_dir(server: &Server) -> Result<PathBuf, String> {
    let dir = server.settings.data_dir.join("lab");
    ensure_private_directory(&dir).map_err(|error| match error {
        DirectoryAdmissionError::Unavailable => "lab_dir_unavailable".to_string(),
        #[cfg(unix)]
        DirectoryAdmissionError::Permissions => "lab_dir_permissions_failed".to_string(),
    })?;
    Ok(dir)
}

/// Persists the raw request and response bytes for one lab Tally exchange
/// under `data_dir/lab/`, named and manifested by their own sha256 -- every
/// lab request/response is retained, not just its receipt (contrast
/// `egress_log`, which deliberately never persists raw bodies).
fn persist_lab_exchange(
    server: &Server,
    tool: &str,
    request_xml: &str,
    response_xml: &str,
) -> Result<(), String> {
    let dir = lab_evidence_dir(server)?;
    let request_sha256 = sha256_hex(request_xml.as_bytes());
    let response_sha256 = sha256_hex(response_xml.as_bytes());
    write_private_file(
        &dir.join(format!("{request_sha256}.request.xml")),
        request_xml.as_bytes(),
    )?;
    write_private_file(
        &dir.join(format!("{response_sha256}.response.xml")),
        response_xml.as_bytes(),
    )?;
    let record = json!({
        "tool": tool,
        "at": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "request_sha256": request_sha256,
        "response_sha256": response_sha256,
    });
    append_egress_line(&dir.join("lab-manifest.jsonl"), &record.to_string())
}

fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|_| "lab_evidence_write_failed".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "lab_evidence_write_failed".to_string())?;
    }
    Ok(())
}

/// A `post_read` that additionally persists the exchange (§ above) before
/// returning. All lab reads go through this, never the bare `post_read`.
async fn lab_post_read(
    server: &Server,
    identity: &VerifiedCompanyIdentity,
    tool: &str,
    request: String,
) -> Result<(String, Evidence), ToolFailure> {
    let (response, evidence) = server.post_read(identity, request.clone()).await?;
    persist_lab_exchange(server, tool, &request, &response)
        .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
    Ok((response, evidence))
}

// ---------------------------------------------------------------------------
// Master reads: units, godowns, stock groups, stock items
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum LabMasterKind {
    Unit,
    Godown,
    StockGroup,
    StockItem,
}

impl LabMasterKind {
    const ALL: [Self; 4] = [Self::Unit, Self::Godown, Self::StockGroup, Self::StockItem];

    fn tally_type(self) -> &'static str {
        match self {
            Self::Unit => "Unit",
            Self::Godown => "Godown",
            Self::StockGroup => "StockGroup",
            Self::StockItem => "StockItem",
        }
    }

    fn fetch_fields(self) -> &'static str {
        match self {
            Self::Unit => "NAME,ORIGINALNAME,ISSIMPLEUNIT,DECIMALPLACES,GUID,MASTERID,ALTERID",
            Self::Godown => "NAME,PARENT,GUID,MASTERID,ALTERID",
            Self::StockGroup => "NAME,PARENT,GUID,MASTERID,ALTERID",
            Self::StockItem => {
                "NAME,PARENT,BASEUNITS,OPENINGBALANCE,OPENINGRATE,OPENINGVALUE,\
                GSTAPPLICABLE,GSTTYPEOFSUPPLY,HSNCODE,GSTHSNNAME,GUID,MASTERID,ALTERID"
            }
        }
    }

    fn result_key(self) -> &'static str {
        match self {
            Self::Unit => "units",
            Self::Godown => "godowns",
            Self::StockGroup => "stock_groups",
            Self::StockItem => "stock_items",
        }
    }
}

fn render_lab_master_collection(company: &str, kind: LabMasterKind) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    let object_type = kind.tally_type();
    let name = format!("Bridge Lab {object_type}s");
    Ok(format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>{name}</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="{name}" ISMODIFY="No"><TYPE>{object_type}</TYPE><FETCH>{}</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        xml_escape(company.as_str()),
        kind.fetch_fields()
    ))
}

/// Parses a flat master collection (`<UNIT NAME="...">...</UNIT>` etc, one
/// level under `COLLECTION`) into raw field maps. Deliberately conservative
/// like the production parsers: an unexpected non-row child of `COLLECTION`
/// fails closed rather than being silently skipped.
fn parse_lab_master_rows(
    xml: &str,
    row_tag: &str,
) -> Result<Vec<BTreeMap<String, String>>, String> {
    validate_agent_envelope(xml)?;
    let row_tag = row_tag.to_ascii_uppercase();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut path: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut current_tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                let at_collection = path == ["ENVELOPE", "BODY", "DATA", "COLLECTION"];
                if at_collection {
                    if tag != row_tag {
                        return Err("agent_read_protocol_invalid".to_string());
                    }
                    let mut row = BTreeMap::new();
                    for attribute in event.attributes() {
                        let attribute =
                            attribute.map_err(|_| "agent_read_protocol_invalid".to_string())?;
                        if attribute.key.as_ref().eq_ignore_ascii_case(b"NAME") {
                            row.insert(
                                "NAME".to_string(),
                                attribute
                                    .decoded_and_normalized_value(
                                        quick_xml::XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|_| "agent_read_protocol_invalid".to_string())?
                                    .into_owned(),
                            );
                        }
                    }
                    current = Some(row);
                }
                path.push(tag.clone());
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                // `path` includes the just-opened field tag (`current_tag`);
                // a field belongs to the row if its parent chain is exactly
                // COLLECTION_PREFIX + [row_tag].
                let is_row_field = path.len() == 6
                    && path[..4] == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
                    && path[4] == row_tag;
                if is_row_field {
                    if let Some(row) = current.as_mut() {
                        append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                    }
                }
            }
            // See the identical arm in `agent_lab_import.rs`'s
            // `parse_voucher_readback_nested`: quick_xml delivers an entity
            // reference (`&amp;`, ...) as its own `GeneralRef` event, not
            // inline within `Text`. Without this arm it is silently dropped
            // by the catch-all below -- the exact 2026-09-14 rehearsal bug
            // that read "Duties & Taxes" back as "Duties  Taxes".
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                let is_row_field = path.len() == 6
                    && path[..4] == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
                    && path[4] == row_tag;
                if is_row_field {
                    if let Some(row) = current.as_mut() {
                        append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                    }
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if path.last().map(String::as_str) == Some(end.as_str())
                    && end == row_tag
                    && path.len() == 5
                {
                    if let Some(row) = current.take() {
                        rows.push(row);
                    }
                }
                if path.pop().as_deref() != Some(end.as_str()) {
                    return Err("agent_read_protocol_invalid".to_string());
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
        }
    }
    Ok(rows)
}

fn optional_field(row: &BTreeMap<String, String>, key: &str) -> Value {
    row.get(key)
        .filter(|value| !value.trim().is_empty())
        .map(|value| Value::String(value.clone()))
        .unwrap_or(Value::Null)
}

fn lab_master_json(kind: LabMasterKind, row: &BTreeMap<String, String>) -> Value {
    match kind {
        LabMasterKind::Unit => json!({
            "name": party_name(row.get("NAME").cloned().unwrap_or_default()),
            "is_simple_unit": optional_field(row, "ISSIMPLEUNIT"),
            "decimal_places": optional_field(row, "DECIMALPLACES"),
        }),
        LabMasterKind::Godown | LabMasterKind::StockGroup => json!({
            "name": party_name(row.get("NAME").cloned().unwrap_or_default()),
            "parent": optional_field(row, "PARENT"),
        }),
        LabMasterKind::StockItem => json!({
            "name": party_name(row.get("NAME").cloned().unwrap_or_default()),
            "parent": optional_field(row, "PARENT"),
            "base_unit": optional_field(row, "BASEUNITS"),
            "opening_qty": optional_field(row, "OPENINGBALANCE"),
            "opening_rate": optional_field(row, "OPENINGRATE"),
            "opening_value": optional_field(row, "OPENINGVALUE"),
            // GST/HSN fields as returned, unclassified -- no signed
            // compatibility evidence exists for these on any Tally
            // release/mode yet (unlike the ledger GST duty-head vocabulary,
            // which §4.3 confirms is live-capture backed).
            "gst_applicable": optional_field(row, "GSTAPPLICABLE"),
            "gst_type_of_supply": optional_field(row, "GSTTYPEOFSUPPLY"),
            "hsn_code": optional_field(row, "HSNCODE"),
            "gst_hsn_name": optional_field(row, "GSTHSNNAME"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Inventory entries per voucher, windowed
// ---------------------------------------------------------------------------

/// True when `path` (the currently open element stack, most-recent last)
/// equals `expected` exactly.
fn path_is(path: &[String], expected: &[&str]) -> bool {
    path.len() == expected.len() && path.iter().zip(expected).all(|(a, b)| a == b)
}

const COLLECTION_PREFIX: [&str; 4] = ["ENVELOPE", "BODY", "DATA", "COLLECTION"];
const VOUCHER_PREFIX: [&str; 5] = ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"];
const ENTRY_PREFIX: [&str; 6] = [
    "ENVELOPE",
    "BODY",
    "DATA",
    "COLLECTION",
    "VOUCHER",
    "ALLINVENTORYENTRIES.LIST",
];
const BATCH_PREFIX: [&str; 7] = [
    "ENVELOPE",
    "BODY",
    "DATA",
    "COLLECTION",
    "VOUCHER",
    "ALLINVENTORYENTRIES.LIST",
    "BATCHALLOCATIONS.LIST",
];

/// Parses `ALLINVENTORYENTRIES.LIST` (with an optional nested
/// `BATCHALLOCATIONS.LIST`) per voucher, mirroring the two-level nesting
/// already proven for `ALLLEDGERENTRIES.LIST`/`BILLALLOCATIONS.LIST`
/// (`agent_voucher_parse.rs`). Rows carry a lower-case `date` field so the
/// shared `window_honoured` check can be reused unmodified.
fn parse_lab_inventory_vouchers(xml: &str) -> Result<Vec<Value>, String> {
    validate_agent_envelope(xml)?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut path: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut voucher: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut batch: Option<BTreeMap<String, String>> = None;
    let mut entries: Vec<Value> = Vec::new();
    let mut batches: Vec<Value> = Vec::new();
    let mut current_tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                // `path` is the parent chain of the element about to open.
                if path_is(&path, &COLLECTION_PREFIX) {
                    if tag != "VOUCHER" {
                        return Err("agent_read_protocol_invalid".to_string());
                    }
                    voucher = Some(BTreeMap::new());
                    entries.clear();
                } else if path_is(&path, &VOUCHER_PREFIX) && tag == "ALLINVENTORYENTRIES.LIST" {
                    entry = Some(BTreeMap::new());
                    batches.clear();
                } else if path_is(&path, &ENTRY_PREFIX) && tag == "BATCHALLOCATIONS.LIST" {
                    batch = Some(BTreeMap::new());
                }
                path.push(tag.clone());
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                let value = decoded_agent_text(text)?;
                // `path` here includes the just-opened `current_tag`, so a
                // field at depth N+1 belongs to the container at depth N.
                if path_is(&path[..path.len().saturating_sub(1)], &BATCH_PREFIX) {
                    if let Some(row) = batch.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                } else if path_is(&path[..path.len().saturating_sub(1)], &ENTRY_PREFIX) {
                    if let Some(row) = entry.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                } else if path_is(&path[..path.len().saturating_sub(1)], &VOUCHER_PREFIX) {
                    if let Some(row) = voucher.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                }
            }
            // Same entity-reference gap as `parse_lab_master_rows` above.
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                let value = decoded_agent_reference(reference)?;
                if path_is(&path[..path.len().saturating_sub(1)], &BATCH_PREFIX) {
                    if let Some(row) = batch.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                } else if path_is(&path[..path.len().saturating_sub(1)], &ENTRY_PREFIX) {
                    if let Some(row) = entry.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                } else if path_is(&path[..path.len().saturating_sub(1)], &VOUCHER_PREFIX) {
                    if let Some(row) = voucher.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                let closing_batch = end == "BATCHALLOCATIONS.LIST" && path_is(&path, &BATCH_PREFIX);
                let closing_entry =
                    end == "ALLINVENTORYENTRIES.LIST" && path_is(&path, &ENTRY_PREFIX);
                let closing_voucher = end == "VOUCHER" && path_is(&path, &VOUCHER_PREFIX);
                if closing_batch {
                    if let Some(row) = batch.take() {
                        batches.push(json!({
                            "batch": optional_field(&row, "BATCHNAME"),
                            "godown": optional_field(&row, "GODOWNNAME"),
                            "actual_qty": optional_field(&row, "ACTUALQTY"),
                            "billed_qty": optional_field(&row, "BILLEDQTY"),
                            "amount": optional_field(&row, "AMOUNT"),
                        }));
                    }
                }
                if closing_entry {
                    if let Some(row) = entry.take() {
                        let mut json_entry = json!({
                            "stock_item": optional_field(&row, "STOCKITEMNAME"),
                            "rate": optional_field(&row, "RATE"),
                            "amount": optional_field(&row, "AMOUNT"),
                            "actual_qty": optional_field(&row, "ACTUALQTY"),
                            "billed_qty": optional_field(&row, "BILLEDQTY"),
                            "godown": optional_field(&row, "GODOWNNAME"),
                        });
                        if !batches.is_empty() {
                            json_entry["batch_allocations"] = Value::Array(batches.clone());
                        }
                        entries.push(json_entry);
                        batches.clear();
                    }
                }
                if closing_voucher {
                    if let Some(row) = voucher.take() {
                        let date = row.get("DATE").cloned().unwrap_or_default();
                        rows.push(json!({
                            "date": date,
                            "voucher_number": optional_field(&row, "VOUCHERNUMBER"),
                            "voucher_type": optional_field(&row, "VOUCHERTYPENAME"),
                            "party": row.get("PARTYLEDGERNAME").cloned().map(party_name),
                            "guid": optional_field(&row, "GUID"),
                            "is_cancelled": optional_field(&row, "ISCANCELLED"),
                            "inventory_entries": entries.clone(),
                        }));
                        entries.clear();
                    }
                }
                if path.pop().as_deref() != Some(end.as_str()) {
                    return Err("agent_read_protocol_invalid".to_string());
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
        }
    }
    Ok(rows)
}

// ---------------------------------------------------------------------------
// The tool: lab_read_inventory
// ---------------------------------------------------------------------------

pub(super) async fn lab_read_inventory(
    server: &Server,
    args: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let guid = required_string(args, "company_guid")?;
    let from = normalized_date(required_string(args, "from")?)?;
    let to = normalized_date(required_string(args, "to")?)?;
    if from > to {
        return Err("invalid_date_range".to_string().into());
    }
    // Always-on lab preconditions (feature+env; §3.1). This is a read, so it
    // reads whichever company_guid the caller asks for -- it does not require
    // that company to be BRIDGE_LAB_TARGET_GUID, nor that the target be the
    // one currently loaded; that stronger loaded-company/deny-list guard is
    // scoped to a write batch (see `admit_lab_target`).
    require_lab_read_guards(server)?;
    let (company, identity, mut evidence) = server.verified_company(guid).await?;
    let result: Result<ToolOutcome, ToolFailure> = async {
        let mut masters = serde_json::Map::new();
        for kind in LabMasterKind::ALL {
            let request = render_lab_master_collection(identity.display_name(), kind)
                .map_err(ToolFailure::from)?;
            let (xml, read_evidence) =
                lab_post_read(server, &identity, "lab_read_inventory.masters", request).await?;
            evidence = combine_evidence(evidence.clone(), read_evidence);
            let rows = parse_lab_master_rows(&xml, kind.tally_type())
                .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
            let items = rows
                .iter()
                .map(|row| lab_master_json(kind, row))
                .collect::<Vec<_>>();
            masters.insert(kind.result_key().to_string(), Value::Array(items));
        }

        let voucher_request = render_agent_lab_inventory_vouchers(identity.display_name(), &from, &to)
            .map_err(ToolFailure::from)?;
        let (voucher_xml, voucher_evidence) = lab_post_read(
            server,
            &identity,
            "lab_read_inventory.vouchers",
            voucher_request,
        )
        .await?;
        evidence = combine_evidence(evidence.clone(), voucher_evidence);
        let rows = parse_lab_inventory_vouchers(&voucher_xml)
            .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
        if !window_honoured(&rows, &from, &to) {
            return Err(ToolFailure::from("window_not_honoured".to_string())
                .with_prior_evidence(evidence.clone()));
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit = arg_positive_usize(args, "limit", server.settings.max_rows)?
            .min(server.settings.max_rows);
        let total = rows.len();
        let items = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| redact_value(row, server.settings.redaction))
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(items.len()) < total;

        let mut result = Value::Object(masters);
        result["vouchers"] = json!({
            "items": items,
            "offset": offset,
            "total": total,
            "window_honoured": true,
            "profile": "lab_read_inventory_v1",
        });
        Ok(ToolOutcome {
            payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            evidence: evidence.clone(),
            company_guid: Some(guid.to_string()),
            truncated,
        })
    }
    .await;
    result.map_err(|failure| failure.with_prior_evidence(evidence))
}

// ---------------------------------------------------------------------------
// Tests -- synthetic fixtures only (BRIDGE CORPUS GST-style shapes), no
// client data, and no live Tally connection.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET_GUID: &str = "89b0cc46-e3b8-4809-8fc7-e29eb2ae547d";
    const DENY_GUID_1: &str = "2864b4ac-e5a3-4efc-9d2b-7593928d8f8b";
    const DENY_GUID_2: &str = "a0f82923-3d25-4757-80e0-4fa786e34610";

    /// `std::env::set_var`/`remove_var` are process-wide, so any test that
    /// touches real env vars (as opposed to `LabGuardConfig::from_values`,
    /// which takes plain strings) must serialize against every other such
    /// test in this module -- otherwise two tests racing on the same
    /// process env corrupt each other's reads under parallel test threads.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_MUTEX
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn synthetic_unit_collection() -> String {
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<UNIT NAME=\"Nos\"><ISSIMPLEUNIT>Yes</ISSIMPLEUNIT><DECIMALPLACES>0</DECIMALPLACES></UNIT>\
<UNIT NAME=\"Kgs\"><ISSIMPLEUNIT>Yes</ISSIMPLEUNIT><DECIMALPLACES>3</DECIMALPLACES></UNIT>\
</COLLECTION></DATA></BODY></ENVELOPE>"
            .to_string()
    }

    fn synthetic_stock_item_collection() -> String {
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<STOCKITEM NAME=\"Sodium Bicarbonate\"><PARENT>Chemicals</PARENT><BASEUNITS>Kgs</BASEUNITS>\
<OPENINGBALANCE>100</OPENINGBALANCE><OPENINGRATE>50.00</OPENINGRATE><OPENINGVALUE>5000.00</OPENINGVALUE>\
<GSTAPPLICABLE>Applicable</GSTAPPLICABLE><HSNCODE>28362000</HSNCODE></STOCKITEM>\
</COLLECTION></DATA></BODY></ENVELOPE>"
            .to_string()
    }

    fn synthetic_inventory_voucher_collection() -> String {
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<VOUCHER><DATE>20260405</DATE><VOUCHERNUMBER>1</VOUCHERNUMBER><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>\
<PARTYLEDGERNAME>Fixture Acid &amp; Chemicals</PARTYLEDGERNAME><GUID>fixture-guid-1</GUID><ISCANCELLED>No</ISCANCELLED>\
<ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Sodium Bicarbonate</STOCKITEMNAME><RATE>55.00</RATE>\
<AMOUNT>5500.00</AMOUNT><ACTUALQTY>100</ACTUALQTY><BILLEDQTY>100</BILLEDQTY><GODOWNNAME>Main Godown</GODOWNNAME>\
</ALLINVENTORYENTRIES.LIST></VOUCHER>\
</COLLECTION></DATA></BODY></ENVELOPE>"
            .to_string()
    }

    fn synthetic_inventory_voucher_with_batch() -> String {
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<VOUCHER><DATE>20260406</DATE><VOUCHERNUMBER>2</VOUCHERNUMBER><VOUCHERTYPENAME>Sales</VOUCHERTYPENAME>\
<PARTYLEDGERNAME>Test Party</PARTYLEDGERNAME><GUID>fixture-guid-2</GUID><ISCANCELLED>No</ISCANCELLED>\
<ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>Sodium Bicarbonate</STOCKITEMNAME><RATE>55.00</RATE><AMOUNT>2750.00</AMOUNT>\
<BATCHALLOCATIONS.LIST><BATCHNAME>Batch-01</BATCHNAME><GODOWNNAME>Main Godown</GODOWNNAME>\
<ACTUALQTY>50</ACTUALQTY><BILLEDQTY>50</BILLEDQTY><AMOUNT>2750.00</AMOUNT></BATCHALLOCATIONS.LIST>\
</ALLINVENTORYENTRIES.LIST></VOUCHER>\
</COLLECTION></DATA></BODY></ENVELOPE>"
            .to_string()
    }

    #[test]
    fn lab_guard_config_requires_all_env_vars() {
        let _guard = lock_env();
        std::env::remove_var("BRIDGE_LAB_TARGET_GUID");
        std::env::remove_var("BRIDGE_LAB_DENY_GUIDS");
        assert_eq!(
            LabGuardConfig::from_env(),
            Err("lab_target_guid_required".to_string())
        );
    }

    #[test]
    fn lab_guard_config_rejects_target_in_its_own_deny_list() {
        // Pure parsing: exercises `from_values` directly, no process env
        // touched, so this is safe under parallel test threads.
        let result =
            LabGuardConfig::from_values(TARGET_GUID, &format!("{DENY_GUID_1},{TARGET_GUID}"));
        assert_eq!(result, Err("lab_deny_guids_invalid".to_string()));
    }

    #[test]
    fn lab_guard_config_parses_a_valid_comma_list() {
        let config =
            LabGuardConfig::from_values(TARGET_GUID, &format!(" {DENY_GUID_1} , {DENY_GUID_2} "))
                .expect("valid config parses");
        assert_eq!(config.target_guid, TARGET_GUID.to_ascii_lowercase());
        assert_eq!(config.deny_guids.len(), 2);
    }

    #[test]
    fn lab_guard_config_from_env_reads_the_real_env_vars() {
        let _guard = lock_env();
        std::env::set_var("BRIDGE_LAB_TARGET_GUID", TARGET_GUID);
        std::env::set_var("BRIDGE_LAB_DENY_GUIDS", DENY_GUID_1);
        let config = LabGuardConfig::from_env();
        std::env::remove_var("BRIDGE_LAB_TARGET_GUID");
        std::env::remove_var("BRIDGE_LAB_DENY_GUIDS");
        let config = config.expect("valid env parses");
        assert_eq!(config.target_guid, TARGET_GUID.to_ascii_lowercase());
        assert_eq!(config.deny_guids, vec![DENY_GUID_1.to_ascii_lowercase()]);
    }

    #[test]
    fn env_lab_writes_enabled_requires_exact_truthy_value() {
        let _guard = lock_env();
        std::env::remove_var("BRIDGE_LAB_WRITES");
        assert!(!env_lab_writes_enabled());
        std::env::set_var("BRIDGE_LAB_WRITES", "1");
        assert!(env_lab_writes_enabled());
        std::env::set_var("BRIDGE_LAB_WRITES", "yes");
        assert!(!env_lab_writes_enabled());
        std::env::remove_var("BRIDGE_LAB_WRITES");
    }

    #[test]
    fn parses_units_from_a_synthetic_collection() {
        let rows = parse_lab_master_rows(&synthetic_unit_collection(), "Unit").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("NAME").unwrap(), "Nos");
        let json = lab_master_json(LabMasterKind::Unit, &rows[0]);
        assert_eq!(json["decimal_places"], "0");
    }

    #[test]
    fn parses_stock_item_opening_and_gst_hsn_fields() {
        let rows = parse_lab_master_rows(&synthetic_stock_item_collection(), "StockItem").unwrap();
        assert_eq!(rows.len(), 1);
        let json = lab_master_json(LabMasterKind::StockItem, &rows[0]);
        assert_eq!(json["parent"], "Chemicals");
        assert_eq!(json["base_unit"], "Kgs");
        assert_eq!(json["opening_qty"], "100");
        assert_eq!(json["opening_rate"], "50.00");
        assert_eq!(json["opening_value"], "5000.00");
        assert_eq!(json["hsn_code"], "28362000");
    }

    #[test]
    fn master_readback_decodes_entities_in_parent_names() {
        // 2026-09-14 coordinator finding, live rehearsal: the target's
        // ledgers read back with PARENT "Duties  Taxes" / "Loans  Advances
        // (Asset)" (note the double space -- the `&amp;` entity dropped
        // entirely, not merely left literal) while production `ledger_masters`
        // against the same company correctly reported "Duties & Taxes".
        // Root cause: quick_xml delivers `&amp;` as its own `GeneralRef`
        // event, separate from the surrounding `Text` events, and this
        // parser had no arm for it.
        let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<LEDGER NAME=\"GST Paid\"><PARENT>Loans &amp; Advances (Asset)</PARENT>\
<OPENINGBALANCE>0.00</OPENINGBALANCE></LEDGER>\
<LEDGER NAME=\"Output CGST 9%\"><PARENT>Duties &amp; Taxes</PARENT>\
<TAXTYPE>GST</TAXTYPE></LEDGER>\
</COLLECTION></DATA></BODY></ENVELOPE>";
        let rows = parse_lab_master_rows(xml, "Ledger").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].get("PARENT").map(String::as_str),
            Some("Loans & Advances (Asset)")
        );
        assert_eq!(
            rows[1].get("PARENT").map(String::as_str),
            Some("Duties & Taxes")
        );
        // Never the entity literal, and never dropped to a bare double space.
        for row in &rows {
            let parent = row.get("PARENT").unwrap();
            assert!(!parent.contains("&amp;"));
            assert!(!parent.contains("  "));
        }
    }

    #[test]
    fn a_non_row_child_of_collection_is_refused() {
        let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<CMPINFO><UNIT>2</UNIT></CMPINFO></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(
            parse_lab_master_rows(xml, "Unit"),
            Err("agent_read_protocol_invalid".to_string())
        );
    }

    #[test]
    fn parses_inventory_entries_per_voucher_and_honours_the_window() {
        let rows = parse_lab_inventory_vouchers(&synthetic_inventory_voucher_collection()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["date"], "20260405");
        assert_eq!(rows[0]["voucher_number"], "1");
        let entries = rows[0]["inventory_entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["stock_item"], "Sodium Bicarbonate");
        assert_eq!(entries[0]["actual_qty"], "100");
        assert_eq!(entries[0]["godown"], "Main Godown");
        assert!(window_honoured(&rows, "20260401", "20260430"));
        assert!(!window_honoured(&rows, "20260401", "20260404"));
    }

    #[test]
    fn parses_nested_batch_allocations_under_an_inventory_entry() {
        let rows = parse_lab_inventory_vouchers(&synthetic_inventory_voucher_with_batch()).unwrap();
        assert_eq!(rows.len(), 1);
        let entries = rows[0]["inventory_entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let batches = entries[0]["batch_allocations"].as_array().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0]["batch"], "Batch-01");
        assert_eq!(batches[0]["godown"], "Main Godown");
        assert_eq!(batches[0]["actual_qty"], "50");
    }

    #[test]
    fn a_non_voucher_child_of_collection_is_refused_for_inventory_vouchers() {
        let xml = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>\
<CMPINFO><VOUCHER>2</VOUCHER></CMPINFO></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(
            parse_lab_inventory_vouchers(xml),
            Err("agent_read_protocol_invalid".to_string())
        );
    }

    #[test]
    fn render_lab_master_collection_carries_the_exact_company_name() {
        let request =
            render_lab_master_collection("BRIDGE CORPUS GST", LabMasterKind::StockItem).unwrap();
        assert!(request.contains("<SVCURRENTCOMPANY>BRIDGE CORPUS GST</SVCURRENTCOMPANY>"));
        assert!(request.contains("<TYPE>StockItem</TYPE>"));
    }
}
