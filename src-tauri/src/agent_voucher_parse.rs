//! Voucher parse for the local MCP adapter.
use super::*;

/// Shares the native collection boundary used by the protocol crate: CMPINFO
/// contains identically named counters outside BODY/DATA/COLLECTION.
#[derive(Default)]
pub(super) struct NativeCollectionScope {
    path: Vec<String>,
    collection_seen: bool,
}

impl NativeCollectionScope {
    pub(super) fn collection(&self) -> bool {
        self.path == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
    }
    pub(super) fn row(&self, row: &str) -> bool {
        self.path.len() == 5
            && self.path[..4] == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
            && self.path[4] == row
    }
    pub(super) fn field(&self, row: &str) -> bool {
        self.path.len() == 6
            && self.path[..4] == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
            && self.path[4] == row
    }
    pub(super) fn child(&self, row: &str, child: &str) -> bool {
        self.field(row) && self.path[5] == child
    }
    pub(super) fn entry_field(&self) -> bool {
        self.path.len() == 7
            && self.path[..5] == ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"]
            && self.path[5] == "ALLLEDGERENTRIES.LIST"
    }
    pub(super) fn start(&mut self, name: String) {
        if self.path == ["ENVELOPE", "BODY", "DATA"] && name == "COLLECTION" {
            self.collection_seen = true;
        }
        self.path.push(name);
    }
    pub(super) fn end(&mut self, name: &str) -> Result<(), String> {
        if self.path.pop().as_deref() != Some(name) {
            return Err("agent_read_protocol_invalid".to_string());
        }
        Ok(())
    }
    pub(super) fn finish(&self) -> Result<(), String> {
        if self.collection_seen && self.path.is_empty() {
            Ok(())
        } else {
            Err("agent_read_protocol_invalid".to_string())
        }
    }
}

pub(super) fn parse_agent_rows(xml: &str) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, false)
}

pub(super) fn parse_agent_changed_rows(xml: &str) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, true)
}

pub(super) fn parse_agent_rows_with_accounting_state(
    xml: &str,
    require_accounting_state: bool,
) -> Result<Vec<Value>, String> {
    // Tally's collection XML varies by release; use a deliberately conservative
    // extractor and never infer a missing field. Malformed rows fail before
    // optional selectors can hide them as an apparently complete empty result.
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    validate_agent_envelope(xml, "VOUCHER")?;
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut entries = Vec::<Value>::new();
    let mut current_tag = String::new();
    let mut scope = NativeCollectionScope::default();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if tag == "VOUCHER" && scope.collection() {
                    current = Some(BTreeMap::new());
                    entries.clear();
                }
                if tag == "ALLLEDGERENTRIES.LIST" && scope.row("VOUCHER") {
                    entry = Some(BTreeMap::new());
                }
                scope.start(tag.clone());
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST") {
                    if let (Some(_), Some(entry_row)) = (current.as_mut(), entry.take()) {
                        let ledger = entry_row
                            .get("LEDGERNAME")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let amount = entry_row
                            .get("AMOUNT")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let polarity = entry_row
                            .get("ISDEEMEDPOSITIVE")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        entries.push(json!({
                            "ledger": ledger,
                            "amount": amount,
                            "is_deemed_positive": polarity,
                        }));
                    }
                } else if scope.row("VOUCHER") {
                    if let Some(row) = current.take() {
                        if ["DATE", "VOUCHERTYPENAME"].iter().any(|field| {
                            row.get(*field).is_none_or(|value| value.trim().is_empty())
                        }) {
                            return Err(if require_accounting_state {
                                "change_row_core_field_invalid"
                            } else {
                                "agent_read_protocol_invalid"
                            }
                            .to_string());
                        }
                        if require_accounting_state
                            && row
                                .get("GUID")
                                .filter(|value| !value.trim().is_empty())
                                .is_none()
                            && row
                                .get("MASTERID")
                                .filter(|value| !value.trim().is_empty())
                                .is_none()
                        {
                            return Err("change_row_identity_invalid".to_string());
                        }
                        let amounts = std::mem::take(&mut entries);
                        let mut parsed = json!({"date": row.get("DATE"), "voucher_number": row.get("VOUCHERNUMBER"), "voucher_type": row.get("VOUCHERTYPENAME"), "party": row.get("PARTYLEDGERNAME"), "narration": row.get("NARRATION"), "guid": row.get("GUID"), "alter_id": row.get("ALTERID").and_then(|v| v.trim().parse::<u64>().ok()), "master_id": row.get("MASTERID"), "amounts": amounts});
                        if require_accounting_state {
                            parsed["cancelled"] =
                                Value::Bool(required_tally_bool(row.get("ISCANCELLED"))?);
                            parsed["optional"] =
                                Value::Bool(required_tally_bool(row.get("ISOPTIONAL"))?);
                        }
                        rows.push(parsed);
                    }
                }
                scope.end(&end)?;
                current_tag.clear();
            }
            Ok(quick_xml::events::Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.collection() && name == "VOUCHER" {
                    return Err("agent_read_protocol_invalid".to_string());
                }
                scope.start(name.clone());
                scope.end(&name)?;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
            _ => {}
        }
    }
    scope.finish()?;
    Ok(rows)
}

pub(super) fn append_agent_text(row: &mut BTreeMap<String, String>, tag: &str, value: String) {
    row.entry(tag.to_string()).or_default().push_str(&value);
}

pub(super) fn decoded_agent_text(text: quick_xml::events::BytesText<'_>) -> Result<String, String> {
    let decoded = text
        .decode()
        .map_err(|_| "agent_read_protocol_invalid".to_string())?;
    quick_xml::escape::unescape(&decoded)
        .map(|value| value.into_owned())
        .map_err(|_| "agent_read_protocol_invalid".to_string())
}

pub(super) fn decoded_agent_reference(
    reference: quick_xml::events::BytesRef<'_>,
) -> Result<String, String> {
    let reference = reference
        .decode()
        .map_err(|_| "agent_read_protocol_invalid".to_string())?;
    quick_xml::escape::unescape(&format!("&{reference};"))
        .map(|value| value.into_owned())
        .map_err(|_| "agent_read_protocol_invalid".to_string())
}

pub(super) fn required_tally_bool(value: Option<&String>) -> Result<bool, String> {
    match value.map(String::as_str).map(str::trim) {
        Some("Yes") => Ok(true),
        Some("No") => Ok(false),
        _ => Err("voucher_accounting_state_not_observed".to_string()),
    }
}
