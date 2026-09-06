//! Describe the final released JSON shape, never the contained values.
use serde_json::Value;
use std::collections::BTreeSet;

/// Paths are relative to structuredContent and include company/evidence/result.
/// Arrays use `[]` instead of row indices; nulls and empty containers are retained.
///
/// Producer invariant: released object keys are server-defined contract fields
/// (literal json! keys, typed Serialize fields, public voucher-schema properties,
/// or fixed verification status counters). Tally/user identifiers are values,
/// never object keys. Parser scratch maps are projected onto literal output keys;
/// egress-log lines and accounting fingerprints remain strings, not parsed maps.
/// Any future keyed report must preserve that invariant before using this walker.
pub(super) fn released_fields(value: &Value) -> Vec<String> {
    let mut fields = BTreeSet::new();
    visit(value, "", &mut fields);
    fields.into_iter().collect()
}

fn visit(value: &Value, path: &str, fields: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) if !object.is_empty() => {
            for (key, child) in object {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                visit(child, &child_path, fields);
            }
        }
        Value::Array(values) => {
            let item_path = format!("{path}[]");
            if values.is_empty() {
                fields.insert(item_path);
            } else {
                for value in values {
                    visit(value, &item_path, fields);
                }
            }
        }
        _ if !path.is_empty() => {
            fields.insert(path.to_string());
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "agent_receipt_fields_tests.rs"]
mod tests;
