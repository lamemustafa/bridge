//! Public tool catalog and argument admission before any Tally read.
use super::*;

pub(super) fn validate_tool_arguments(name: &str, args: &Value) -> Result<(), String> {
    let arguments = args
        .as_object()
        .ok_or_else(|| "argument_schema_invalid".to_string())?;
    let definitions = registered_tool_definitions(true, true);
    let schema = &definitions
        .as_array()
        .expect("tool definitions")
        .iter()
        .find(|tool| tool["name"] == name)
        .ok_or_else(|| "tool_not_found".to_string())?["inputSchema"];
    let properties = schema["properties"].as_object();
    for key in arguments.keys() {
        if !properties.is_some_and(|properties| properties.contains_key(key)) {
            return Err("argument_unknown".to_string());
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
                if key == "company_guid" {
                    parse_native_company_guid(text)?;
                }
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

pub(super) fn tool_definitions(import_enabled: bool, writes_enabled: bool) -> Value {
    let mut definitions = registered_tool_definitions(import_enabled, writes_enabled);
    definitions
        .as_array_mut()
        .expect("tools")
        .retain(|tool| tool["name"] != "changed_since");
    definitions
}

// Retain the internal schema while bounded change enumeration is unqualified.
pub(super) fn registered_tool_definitions(import_enabled: bool, writes_enabled: bool) -> Value {
    let names = [
        "tally_status",
        "list_companies",
        "voucher_schema",
        "validate_masters",
        "build_import_xml",
        "verify_import",
        "post_import",
        "outstandings",
        "ledger_masters",
        "ledger_movement",
        "trial_balance",
        "vouchers",
        "voucher_presence",
        "changed_since",
        "read_evidence",
        "egress_log",
    ];
    Value::Array(
        names
            .into_iter()
            // Verification is a read-only recovery capability. Keep it
            // available when Journal generation/posting is disabled so an
            // uncertain saved batch can still be checked safely.
            .filter(|name| import_enabled || *name != "build_import_xml")
            .filter(|name| writes_enabled || *name != "post_import")
            .map(|name| {
                let (description, input_schema) = match name {
                    "voucher_schema" => (
                        "Return the fail-closed local voucher-file schema; no Tally request is sent.",
                        json!({"type":"object", "additionalProperties":false}),
                    ),
                    "validate_masters" => (
                        "Bind 1–100 nonblank ledger names (at most 1024 characters each) against the live catalogue. An identifier embedded in a master name is matched before the name itself. `match_state` is exact, normalized, identifier, near_miss or missing; only `exact` is admitted by build_import_xml, and a bound row alone carries `exact_live_spelling`. A near-miss is never resolved: it returns candidates with the rule that surfaced each, bounded to 25 names and 8192 UTF-8 bytes per requested name, with total count and truncation reported. There is no ranking and no score.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","ledgers"], "properties":{"company_guid":{"type":"string"},"ledgers":{"type":"array","minItems":1,"maxItems":agent_import::MAX_MASTER_NAMES,"items":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"}}}}),
                    ),
                    "build_import_xml" => (
                        "Validate a Journal batch and read its current verification window before writing a local import file. Every build creates a new batch identity, even for reused transaction labels; retry the saved file instead of rebuilding the same business event. Later changes may exceed read limits. Other voucher types are unqualified. This never dispatches import XML to Tally.",
                        agent_import::voucher_input_schema(),
                    ),
                    "post_import" => (
                        "Ask the local user to review and approve ONE saved Journal in a native dialog, then attempt posting once and read it back. Requires opt-in. Repeating the original batch only reconciles; never rebuild the same event after a timeout. The model cannot approve it. No master creation, sales, purchase, tax, inventory, alteration or deletion.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","batch_id"], "properties":{"company_guid":{"type":"string"},"batch_id":{"type":"string","minLength":43,"maxLength":43}}}),
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
                        "Return paired native receivable/payable totals, ageing, top-party ranking, and paginated open bills with a freshly observed supported product/mode and an operation-valid date boundary. `top` applies only to party ranking; use offset and limit for bills.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"direction":{"type":"string","enum":["receivable","payable","both"],"default":"both"},"as_of":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ageing_basis":{"type":"string","enum":["bill_date","due_date"],"default":"due_date"},"top":{"type":"integer","minimum":1,"default":25},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "ledger_masters" => (
                        "Return verified ledger masters with monetary fields on a freshly observed supported product/mode; compliance includes paired party-master observations.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"group":{"type":"string"},"fields":{"type":"string","enum":["basic","compliance"],"default":"basic"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "ledger_movement" => (
                        "Return literal-window ledger opening, exact debit/credit movement, closing, and touched-voucher count with a freshly observed supported product/mode and an operation-valid opening boundary. Reads the full voucher window before filtering or pagination; use narrow dates. Dense windows can fail source limits.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "trial_balance" => (
                        "Return native ledger-wise Trial Balance for a date range, using Tally's TBAL fields without scanning vouchers. Requires observed INR currency and supported date boundaries. Preserves empty amounts; paired source stability is not voucher-level reconciliation. Pagination limits output only; each call captures a fresh report.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "vouchers" => (
                        "Return literal-window voucher evidence with curated metadata and redaction. Reads the full source window before selectors and output pagination; limit does not reduce Tally work. Use narrow dates; dense windows are unqualified and can fail source limits.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"voucher_type":{"type":"string","maxLength":agent_import::MAX_MASTER_NAME_CHARS},"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "voucher_presence" => (
                        "Answer which of 1\u{2013}500 proposed vouchers are already in the book, over one literal window. Tally dedupes on one key only: re-sending a voucher under the same VOUCHERNUMBER creates a second one, while a client-supplied REMOTEID upserts instead. A voucher keyed by hand carries no client REMOTEID, so it is the one at duplication risk. `presence` is present, possibly_present or absent, and only `present` names a book voucher. Only identity decides: a shared REMOTEID, or a voucher number on a voucher type you declare `manual` \u{2014} unique on both sides, within an observed voucher type, and never onto a cancelled or optional voucher. Date, party and amount only ever produce candidates, with the rule that surfaced each and no ranking or score. Every voucher type a proposal names needs a declared numbering method; under `automatic` Tally discards the supplied number, so nothing can be decided from it. `absent` means absent from this window, so cover the dates the book could hold. Reads the full window before comparing; dense windows can fail source limits. Party names bind through the same rules as validate_masters. A reported difference on a `present` voucher is a finding for a person, not a work item: correcting a voucher by Alter or Cancel silently creates a duplicate instead (\u{00a7}9.7), and no Bridge path can correct a voucher it did not write. This never dispatches import XML to Tally.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to","numbering","vouchers"],"properties":{
                            "company_guid":{"type":"string","minLength":1},
                            "from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
                            "to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
                            "numbering":{"type":"array","minItems":1,"maxItems":presence::MAX_PRESENCE_VOUCHER_TYPES,"items":{"type":"object","additionalProperties":false,"required":["voucher_type","numbering_method"],"properties":{"voucher_type":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"numbering_method":{"type":"string","enum":["manual","automatic","unknown"]}}}},
                            "vouchers":{"type":"array","minItems":1,"maxItems":presence::MAX_PRESENCE_VOUCHERS,"items":{"type":"object","additionalProperties":false,"required":["date","voucher_type","entries"],"properties":{
                                "date":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
                                "voucher_type":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
                                "voucher_number":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
                                "remote_id":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
                                "party":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
                                "entries":{"type":"array","minItems":1,"maxItems":presence::MAX_PRESENCE_ENTRIES,"items":{"type":"object","additionalProperties":false,"required":["ledger","amount"],"properties":{"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"amount":{"type":"string","minLength":1,"maxLength":64,"pattern":r"\S"}}}}
                            }}}
                        }}),
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
                let mut tool = json!({"name": name, "description": description, "inputSchema": input_schema});
                if name == "post_import" {
                    tool["annotations"] = json!({"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":true});
                }
                tool
            })
            .collect(),
    )
}

#[cfg(test)]
#[path = "agent_admission_tests.rs"]
mod tests;
