//! Responses for the local MCP adapter.
use super::*;

#[derive(Clone, Copy)]
enum PageShape {
    Changes,
    Outstandings,
    Rows(&'static str),
}

const CHANGE_AXES: [(&str, &str, &str); 2] = [
    ("vouchers", "next_voucher_alter_id", "voucher_alter_id"),
    ("masters", "next_master_alter_id", "master_alter_id"),
];

fn page_shape(response: &Value) -> Option<(PageShape, usize)> {
    let result = &response["result"];
    let change_width = CHANGE_AXES
        .iter()
        .filter_map(|(key, cursor, fallback)| {
            (result[cursor].is_u64() && result[fallback].is_u64())
                .then(|| result[key].as_array().map(Vec::len))
                .flatten()
        })
        .max()
        .unwrap_or(0);
    if change_width > 0 {
        return Some((PageShape::Changes, change_width));
    }
    if result["open_bills"].is_array() || result["unallocated"]["parties"].is_array() {
        return Some((
            PageShape::Outstandings,
            result["open_bills"].as_array().map_or(0, Vec::len).max(
                result["unallocated"]["parties"]
                    .as_array()
                    .map_or(0, Vec::len),
            ),
        ));
    }
    ["items", "ledgers"].into_iter().find_map(|key| {
        result[key]
            .as_array()
            .map(|rows| (PageShape::Rows(key), rows.len()))
    })
}

fn retain_page_width(response: &mut Value, shape: PageShape, width: usize) -> Result<(), String> {
    if width == 0 {
        return Err("agent_response_too_large".into());
    }
    let result = &mut response["result"];
    let offset = result["offset"].as_u64().unwrap_or(0);
    match shape {
        PageShape::Changes => {
            for (key, cursor, fallback) in CHANGE_AXES {
                if !result[cursor].is_u64() || !result[fallback].is_u64() {
                    continue;
                }
                if let Some(rows) = result[key].as_array_mut().filter(|rows| rows.len() > width) {
                    rows.truncate(width);
                    let next = rows
                        .iter()
                        .filter_map(|row| row["alter_id"].as_u64())
                        .max()
                        .ok_or_else(|| "change_page_cursor_invalid".to_string())?;
                    result[cursor] = json!(next);
                    result["checkpoint_advanceable"] = json!(false);
                }
            }
        }
        PageShape::Outstandings => {
            // Both collections consume one input offset. Keep their shared
            // prefix width; exhausted shorter axes retain their null cursor.
            if let Some(rows) = result["open_bills"]
                .as_array_mut()
                .filter(|rows| rows.len() > width)
            {
                rows.truncate(width);
                result["next_offset"] = json!(offset + width as u64);
            }
            if let Some(rows) = result["unallocated"]["parties"]
                .as_array_mut()
                .filter(|rows| rows.len() > width)
            {
                rows.truncate(width);
                result["unallocated"]["next_offset"] = json!(offset + width as u64);
                result["unallocated"]["truncated"] = json!(true);
            }
        }
        PageShape::Rows(key) => {
            result[key]
                .as_array_mut()
                .expect("observed page rows")
                .truncate(width);
            result["next_offset"] = json!(offset + width as u64);
        }
    }
    response["truncated"] = json!(true);
    Ok(())
}

// Measure O(log n) prefix candidates instead of serializing once for each
// discarded row. Each candidate retains at least one row on every active axis.
// The caller measures the actual outer envelope, including duplicated text and
// the wire newline, so escaping and final framing remain part of the byte cap.
fn fit_response(
    response: &mut Value,
    structured_path: &str,
    max_bytes: usize,
    mut encoded_len: impl FnMut(&mut Value) -> usize,
) -> Result<bool, String> {
    if encoded_len(response) <= max_bytes {
        return Ok(false);
    }
    let (shape, width) = response
        .pointer(structured_path)
        .and_then(page_shape)
        .ok_or_else(|| "agent_response_too_large".to_string())?;
    let (mut lower, mut upper) = (1, width.saturating_sub(1));
    let original = response.clone();
    let mut best = None;
    while lower <= upper {
        let keep = lower + (upper - lower) / 2;
        let mut candidate = original.clone();
        retain_page_width(
            candidate
                .pointer_mut(structured_path)
                .expect("observed payload"),
            shape,
            keep,
        )?;
        if encoded_len(&mut candidate) <= max_bytes {
            best = Some(candidate);
            lower = keep + 1;
        } else {
            upper = keep - 1;
        }
    }
    *response = best.ok_or_else(|| "agent_response_too_large".to_string())?;
    Ok(true)
}

pub(super) fn enforce_response_byte_cap(
    mut response: Value,
    max_bytes: usize,
) -> Result<(Value, bool, usize), String> {
    let truncated = fit_response(&mut response, "", max_bytes, |value| {
        value.to_string().len()
    })?;
    let rows = response_row_count(&response).unwrap_or_default();
    Ok((response, truncated, rows))
}

#[cfg(test)]
pub(super) fn truncate_response_items(response: &mut Value) -> Result<bool, String> {
    let Some((shape, width)) = page_shape(response).filter(|(_, width)| *width > 0) else {
        return Ok(false);
    };
    retain_page_width(response, shape, width - 1)?;
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
    // Receipt counting does not imply pagination support. Only page_shape
    // determines which arrays can be trimmed with a resumable cursor.
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
    mcp_response["content"] =
        json!([{"type":"text","text":mcp_response["structuredContent"].to_string()}]);
}

pub(super) fn enforce_mcp_result_byte_cap(
    mcp_response: &mut Value,
    max_bytes: usize,
    _name: &str,
    _fallback_rows: usize,
) -> Result<(), String> {
    fit_response(mcp_response, "/structuredContent", max_bytes, |value| {
        set_mcp_content_json(value);
        value.to_string().len()
    })
    .map(|_| ())
}

pub(super) fn enforce_jsonrpc_response_byte_cap(
    response: &mut Value,
    max_bytes: usize,
) -> Result<(), String> {
    fit_response(response, "/result/structuredContent", max_bytes, |value| {
        if value["result"]["structuredContent"].is_object() {
            set_mcp_content_json(&mut value["result"]);
        }
        value.to_string().len() + 1
    })
    .map(|_| ())
}

#[cfg(test)]
#[path = "agent_response_tests.rs"]
mod tests;
