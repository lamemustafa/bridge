//! Public tool catalog and argument admission before any Tally read.
use super::*;

pub(super) fn validate_tool_arguments(name: &str, args: &Value) -> Result<(), String> {
    let arguments = args
        .as_object()
        .ok_or_else(|| "argument_schema_invalid".to_string())?;
    let definitions = registered_tool_definitions(true);
    let schema = &definitions
        .as_array()
        .expect("tool definitions")
        .iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| "tool_not_found".to_string())?["inputSchema"];
    let properties = schema["properties"].as_object();
    for key in arguments.keys() {
        if !properties.is_some_and(|properties| properties.contains_key(key)) {
            return Err(format!("argument_unknown:{key}"));
        }
    }
    for key in schema["required"].as_array().into_iter().flatten() {
        let key = key.as_str().expect("required property name");
        if !arguments.contains_key(key) {
            return Err(format!("{key}_required"));
        }
    }
    // Validate selectors before any company probe. The published tool schema
    // is the sole property/type/enum registry; tool-specific accounting and
    // nested import validation remain at their existing typed boundaries.
    for (key, value) in arguments {
        let property = &schema["properties"][key];
        match property["type"].as_str() {
            Some("string") => {
                let text = value
                    .as_str()
                    .ok_or_else(|| format!("argument_invalid:{key}"))?;
                validate_string_bounds(text, property, key)?;
                if matches!(key.as_str(), "from" | "to" | "as_of") {
                    normalized_date(text)?;
                }
            }
            Some("integer") => {
                if matches!(key.as_str(), "offset" | "limit" | "top") {
                    if property["minimum"].as_u64() == Some(1) {
                        arg_positive_usize(args, key, 1)?;
                    } else {
                        arg_usize(args, key, 0)?;
                    }
                } else {
                    if !value.is_u64() {
                        return Err("checkpoint_invalid".to_string());
                    }
                    checkpoint_arg(args, key)?;
                }
            }
            Some("array") => {
                let values = value
                    .as_array()
                    .ok_or_else(|| format!("argument_invalid:{key}"))?;
                if property["minItems"]
                    .as_u64()
                    .is_some_and(|min| values.len() < min as usize)
                    || property["maxItems"]
                        .as_u64()
                        .is_some_and(|max| values.len() > max as usize)
                {
                    return Err(format!("argument_invalid:{key}"));
                }
                if property["items"]["type"] == "string" {
                    for value in values {
                        let text = value
                            .as_str()
                            .ok_or_else(|| format!("argument_invalid:{key}"))?;
                        validate_string_bounds(text, &property["items"], key)?;
                    }
                }
            }
            _ => {}
        }
        if property["enum"]
            .as_array()
            .is_some_and(|values| !values.contains(value))
        {
            return Err(format!("argument_invalid:{key}"));
        }
    }
    Ok(())
}

fn validate_string_bounds(text: &str, schema: &Value, key: &str) -> Result<(), String> {
    let length = text.chars().count();
    if schema["minLength"]
        .as_u64()
        .is_some_and(|min| length < min as usize)
        || schema["maxLength"]
            .as_u64()
            .is_some_and(|max| length > max as usize)
        || (schema["pattern"] == r"\S" && text.trim().is_empty())
    {
        return Err(format!("argument_invalid:{key}"));
    }
    Ok(())
}

pub(super) fn tool_definitions(import_enabled: bool) -> Value {
    let mut definitions = registered_tool_definitions(import_enabled);
    definitions
        .as_array_mut()
        .expect("tool definitions")
        .retain(|tool| tool["name"] != "changed_since");
    definitions
}

// Retain the internal schema while bounded change enumeration is unqualified.
pub(super) fn registered_tool_definitions(import_enabled: bool) -> Value {
    let names = [
        "tally_status",
        "list_companies",
        "voucher_schema",
        "validate_masters",
        "build_import_xml",
        "verify_import",
        "outstandings",
        "ledger_masters",
        "ledger_movement",
        "vouchers",
        "changed_since",
        "read_evidence",
        "egress_log",
    ];
    Value::Array(
        names
            .into_iter()
            .filter(|name| import_enabled || !matches!(*name, "build_import_xml" | "verify_import"))
            .map(|name| {
                let (description, input_schema) = match name {
                    "voucher_schema" => (
                        "Return the fail-closed local voucher-file schema; no Tally request is sent.",
                        json!({"type":"object", "additionalProperties":false}),
                    ),
                    "validate_masters" => (
                        "Validate 1–100 nonblank ledger names (at most 1024 characters each). Near-miss suggestions are bounded to 25 names and 8192 UTF-8 bytes per requested name, with total count and truncation reported.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","ledgers"], "properties":{"company_guid":{"type":"string"},"ledgers":{"type":"array","minItems":1,"maxItems":agent_import::MAX_MASTER_NAMES,"items":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"}}}}),
                    ),
                    "build_import_xml" => (
                        "Validate and write a local Tally voucher import file. This never dispatches import XML to Tally.",
                        agent_import::voucher_input_schema(),
                    ),
                    "verify_import" => (
                        "Read back a manually imported local batch and write Proof-of-Post files. This never dispatches import XML to Tally.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","batch_id"], "properties":{"company_guid":{"type":"string"},"batch_id":{"type":"string"}}}),
                    ),
                    "tally_status" => (
                        "Return loopback endpoint status and observed loaded-company identity tuples.",
                        json!({"type":"object","additionalProperties":false}),
                    ),
                    "list_companies" => (
                        "Return observed company tuples and identity ambiguity flags.",
                        json!({"type":"object","additionalProperties":false}),
                    ),
                    "outstandings" => (
                        "Return paired native receivable/payable totals, ageing, top-party ranking, and paginated open bills. `top` applies only to party ranking; use offset and limit for bills.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"direction":{"type":"string","enum":["receivable","payable","both"],"default":"both"},"as_of":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ageing_basis":{"type":"string","enum":["bill_date","due_date"],"default":"due_date"},"top":{"type":"integer","minimum":1,"default":25},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "ledger_masters" => (
                        "Return verified ledger masters; compliance includes paired party-master observations.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"group":{"type":"string"},"fields":{"type":"string","enum":["basic","compliance"],"default":"basic"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "ledger_movement" => (
                        "Return literal-window ledger opening, exact debit/credit movement, closing, and touched-voucher count.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "vouchers" => (
                        "Return bounded, literal-window voucher evidence with curated metadata and redaction applied.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"voucher_type":{"type":"string"},"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "changed_since" => (
                        "Return snapshot-pinned AlterID voucher and master evidence. Continue a truncated scan with both returned AlterID cursors and snapshot values; deletion detection remains unsupported.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"voucher_alter_id":{"type":"integer","minimum":0,"default":0},"master_alter_id":{"type":"integer","minimum":0,"default":0},"voucher_snapshot_alter_id":{"type":"integer","minimum":0},"master_snapshot_alter_id":{"type":"integer","minimum":0}}}),
                    ),
                    "read_evidence" | "egress_log" => (
                        "Return bounded local metadata-only read evidence or egress receipts.",
                        json!({"type":"object","additionalProperties":false,"properties":{"limit":{"type":"integer","minimum":1,"default":20}}}),
                    ),
                    _ => (
                        "Bridge read-only Tally tool",
                        json!({"type":"object", "additionalProperties": false}),
                    ),
                };
                json!({"name": name, "description": description, "inputSchema": input_schema})
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "agent_admission_tests.rs"]
mod tests;
