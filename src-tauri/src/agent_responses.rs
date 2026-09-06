//! Responses for the local MCP adapter.
use super::*;

pub(super) fn enforce_response_byte_cap(
    mut response: Value,
    max_bytes: usize,
) -> Result<(Value, bool, usize), String> {
    if response.to_string().len() <= max_bytes {
        let rows = response_row_count(&response).unwrap_or_default();
        return Ok((response, false, rows));
    }
    while response.to_string().len() > max_bytes {
        if !truncate_response_items(&mut response)? {
            return Err("agent_response_too_large".to_string());
        }
    }
    let rows = response_row_count(&response).unwrap_or_default();
    Ok((response, true, rows))
}

pub(super) fn truncate_response_items(response: &mut Value) -> Result<bool, String> {
    let change_axis = ["vouchers", "masters"]
        .into_iter()
        .filter_map(|key| {
            let cursor_key = if key == "vouchers" {
                "next_voucher_alter_id"
            } else {
                "next_master_alter_id"
            };
            let fallback_key = if key == "vouchers" {
                "voucher_alter_id"
            } else {
                "master_alter_id"
            };
            (response["result"][cursor_key].is_u64() && response["result"][fallback_key].is_u64())
                .then(|| {
                    response["result"][key]
                        .as_array()
                        .map(|rows| (key, rows.len()))
                })
                .flatten()
        })
        .max_by_key(|(_, length)| *length)
        .filter(|(_, length)| *length > 0)
        .map(|(key, _)| key);
    if let Some(key) = change_axis {
        let cursor_key = if key == "vouchers" {
            "next_voucher_alter_id"
        } else {
            "next_master_alter_id"
        };
        let fallback_key = if key == "vouchers" {
            "voucher_alter_id"
        } else {
            "master_alter_id"
        };
        let rows = response["result"][key]
            .as_array_mut()
            .expect("non-empty change axis");
        rows.pop();
        let cursor = rows
            .iter()
            .filter_map(|row| row["alter_id"].as_u64())
            .max()
            .or_else(|| response["result"][fallback_key].as_u64())
            .ok_or_else(|| "change_page_cursor_invalid".to_string())?;
        response["truncated"] = Value::Bool(true);
        response["result"][cursor_key] = json!(cursor);
        response["result"]["checkpoint_advanceable"] = Value::Bool(false);
        return Ok(true);
    }
    let requested_offset = response["result"]["offset"].as_u64().unwrap_or(0);
    if response["result"]["open_bills"].is_array()
        || response["result"]["unallocated"]["parties"].is_array()
    {
        return truncate_outstandings_page(response, requested_offset);
    }
    for key in ["items", "ledgers"] {
        if let Some(items) = response["result"][key]
            .as_array_mut()
            .filter(|items| !items.is_empty())
        {
            if items.len() == 1 {
                return Err("agent_response_too_large".to_string());
            }
            items.pop();
            let remaining = items.len();
            response["truncated"] = Value::Bool(true);
            response["result"]["next_offset"] = json!(requested_offset + remaining as u64);
            return Ok(true);
        }
    }
    Ok(false)
}

fn truncate_outstandings_page(response: &mut Value, offset: u64) -> Result<bool, String> {
    let bills = response["result"]["open_bills"]
        .as_array()
        .map_or(0, Vec::len);
    let parties = response["result"]["unallocated"]["parties"]
        .as_array()
        .map_or(0, Vec::len);
    let width = bills.max(parties);
    if width == 0 {
        return Ok(false);
    }
    if width == 1 {
        return Err("agent_response_too_large".to_string());
    }
    // Both collections consume the same input offset. Shrink their shared page
    // width together so every continuing cursor advances without skipping rows
    // from the other collection. An already exhausted shorter axis stays intact.
    let remaining = width - 1;
    if bills > remaining {
        response["result"]["open_bills"]
            .as_array_mut()
            .unwrap()
            .truncate(remaining);
        response["result"]["next_offset"] = json!(offset + remaining as u64);
    }
    if parties > remaining {
        response["result"]["unallocated"]["parties"]
            .as_array_mut()
            .unwrap()
            .truncate(remaining);
        response["result"]["unallocated"]["truncated"] = Value::Bool(true);
        response["result"]["unallocated"]["next_offset"] = json!(offset + remaining as u64);
    }
    response["truncated"] = Value::Bool(true);
    Ok(true)
}

pub(super) fn response_row_count(response: &Value) -> Option<usize> {
    let result = &response["result"];
    if let Some(vouchers) = result["vouchers"].as_array() {
        return Some(vouchers.len() + result["masters"].as_array().map_or(0, Vec::len));
    }
    // Receipts count each released outstandings row collection: open bills and
    // unallocated parties. Top parties are a derived ranking summary, not a
    // separately paged row collection, so they are intentionally excluded.
    if result["open_bills"].is_array() || result["unallocated"]["parties"].is_array() {
        return Some(
            result["open_bills"].as_array().map_or(0, Vec::len)
                + result["unallocated"]["parties"]
                    .as_array()
                    .map_or(0, Vec::len),
        );
    }
    [
        "items",
        "ledgers",
        "records",
        "companies",
        "masters",
        "loaded_companies",
    ]
    .into_iter()
    .find_map(|key| result[key].as_array().map(Vec::len))
}

pub(super) fn set_mcp_content_json(mcp_response: &mut Value) {
    // MCP 2025-06-18 Tools: preserve the complete payload for clients that only
    // consume TextContent, including the supported 2024-11-05 protocol.
    mcp_response["content"][0]["text"] =
        Value::String(mcp_response["structuredContent"].to_string());
}

pub(super) fn enforce_mcp_result_byte_cap(
    mcp_response: &mut Value,
    max_bytes: usize,
    _name: &str,
    _fallback_rows: usize,
) -> Result<(), String> {
    set_mcp_content_json(mcp_response);
    while mcp_response.to_string().len() > max_bytes {
        let structured = mcp_response
            .get_mut("structuredContent")
            .ok_or_else(|| "agent_response_too_large".to_string())?;
        if !truncate_response_items(structured)? {
            return Err("agent_response_too_large".to_string());
        }
        set_mcp_content_json(mcp_response);
    }
    Ok(())
}

pub(super) fn enforce_jsonrpc_response_byte_cap(
    response: &mut Value,
    max_bytes: usize,
) -> Result<(), String> {
    while response.to_string().len() + 1 > max_bytes {
        let result = response
            .get_mut("result")
            .ok_or_else(|| "agent_response_too_large".to_string())?;
        let structured = result
            .get_mut("structuredContent")
            .ok_or_else(|| "agent_response_too_large".to_string())?;
        if !truncate_response_items(structured)? {
            return Err("agent_response_too_large".to_string());
        }
        set_mcp_content_json(result);
    }
    Ok(())
}

#[cfg(test)]
#[path = "agent_response_tests.rs"]
mod tests;
