//! Change parse for the local MCP adapter.
use super::*;

pub(super) fn parse_company_high_water(xml: &str, expected_guid: &str) -> Result<Value, String> {
    validate_agent_envelope(xml)?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::<BTreeMap<String, String>>::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut tag = String::new();
    let mut scope = NativeCollectionScope::default();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if name == "COMPANY" && scope.collection() {
                    current = Some(BTreeMap::new());
                }
                scope.start(name.clone());
                tag = name;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let (Some(row), Ok(value)) = (
                    current.as_mut().filter(|_| scope.field("COMPANY")),
                    text.decode(),
                ) {
                    row.insert(tag.clone(), value.into_owned());
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.row("COMPANY") {
                    if let Some(row) = current.take() {
                        rows.push(row);
                    }
                }
                scope.end(&end)?;
                tag.clear();
            }
            Ok(quick_xml::events::Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.collection() && matches!(name.as_str(), "COMPANY" | "LEDGER" | "GROUP") {
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
    let row = rows
        .into_iter()
        .find(|row| {
            row.get("GUID")
                .is_some_and(|guid| guid.eq_ignore_ascii_case(expected_guid))
        })
        .ok_or_else(|| "company_high_water_identity_absent".to_string())?;
    Ok(json!({
        "altvchid": observed_checkpoint(row.get("ALTVCHID"), "voucher")?,
        "altmstid": observed_checkpoint(row.get("ALTMSTID"), "master")?,
    }))
}

pub(super) fn parse_master_domain_high_water(xml: &str) -> Result<u64, String> {
    validate_agent_envelope(xml)?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_domain_row = false;
    let mut tag = String::new();
    let mut scope = NativeCollectionScope::default();
    let mut high_water = 0_u64;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if matches!(tag.as_str(), "LEDGER" | "GROUP") && scope.collection() {
                    in_domain_row = true;
                }
                scope.start(tag.clone());
            }
            Ok(quick_xml::events::Event::Text(text))
                if in_domain_row
                    && (scope.field("LEDGER") || scope.field("GROUP"))
                    && tag == "ALTERID" =>
            {
                let value = text
                    .decode()
                    .map_err(|_| "master_checkpoint_invalid".to_string())?;
                high_water = high_water.max(
                    value
                        .trim()
                        .parse::<u64>()
                        .map_err(|_| "master_checkpoint_invalid".to_string())?,
                );
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.row("LEDGER") || scope.row("GROUP") {
                    in_domain_row = false;
                }
                scope.end(&end)?;
                tag.clear();
            }
            Ok(quick_xml::events::Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.collection() && matches!(name.as_str(), "COMPANY" | "LEDGER" | "GROUP") {
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
    Ok(high_water)
}

pub(super) fn checkpoint_arg(args: &Value, field: &str) -> Result<Option<u64>, String> {
    match args.get(field) {
        None => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| "checkpoint_invalid".to_string()),
        Some(Value::String(value)) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| "checkpoint_invalid".to_string()),
        Some(_) => Err("checkpoint_invalid".to_string()),
    }
}

pub(super) fn checkpoint_advanceable(
    returned_max: Option<u64>,
    requested_checkpoint: u64,
    company_high_water: u64,
    truncated: bool,
) -> bool {
    !truncated && returned_max.unwrap_or(requested_checkpoint) >= company_high_water
}

pub(super) fn observed_checkpoint(value: Option<&String>, axis: &str) -> Result<u64, String> {
    value
        .ok_or_else(|| format!("{axis}_checkpoint_not_observed"))?
        .trim()
        .parse::<u64>()
        .map_err(|_| format!("{axis}_checkpoint_invalid"))
}

pub(super) fn parse_agent_changed_masters(xml: &str) -> Result<Vec<Value>, String> {
    validate_agent_envelope(xml)?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    let mut current: Option<(String, BTreeMap<String, String>)> = None;
    let mut tag = String::new();
    let mut scope = NativeCollectionScope::default();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let next = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if matches!(next.as_str(), "LEDGER" | "GROUP") && scope.collection() {
                    current = Some((next.clone(), BTreeMap::new()));
                }
                scope.start(next.clone());
                tag = next;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some((_, fields)) = current
                    .as_mut()
                    .filter(|_| scope.field("LEDGER") || scope.field("GROUP"))
                {
                    append_agent_text(fields, &tag, decoded_agent_text(text)?);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                if let Some((_, fields)) = current
                    .as_mut()
                    .filter(|_| scope.field("LEDGER") || scope.field("GROUP"))
                {
                    append_agent_text(fields, &tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.row("LEDGER") || scope.row("GROUP") {
                    if let Some((kind, fields)) = current.take() {
                        let alter_id = fields
                            .get("ALTERID")
                            .and_then(|value| value.trim().parse::<u64>().ok())
                            .ok_or_else(|| "change_row_alterid_invalid".to_string())?;
                        let name = fields
                            .get("NAME")
                            .filter(|name| !name.trim().is_empty())
                            .ok_or_else(|| "change_row_name_invalid".to_string())?;
                        let guid = fields.get("GUID").filter(|value| !value.trim().is_empty());
                        let master_id = fields
                            .get("MASTERID")
                            .filter(|value| !value.trim().is_empty());
                        if guid.is_none() && master_id.is_none() {
                            return Err("change_row_identity_invalid".to_string());
                        }
                        rows.push(json!({"kind": kind.to_ascii_lowercase(), "name": name, "parent": fields.get("PARENT"), "alter_id": alter_id, "guid": guid, "master_id": master_id}));
                    }
                }
                scope.end(&end)?;
                tag.clear();
            }
            Ok(quick_xml::events::Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.collection() && matches!(name.as_str(), "COMPANY" | "LEDGER" | "GROUP") {
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

pub(super) fn validate_change_row_alter_ids(rows: &[Value]) -> Result<(), String> {
    if rows
        .iter()
        .any(|row| row.get("alter_id").and_then(Value::as_u64).is_none())
    {
        Err("change_row_alterid_invalid".to_string())
    } else {
        Ok(())
    }
}

pub(super) fn stable_change_page(
    mut rows: Vec<Value>,
    checkpoint: u64,
    snapshot: u64,
    max_rows: usize,
) -> Result<(Vec<Value>, bool, u64), String> {
    if checkpoint > snapshot {
        return Err("change_checkpoint_exceeds_snapshot".to_string());
    }
    validate_change_row_alter_ids(&rows)?;
    rows.retain(|row| {
        row["alter_id"]
            .as_u64()
            .is_some_and(|alter_id| alter_id > checkpoint && alter_id <= snapshot)
    });
    rows.sort_by_key(|row| row["alter_id"].as_u64());
    let truncated = rows.len() > max_rows;
    rows.truncate(max_rows);
    let next_cursor = if truncated {
        rows.last()
            .and_then(|row| row["alter_id"].as_u64())
            .ok_or_else(|| "change_page_cursor_invalid".to_string())?
    } else {
        snapshot
    };
    Ok((rows, truncated, next_cursor))
}
