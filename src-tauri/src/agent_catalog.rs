//! Public tool catalog and argument admission before any Tally read.
use super::*;

const NONBLANK_PATTERN: &str = r"\S";
const DATE_WIRE_PATTERN: &str = "^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$";
const BRIDGE_TRANSACTION_ID_PATTERN: &str = "^[A-Za-z0-9_-]+$";
/// A Bridge batch identity, as `amends_batch_id` publishes it. Until this was
/// in the vocabulary below, every `tools/call` naming `amends_batch_id` was
/// refused `argument_invalid:amends_batch_id` before the handler ran: the
/// amendment path was reachable only by calling the handler directly.
pub(super) const BRIDGE_BATCH_ID_PATTERN: &str =
    "^bridge-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$";

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

/// Validates a value against a published schema fragment, recursively.
///
/// [`validate_tool_arguments`] deliberately stops at the outer selectors,
/// because every tool that predates nested inputs owns its own typed boundary
/// below that line and tightening the shared path would change their refusal
/// codes. A tool whose `inputSchema` *does* describe nested objects calls this
/// instead of restating those bounds in its parser: two copies of one bound
/// drift, and the copy that drifts is the one nobody is looking at.
///
/// It enforces exactly what the fragment states — `type`, `enum`, string
/// bounds and patterns, array bounds, `required`, and `additionalProperties:
/// false` — and nothing it does not, so a schema remains the single
/// description of what a caller may send.
pub(super) fn validate_against_schema(
    value: &Value,
    schema: &Value,
    key: &str,
) -> Result<(), String> {
    let invalid = || format!("argument_invalid:{key}");
    if schema["enum"]
        .as_array()
        .is_some_and(|allowed| !allowed.contains(value))
    {
        return Err(invalid());
    }
    match schema["type"].as_str() {
        Some("string") => {
            let text = value.as_str().ok_or_else(invalid)?;
            validate_string_bounds(text, schema, key)?;
        }
        Some("integer") => {
            let number = value.as_u64().ok_or_else(invalid)?;
            if schema["minimum"].as_u64().is_some_and(|min| number < min) {
                return Err(invalid());
            }
        }
        Some("array") => {
            let items = value.as_array().ok_or_else(invalid)?;
            if schema["minItems"]
                .as_u64()
                .is_some_and(|min| items.len() < min as usize)
                || schema["maxItems"]
                    .as_u64()
                    .is_some_and(|max| items.len() > max as usize)
            {
                return Err(invalid());
            }
            for item in items {
                validate_against_schema(item, &schema["items"], key)?;
            }
        }
        Some("object") => {
            let object = value.as_object().ok_or_else(invalid)?;
            let properties = schema["properties"].as_object();
            if schema["additionalProperties"] == Value::Bool(false)
                && object
                    .keys()
                    .any(|name| !properties.is_some_and(|properties| properties.contains_key(name)))
            {
                return Err(invalid());
            }
            for required in schema["required"].as_array().into_iter().flatten() {
                let name = required.as_str().ok_or_else(invalid)?;
                if !object.contains_key(name) {
                    return Err(invalid());
                }
            }
            for (name, member) in object {
                if let Some(fragment) = properties.and_then(|properties| properties.get(name)) {
                    validate_against_schema(member, fragment, key)?;
                }
            }
        }
        _ => {}
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
        || schema["pattern"]
            .as_str()
            .is_some_and(|pattern| !published_pattern_matches(pattern, text))
    {
        return Err(format!("argument_invalid:{key}"));
    }
    Ok(())
}

/// `[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}`, exactly.
pub(super) fn is_uuid_v4_lowercase(text: &str) -> bool {
    let bytes = text.as_bytes();
    let hex = |byte: &u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte);
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            14 => *byte == b'4',
            19 => matches!(byte, b'8' | b'9' | b'a' | b'b'),
            _ => hex(byte),
        })
}

/// Recognize the finite pattern vocabulary in the published local-tool schema.
///
/// Pattern text is schema authority, but accepting an arbitrary new expression
/// would add an unbounded compile/cache decision to the admission path. Unknown
/// patterns therefore refuse input until their exact wire shape is implemented
/// and reviewed here. Calendar validity stays with `normalized_date` at the
/// typed boundary; this only preserves the published lexical shape.
fn published_pattern_matches(pattern: &str, text: &str) -> bool {
    published_pattern_matcher(pattern).is_some_and(|matches| matches(text))
}

/// The matcher for one recognized pattern, or `None` for a pattern admission
/// does not implement, which refuses every value published under it. That is
/// how `amends_batch_id` came to refuse every value; a test walks every
/// published pattern through this lookup so another cannot.
fn published_pattern_matcher(pattern: &str) -> Option<fn(&str) -> bool> {
    match pattern {
        NONBLANK_PATTERN => Some(|text| text.chars().any(|character| !character.is_whitespace())),
        DATE_WIRE_PATTERN => Some(date_wire_matches),
        // Admission applies the build's own rule, which the published pattern restates.
        BRIDGE_BATCH_ID_PATTERN => Some(agent_import::valid_batch_id),
        BRIDGE_TRANSACTION_ID_PATTERN => Some(|text| {
            !text.is_empty()
                && text
                    .as_bytes()
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }),
        _ => None,
    }
}

fn date_wire_matches(text: &str) -> bool {
    let bytes = text.as_bytes();
    let Some((year, remainder)) = bytes.split_at_checked(4) else {
        return false;
    };
    if !year.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let remainder = remainder.strip_prefix(b"-").unwrap_or(remainder);
    let Some((month, remainder)) = remainder.split_at_checked(2) else {
        return false;
    };
    if !month.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let remainder = remainder.strip_prefix(b"-").unwrap_or(remainder);
    remainder.len() == 2 && remainder.iter().all(u8::is_ascii_digit)
}

/// One proposed voucher's admission contract, lifted out of the tool literal.
///
/// Nesting it inline exhausted `json!`'s recursion budget; naming it also puts
/// the shape a caller must satisfy in one readable place. Every bound is
/// stated here once and read back by the parser rather than restated there.
fn proposed_voucher_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["date","voucher_type","entries"],"properties":{
        "date":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
        "voucher_type":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
        "voucher_number":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
        "party":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
        // Supplied together or not at all. Presence derives the narration
        // marker from these with the same function the writer used; it never
        // accepts a marker the caller chose. See ADR 0018 §1.
        "batch_id":{"type":"string","minLength":1,"maxLength":64,"pattern":r"\S"},
        "bridge_txn_id":{"type":"string","minLength":1,"maxLength":64,"pattern":"^[A-Za-z0-9_-]+$"},
        "entries":{"type":"array","minItems":1,"maxItems":presence::MAX_PRESENCE_ENTRIES,"items":{"type":"object","additionalProperties":false,"required":["ledger","amount"],"properties":{"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"amount":{"type":"string","minLength":1,"maxLength":64,"pattern":r"\S"}}}}
    }})
}

pub(super) fn tool_definitions(import_enabled: bool, writes_enabled: bool) -> Value {
    let mut definitions = registered_tool_definitions(import_enabled, writes_enabled);
    definitions
        .as_array_mut()
        .expect("tools")
        .retain(|tool| tool["name"] != "changed_since");
    definitions
}

#[cfg(feature = "lab-writes")]
fn lab_tools_env_enabled() -> bool {
    super::lab::env_lab_writes_enabled()
}
#[cfg(not(feature = "lab-writes"))]
fn lab_tools_env_enabled() -> bool {
    false
}

// Retain the internal schema while bounded change enumeration is unqualified.
pub(super) fn registered_tool_definitions(import_enabled: bool, writes_enabled: bool) -> Value {
    #[allow(unused_mut)] // only mutated when the `lab-writes` feature is compiled in
    let mut names = vec![
        "tally_status",
        "list_companies",
        "voucher_schema",
        "validate_masters",
        "build_import_xml",
        "parse_bank_statement",
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
    #[cfg(feature = "lab-writes")]
    names.push("lab_read_inventory");
    #[cfg(feature = "lab-writes")]
    names.push("lab_import_masters");
    #[cfg(feature = "lab-writes")]
    names.push("lab_import_vouchers");
    Value::Array(
        names
            .into_iter()
            // Verification is a read-only recovery capability. Keep it
            // available when Journal generation/posting is disabled so an
            // uncertain saved batch can still be checked safely.
            // Parsing a statement only prepares an import, and its summary
            // carries counterparty names, so it is opted into with imports.
            .filter(|name| {
                import_enabled || !matches!(*name, "build_import_xml" | "parse_bank_statement")
            })
            .filter(|name| writes_enabled || *name != "post_import")
            // LAB-ONLY: registered only when the `lab-writes` feature is
            // compiled in AND `BRIDGE_LAB_WRITES=1` is set (checked fresh on
            // every catalog build, not cached at startup).
            .filter(|name| {
                !matches!(
                    name,
                    &"lab_read_inventory" | &"lab_import_masters" | &"lab_import_vouchers"
                ) || lab_tools_env_enabled()
            })
            .map(|name| {
                let (description, input_schema) = match name {
                    "voucher_schema" => (
                        "Return the fail-closed local voucher-file schema; no Tally request is sent.",
                        json!({"type":"object", "additionalProperties":false}),
                    ),
                    "validate_masters" => (
                        "Bind 1–100 nonblank ledger names (at most 1024 characters each) against the live catalogue. An identifier embedded in a master name is matched before the name itself. `match_state` is exact, identifier, near_miss or missing; folded names remain near_miss candidates because this catalogue has no qualified scope to bind them. Only `exact` is admitted by build_import_xml, and a bound row alone carries `exact_live_spelling`. A near-miss is never resolved: it returns candidates with the rule that surfaced each, bounded to 25 names and 8192 UTF-8 bytes per requested name, with candidate_count, candidate_count_is_lower_bound and truncation reported. When candidate_count_is_lower_bound is true, the count is a conservative lower bound and must be shown as at least that many candidates. There is no ranking and no score.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","ledgers"], "properties":{"company_guid":{"type":"string"},"ledgers":{"type":"array","minItems":1,"maxItems":agent_import::MAX_MASTER_NAMES,"items":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"}}}}),
                    ),
                    "build_import_xml" => (
                        "Validate a Journal, Payment, Receipt or Contra batch and read its current verification window before writing a local import file. Vouchers are given inline, or as a parse_bank_statement proposals file named by proposals_id with the proposals_sha256 that tool returned; a file changed since is refused, and its vouchers are admitted exactly as inline ones. A Payment credits and a Receipt debits a cash/bank ledger against a counterparty established as holding no money, a Contra moves between two of them, and each takes two or more entries (at least one debit and one credit, no ledger on both sides, every leg classified; for more than two, one Bridge-built three-entry Receipt has been imported over the gateway and verified, but no multi-entry Payment or Contra has been, and none of the three, including that Receipt, through Tally's Import menu) with no voucher number or reference; a Journal is unconstrained. Every build creates a new batch identity, even for reused transaction labels, except an amendment: naming amends_batch_id re-renders vouchers of a batch Bridge built under that batch's identity, so importing the file alters them in place, and it is refused unless each is still in the book as Bridge built it in the fields compared (date, a bank voucher's effective date when Tally returns one, type, number when the batch set one, each entry's ledger, amount and side, narration) and none was posted natively. A reference, bill-wise or cost-centre allocations and the party ledger are not compared field by field, and allocations made in Tally, including those Bridge advises after an import, are expected to be lost (not measured directly); instead each voucher's ALTERID must equal the one Bridge recorded when it first verified a build the book matches, so an amendment needs that verification and refuses a voucher altered since (voucher_altered_since_verified, voucher_never_verified). Verifying now records this voucher exactly as it stands in Tally, including any changes made since Bridge built it. Check the voucher in Tally first; if someone has edited it, correct it there instead of amending. The check runs when the amendment is built, not when it is imported: the import is done by hand in Tally, and an edit made there in between is overwritten without warning, so import promptly. Import and verify each amendment before building the next one for the same voucher: two amendments built from the same state overwrite each other, and the later import wins. If any import outcome is uncertain, preserve the original batch and saved file, then reconcile with verify_import without writing; an amendment is not recovery, and is refused for a voucher not found in the book. Otherwise do not re-import or rebuild the same business event, including a Journal; a repeat observation does not qualify recovery after an unknown outcome. Later changes may exceed read limits. Other voucher types are unqualified. This never dispatches import XML to Tally.",
                        agent_import::voucher_input_schema(),
                    ),
                    "parse_bank_statement" => (
                        bank_statement::DESCRIPTION,
                        bank_statement::input_schema(),
                    ),
                    "post_import" => (
                        "Ask the local user to review and approve ONE saved Journal, Payment, Receipt or Contra in a native dialog, then attempt posting once and read it back. Requires opt-in. A Payment, Receipt or Contra is refused (import_bank_classification_changed) if any leg's cash/bank classification changed since the build, checked before approval and again after approval inside the endpoint queue, before the final duplicate check and the post. Every post is aimed by a last all-company snapshot, refused as post_company_scope_changed (or post_company_scope_unconfirmed if unreadable) unless exactly one loaded company has the target's GUID and name and no other loaded company's name could match it; the result's post_location says which companies' voucher marks moved after it, and masters_after_post whether the approved ledgers still resolve to the same masters after the post: when one no longer resolves to its approved GUID (posted_under_changed_masters), the voucher is in Tally but the result is reconciliation_required, never posted_verified, on this and every later verify_import; ask the user to review the voucher in Tally, and do not rebuild the event. When that could not be confirmed (masters_after_post_unconfirmed), it is the same until a later verify_import that finds the voucher completes the check. A post whose check cannot be recorded is refused before it is sent (post_masters_record_unavailable). A company with more than one currency defined is refused (import_multi_currency_unsupported), before approval and again in the queue: Bridge does not post into multi-currency books yet (import_base_currency_undetermined if no usable currency master is read). A change to the company's masters from just before the queue's catalogue re-read to the aim snapshot refuses as post_masters_moved (or post_masters_unconfirmed), when it moves the company's master AlterID (ALTMSTID): measured for ledger renames and creates made through the gateway; a regroup, an edit in Tally's own screens, and whether posting a voucher moves it, are not yet measured. Re-run after a refusal. A ledger now on another GUID than at build (renamed and replaced, or deleted and recreated) refuses as import_masters_changed_since_build, naming it: the name now means a different ledger, so confirm the intended one with validate_masters before building again. A batch built before Bridge recorded ledger identities refuses as import_batch_predates_ledger_binding; build it again. Rebuild only when attempt_recorded is false. Repeating the original batch only reconciles; never rebuild the same event after a timeout. The model cannot approve it. No master creation, sales, purchase, tax, inventory, alteration or deletion.",
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
                        "Return verified ledger masters with monetary fields on a freshly observed supported product/mode; compliance includes paired party-master observations. With fields=compliance, each ledger also carries `ancestry`: its resolved group chain (`chain`, nearest ancestor first, each hop's own `name` and `reserved_name`), `complete` (true only if the chain was resolved all the way to the reserved account root), and `gap` (null when complete, else why resolution stopped: no_parent, group_absent, group_name_repeated, reserved_name_missing, cycle or exhausted). A `reserved_name` beginning with U+FFFD `#4;` is a Tally reserved value (Tally writes it as `&#4;`); `U+FFFD#4; Primary` is the account root, distinct from a group a user named Primary. An incomplete chain is never padded or guessed -- `chain` is exactly what was resolved and no more, and a caller must check `complete` before treating it as exhaustive; this is not available with fields=basic, which never reads the group collection. `group` filters by group name; `group_scope` controls what it matches against -- \"immediate\" (default, unchanged) matches only the ledger's own parent, \"ancestry\" also matches any group in its resolved chain (e.g. a ledger under `Bank OD A/c` matches a `group_scope: ancestry` filter for `Loans (Liability)`) and requires fields=compliance. A gap in a ledger's chain never counts as an ancestry-scope match.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"group":{"type":"string"},"group_scope":{"type":"string","enum":["immediate","ancestry"],"default":"immediate"},"fields":{"type":"string","enum":["basic","compliance"],"default":"basic"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
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
                        "Return literal-window voucher evidence with curated metadata and redaction. Reads the full source window before selectors and output pagination; limit does not reduce Tally work. Use narrow dates; dense windows are unqualified and can fail source limits. Each item carries `cancelled` and `optional` (always booleans; a source that omits or cannot assert either fails the whole read rather than guess). `post_dated` behaves differently: a real capture has shown Tally omitting that tag entirely rather than asserting `No`, so it is boolean only when Tally asserted Yes/No, and the key is absent from the item when Tally did not report it. Absent is not evidence of `false` \u{2014} it means \u{2018}Tally did not say\u{2019}, not \u{2018}Tally said no\u{2019}, and a caller must branch on key presence, not on falsiness, before treating a voucher as not post-dated. Neither this tool nor `voucher_presence` filters out post-dated (or optional/cancelled) vouchers; the caller decides what a non-posting or unobserved status means for its own computation. `reference`, `is_invoice` and `party_gstin` follow the same absent-means-not-observed convention as `post_dated`: each key is present only when Tally reported a non-empty value for it, and its absence must not be read as false or as an empty string. `is_invoice` is a boolean exactly like `post_dated`; `reference` and `party_gstin` are non-empty strings when present. A captured book with no GSTIN recorded against a party's ledger has shown `party_gstin` absent on every voucher for that party, which is not evidence Tally cannot report one.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"voucher_type":{"type":"string","maxLength":agent_import::MAX_MASTER_NAME_CHARS},"ledger":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "voucher_presence" => (
                        "Answer which of 1\u{2013}500 proposed vouchers are already in the book. `presence` is present, possibly_present or absent, and only `present` names a book voucher. The adapter has no source-completeness evidence for a nonempty window, so a nonempty window is read as `partial`; an empty window can still be corroborated complete. `present` and `possibly_present` never need a complete window and are produced either way, but `absent` means absent from the *whole* window and is only ever produced from one proven complete — a proposal that would otherwise be absent from a merely `partial` window instead comes back `possibly_present` with reason `window_not_proven_complete`. The conditional decision basis can use a voucher number on a voucher type you declare `manual` \u{2014} unique on both sides, within an observed voucher type, and never onto a cancelled or optional voucher; or, for a voucher Bridge wrote, the narration marker derived from the supplied `batch_id` and `bridge_txn_id` together. It neither accepts nor reads client remote identifiers. Supplying only one narration identity component is an error. The marker reaches only the current writer identity scheme; older-scheme Bridge writes stay unidentified rather than matched. Date, party and amount only ever produce candidates, with the rule that surfaced each and no ranking or score. Every voucher type a proposal names needs a declared numbering method; under `automatic` Tally discards the supplied number, so nothing can be decided from it. `absent` means absent from this window, so cover the dates the book could hold. Reads the full window before comparing; dense windows can fail source limits. Party names bind through the same rules as validate_masters. A reported difference on a `present` voucher is a finding for a person, not a work item: correcting a voucher by Alter or Cancel silently creates a duplicate instead (\u{00a7}9.7), and no Bridge path can correct a voucher it did not write. This never dispatches import XML to Tally.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to","numbering","vouchers"],"properties":{
                            "company_guid":{"type":"string","minLength":1},
                            "offset":{"type":"integer","minimum":0,"default":0},
                            "limit":{"type":"integer","minimum":1,"default":500},
                            "from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
                            "to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
                            "numbering":{"type":"array","minItems":1,"maxItems":presence::MAX_PRESENCE_VOUCHER_TYPES,"items":{"type":"object","additionalProperties":false,"required":["voucher_type","numbering_method"],"properties":{"voucher_type":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},"numbering_method":{"type":"string","enum":["manual","automatic","unknown"]}}}},
                            "vouchers":{"type":"array","minItems":1,"maxItems":presence::MAX_PRESENCE_VOUCHERS,"items":proposed_voucher_schema()}
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
                    "lab_read_inventory" => (
                        "LAB-ONLY. Compiled only behind the `lab-writes` feature and refuses unless BRIDGE_LAB_WRITES=1, BRIDGE_TALLY_PORT=9001, and BRIDGE_LAB_TARGET_GUID/BRIDGE_LAB_DENY_GUIDS are both set to well-formed GUIDs. This is a read: company_guid selects the company like any other read tool and is verified the same way (`company_identity_not_found`/`company_identity_ambiguous`), independent of the configured lab target -- the stronger loaded-company/deny-list guard applies only to a lab write batch, not a read. Read-only: units, godowns, stock groups and stock items (parent, base unit, opening qty/rate/value, GST/HSN fields as returned, unclassified), plus inventory entries per voucher for a date window. Reuses the same windowing and window_honoured corroboration as `vouchers`. No signed compatibility evidence exists yet for any inventory field on this Tally release/mode -- treat every value as exploratory.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "lab_import_masters" => (
                        "LAB-ONLY (Phase 3.4). Compiled only behind `lab-writes`; refuses unless BRIDGE_LAB_WRITES=1, BRIDGE_TALLY_PORT=9001, and BRIDGE_LAB_TARGET_GUID/BRIDGE_LAB_DENY_GUIDS are set. Creates masters (units, godowns, stock groups, groups, ledgers, stock items, in that order) from the book model's `masters` section (inline `masters` or a `book_path` local JSON file). Re-verifies the loaded-company/deny-list/target-identity guard before every batch (<=200 masters). Refuses before any write if the target already carries a same-name master under any requested kind (the Create-overwrite trap, §9.4) -- `lab_master_already_exists`. Every batch is read back field-by-field (name, parent, opening balance/qty, GST fields) and the whole call stops on the first mismatch; never trusts CREATED/ERRORS alone. Group/Unit/Godown/StockGroup/StockItem XML shapes have no live capture in this repository and are UNVERIFIED for the gateway -- see the tool's module documentation.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"masters":{"type":"object"},"book_path":{"type":"string","minLength":1}}}),
                    ),
                    "lab_import_vouchers" => (
                        "LAB-ONLY (Phase 3.5). Compiled only behind `lab-writes`; refuses unless BRIDGE_LAB_WRITES=1, BRIDGE_TALLY_PORT=9001, and BRIDGE_LAB_TARGET_GUID/BRIDGE_LAB_DENY_GUIDS are set. Creates vouchers (Journal/Payment/Receipt/Contra plus accounting- and invoice-mode Sales/Purchase/Credit Note/Debit Note) from the book model's `vouchers` section (inline `vouchers` or a `book_path` local JSON file), sorted by date and posted in batches of at most 100. Re-verifies the loaded-company/deny-list/target-identity guard before every batch. Before sending a batch, reads its date window back and checks every voucher against a narration-marker/voucher-number plus type/date/ledger-amount fingerprint: a fully-matched batch is skipped (resume), a partially-matched batch stops with `lab_batch_partially_verified_uncertain` rather than guessing, and only an unmatched batch is sent. Every sent batch is read back the same way and the whole call stops on the first mismatch. Invoice-mode XML (`LEDGERENTRIES.LIST`/`ALLINVENTORYENTRIES.LIST`) and every type but Sales/Journal/Payment/Receipt/Contra are UNVERIFIED for the gateway -- see the tool's module documentation. `start_batch` resumes a prior call.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"vouchers":{"type":"array"},"book_path":{"type":"string","minLength":1},"start_batch":{"type":"integer","minimum":0,"default":0}}}),
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
                if name == "parse_bank_statement" {
                    tool["annotations"] = json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":false,"openWorldHint":false});
                }
                if name == "lab_read_inventory" {
                    tool["annotations"] = json!({"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":true});
                }
                if matches!(name, "lab_import_masters" | "lab_import_vouchers") {
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
