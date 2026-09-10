//! Structural payload admission; live identity, masters and balance remain runtime checks.
use super::{LIVE_QUALIFIED_VOUCHER_TYPES, MAX_MASTER_NAME_CHARS, MAX_TEXT_CHARS, MAX_VOUCHERS};
use serde_json::{json, Value};

const CONTROLS: &str = r"[\u0000-\u001F\u007F-\u009F]";
// ECMAScript's `$` can match before these terminal characters. ASCII-only
// scalar patterns additionally refuse them anywhere in the value.
const LINE_TERMINATORS: &str = r"[\u000A\u000D\u2028\u2029]";
const RESERVED_MARKER: &str = r"\[[Bb][Rr][Ii][Dd][Gg][Ee]:";
// Non-control members of Unicode White_Space, matching Rust str::trim.
const BLANK_LEDGER: &str = r"^[\u0020\u00A0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]*$";

pub(in crate::agent) fn voucher_input_schema() -> Value {
    let text = json!({
        "type":"string", "minLength":1, "maxLength":MAX_TEXT_CHARS,
        "not":{"anyOf":[{"pattern":CONTROLS},{"pattern":RESERVED_MARKER}]},
        "description":"Nonempty text without control characters. The reserved [BRIDGE: marker is forbidden in every ASCII case, including XML entity-encoded spellings."
    });
    let entry = json!({
        "type":"object", "additionalProperties":false,
        "required":["ledger","amount","side"],
        "properties":{
            "ledger":{
                "type":"string", "minLength":1, "maxLength":MAX_MASTER_NAME_CHARS,
                "not":{"anyOf":[{"pattern":CONTROLS},{"pattern":BLANK_LEDGER}]}
            },
            "amount":{
                "type":"string", "pattern":r"^[0-9]+\.[0-9]{2}$",
                "maxLength":bridge_tally_core::MAX_EXACT_DECIMAL_BYTES,
                "not":{"anyOf":[{"pattern":r"^0+\.00$"},{"pattern":LINE_TERMINATORS}]}
            },
            "side":{"enum":["Dr","Cr"]}
        }
    });
    let voucher = json!({
        "type":"object", "additionalProperties":false,
        "required":["bridge_txn_id","date","voucher_type","entries"],
        "properties":{
            "bridge_txn_id":{
                "type":"string", "pattern":"^[A-Za-z0-9_-]{1,64}$",
                "not":{"pattern":LINE_TERMINATORS}
            },
            "date":{
                "type":"string", "pattern":r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$",
                "not":{"pattern":LINE_TERMINATORS}
            },
            "voucher_type":{
                "enum":LIVE_QUALIFIED_VOUCHER_TYPES,
                "description":"Journal takes any balanced set of entries and may carry voucher_number and reference. Payment, Receipt and Contra take exactly two entries over two distinct ledgers and neither of those fields, and are refused unless the money side names a ledger whose live group ancestry reaches Bank Accounts, Bank OD A/c or Cash-in-Hand: the credit on a Payment, the debit on a Receipt, both legs on a Contra. The remaining leg of a Payment or Receipt must not be one of those, because money on both sides is a Contra. These rules span fields and live masters, so the server enforces them after admission rather than here."
            },
            "narration":text, "reference":text,
            "voucher_number":{
                "type":"string", "minLength":1, "maxLength":32,
                "not":{"pattern":r"[\u0000-\u001F\u007F-\u009F$]"}
            },
            "entries":{"type":"array", "minItems":2, "items":entry}
        }
    });
    json!({
        "type":"object", "additionalProperties":false,
        "required":["company_guid","vouchers"],
        "properties":{
            "company_guid":{"type":"string","minLength":1},
            "vouchers":{
                "type":"array", "minItems":1, "maxItems":MAX_VOUCHERS,
                "description":"At most 100 distinct ledger names across the batch; repeated ledgers do not reduce the 1000-voucher limit.",
                "items":voucher
            }
        }
    })
}
