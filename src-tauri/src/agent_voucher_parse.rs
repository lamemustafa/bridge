//! Voucher parse for the local MCP adapter.
use super::*;

#[path = "agent_voucher_scalars.rs"]
mod scalars;
pub(super) use scalars::*;

/// Shares the native collection boundary used by the protocol crate: CMPINFO
/// contains identically named counters outside BODY/DATA/COLLECTION.
#[derive(Default)]
pub(super) struct NativeCollectionScope {
    path: Vec<String>,
    collection_seen: bool,
    repeated_collection: bool,
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
    pub(super) fn bill_allocation(&self) -> bool {
        self.path.len() == 7
            && self.path[..5] == ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"]
            && self.path[5] == "ALLLEDGERENTRIES.LIST"
            && self.path[6] == "BILLALLOCATIONS.LIST"
    }
    pub(super) fn bill_allocation_field(&self) -> bool {
        self.path.len() == 8
            && self.path[..5] == ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"]
            && self.path[5] == "ALLLEDGERENTRIES.LIST"
            && self.path[6] == "BILLALLOCATIONS.LIST"
    }
    pub(super) fn voucher_scalar(&self) -> bool {
        self.path.last().is_some_and(|field| {
            (self.field("VOUCHER") && is_voucher_scalar(field))
                || (self.entry_field() && is_voucher_entry_scalar(field))
                || (self.bill_allocation_field() && is_voucher_bill_allocation_scalar(field))
        })
    }
    pub(super) fn start(&mut self, name: String) {
        if self.path == ["ENVELOPE", "BODY", "DATA"] && name == "COLLECTION" {
            self.repeated_collection |= self.collection_seen;
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
        if self.collection_seen && !self.repeated_collection && self.path.is_empty() {
            Ok(())
        } else {
            Err("agent_read_protocol_invalid".to_string())
        }
    }
}

pub(super) fn parse_agent_rows(xml: &str, company_guid: &str) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, false, company_guid)
}

pub(super) fn parse_agent_changed_rows(
    xml: &str,
    company_guid: &str,
) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, true, company_guid)
}

pub(super) fn parse_agent_rows_with_accounting_state(
    xml: &str,
    require_change_identity: bool,
    company_guid: &str,
) -> Result<Vec<Value>, String> {
    // Tally's collection XML varies by release; use a deliberately conservative
    // extractor and never infer a missing field. Malformed rows fail before
    // optional selectors can hide them as an apparently complete empty result.
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    let mut identities = VoucherSourceIdentities::default();
    validate_agent_envelope(xml)?;
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut allocation: Option<BTreeMap<String, String>> = None;
    let mut entries = Vec::<Value>::new();
    let mut allocations = Vec::<Value>::new();
    let mut current_tag = String::new();
    let mut scope = NativeCollectionScope::default();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.voucher_scalar() || (scope.collection() && tag != "VOUCHER") {
                    return Err("agent_read_protocol_invalid".into());
                }
                if tag == "VOUCHER" && scope.collection() {
                    let mut row = BTreeMap::new();
                    for attribute in event.attributes() {
                        let attribute =
                            attribute.map_err(|_| "agent_read_protocol_invalid".to_string())?;
                        if attribute.key.as_ref().eq_ignore_ascii_case(b"REMOTEID") {
                            claim_agent_scalar(&mut row, "REMOTEID")?;
                            row.insert(
                                "REMOTEID".into(),
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
                    entries.clear();
                }
                if tag == "ALLLEDGERENTRIES.LIST" && scope.row("VOUCHER") {
                    entry = Some(BTreeMap::new());
                    allocations.clear();
                }
                if tag == "BILLALLOCATIONS.LIST" && scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST")
                {
                    allocation = Some(BTreeMap::new());
                }
                claim_voucher_scalar(
                    &scope,
                    &tag,
                    current.as_mut(),
                    entry.as_mut(),
                    allocation.as_mut(),
                )?;
                scope.start(tag.clone());
                if scope.repeated_collection {
                    return Err("agent_read_protocol_invalid".into());
                }
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some(row) = allocation
                    .as_mut()
                    .filter(|_| scope.bill_allocation_field())
                {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                } else if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                }
            }
            Ok(quick_xml::events::Event::CData(text)) => {
                let value = text
                    .decode()
                    .map_err(|_| "agent_read_protocol_invalid".to_string())?
                    .into_owned();
                if let Some(row) = allocation
                    .as_mut()
                    .filter(|_| scope.bill_allocation_field())
                {
                    append_agent_text(row, &current_tag, value);
                } else if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, value);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, value);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                if let Some(row) = allocation
                    .as_mut()
                    .filter(|_| scope.bill_allocation_field())
                {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                } else if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.bill_allocation() {
                    if let Some(allocation_row) = allocation.take().filter(|row| !row.is_empty()) {
                        let bill_type = allocation_row
                            .get("BILLTYPE")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "bill_allocation_field_missing".to_string())?;
                        let amount = allocation_row
                            .get("AMOUNT")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "bill_allocation_field_missing".to_string())?;
                        bridge_tally_core::ExactDecimal::parse(amount.clone())
                            .map_err(|_| "bill_allocation_amount_invalid".to_string())?;
                        let reference = if bill_type.trim() == "On Account" {
                            // On Account is the one bill type with no bill identity. Keep that
                            // absence explicit instead of representing it as an empty name.
                            json!({"kind": "on_account"})
                        } else {
                            let name = allocation_row
                                .get("NAME")
                                .filter(|value| !value.trim().is_empty())
                                .ok_or_else(|| "bill_allocation_field_missing".to_string())?;
                            json!({"kind": "named", "name": name})
                        };
                        allocations.push(json!({
                            "reference": reference,
                            "bill_type": bill_type,
                            "amount": amount,
                        }));
                    }
                } else if scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST") {
                    if allocation.is_some() {
                        return Err("agent_read_protocol_invalid".to_string());
                    }
                    if let (Some(_), Some(entry_row)) = (current.as_mut(), entry.take()) {
                        let ledger = entry_row
                            .get("LEDGERNAME")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let amount = entry_row
                            .get("AMOUNT")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let parsed_amount = bridge_tally_core::ExactDecimal::parse(amount.clone())
                            .map_err(|_| "voucher_amount_invalid".to_string())?;
                        let polarity = entry_row
                            .get("ISDEEMEDPOSITIVE")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        validate_tally_entry_polarity(
                            &parsed_amount,
                            required_tally_bool(Some(polarity))?,
                        )?;
                        entries.push(json!({
                            "ledger": ledger,
                            "amount": amount,
                            "is_deemed_positive": polarity,
                            "bill_allocations": std::mem::take(&mut allocations),
                        }));
                    }
                } else if scope.row("VOUCHER") {
                    if let Some(row) = current.take() {
                        if ["DATE", "VOUCHERTYPENAME"].iter().any(|field| {
                            row.get(*field).is_none_or(|value| value.trim().is_empty())
                        }) {
                            return Err(if require_change_identity {
                                "change_row_core_field_invalid"
                            } else {
                                "agent_read_protocol_invalid"
                            }
                            .to_string());
                        }
                        if !row.get("GUID").is_some_and(|guid| {
                            bridge_tally_protocol::master_guid_belongs_to_company(
                                guid,
                                company_guid,
                            )
                        }) {
                            return Err("voucher_company_identity_invalid".to_string());
                        }
                        bridge_tally_core::TallyDate::parse(row["DATE"].clone())
                            .map_err(|_| "voucher_date_invalid".to_string())?;
                        let master_id = parse_optional_tally_u64(
                            row.get("MASTERID").map(String::as_str),
                            "voucher_master_id_invalid",
                        )?;
                        identities.admit(row.get("GUID").map(String::as_str), master_id)?;
                        let amounts = std::mem::take(&mut entries);
                        let mut parsed = json!({"date": row.get("DATE"), "voucher_number": row.get("VOUCHERNUMBER"), "voucher_type": row.get("VOUCHERTYPENAME"), "party": row.get("PARTYLEDGERNAME"), "narration": row.get("NARRATION"), "guid": row.get("GUID"), "alter_id": parse_optional_tally_alter_id(row.get("ALTERID").map(String::as_str))?, "master_id": row.get("MASTERID"), "amounts": amounts});
                        if require_change_identity {
                            parsed["remote_id"] = json!(row.get("REMOTEID"));
                        }
                        parsed["cancelled"] =
                            Value::Bool(required_tally_bool(row.get("ISCANCELLED"))?);
                        parsed["optional"] =
                            Value::Bool(required_tally_bool(row.get("ISOPTIONAL"))?);
                        rows.push(parsed);
                    }
                }
                scope.end(&end)?;
                current_tag.clear();
            }
            Ok(quick_xml::events::Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.voucher_scalar() {
                    return Err("agent_read_protocol_invalid".into());
                }
                if scope.collection() {
                    return Err("agent_read_protocol_invalid".to_string());
                }
                claim_voucher_scalar(
                    &scope,
                    &name,
                    current.as_mut(),
                    entry.as_mut(),
                    allocation.as_mut(),
                )?;
                scope.start(name.clone());
                if scope.repeated_collection {
                    return Err("agent_read_protocol_invalid".into());
                }
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

fn claim_voucher_scalar(
    scope: &NativeCollectionScope,
    field: &str,
    current: Option<&mut BTreeMap<String, String>>,
    entry: Option<&mut BTreeMap<String, String>>,
    allocation: Option<&mut BTreeMap<String, String>>,
) -> Result<(), String> {
    let row = if scope.row("VOUCHER") && is_voucher_scalar(field) {
        current
    } else if scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST") && is_voucher_entry_scalar(field) {
        entry
    } else if scope.bill_allocation() && is_voucher_bill_allocation_scalar(field) {
        allocation
    } else {
        None
    };
    if let Some(row) = row {
        claim_agent_scalar(row, field)?;
    }
    Ok(())
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

#[cfg(test)]
#[path = "agent_voucher_parse_tests.rs"]
mod boundary_tests;
