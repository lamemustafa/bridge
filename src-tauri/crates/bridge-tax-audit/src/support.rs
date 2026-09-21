// SPDX-License-Identifier: Apache-2.0
//! Helpers shared by the test modules ported from batch 1 onward: the reference's voucher
//! evidence label, the tag -> ledger lookup the module invariants resolve figures through, and
//! checked counts and sums.

use std::collections::HashMap;

use crate::book::{Book, Voucher};
use crate::error::{AuditError, Result};
use crate::findings::Value;
use crate::ledger_ids::stable_ledger_tag;
use crate::read::iso;

/// The last 12 characters of a GUID, as the reference's `guid[-12:]` takes them: characters, not
/// bytes, so a non-ASCII GUID neither panics nor gives a different label.
pub(crate) fn guid_tail12(guid: &str) -> &str {
    let chars = guid.chars().count();
    let cut = guid
        .char_indices()
        .nth(chars.saturating_sub(12))
        .map_or(guid.len(), |(i, _)| i);
    &guid[cut..]
}

/// The reference's voucher evidence label: `"<type> <number> on <date>"`, with the GUID's last 12
/// characters standing in for a missing number.
pub(crate) fn voucher_label(v: &Voucher) -> String {
    let num = if v.number.is_empty() {
        guid_tail12(&v.guid)
    } else {
        v.number.as_str()
    };
    format!("{} {} on {}", v.vtype, num, iso(&v.date))
}

/// Every ledger by its stable tag, for resolving a `<figure>_<tag>` id back to its ledger. If two
/// ledgers ever shared a tag, this keeps the last in name order, where the reference's dict keeps
/// the last in read order -- not reachable while tags are unique.
pub(crate) fn ledgers_by_tag(book: &Book) -> Result<HashMap<String, &String>> {
    let mut out = HashMap::new();
    for name in book.ledgers.keys() {
        out.insert(stable_ledger_tag(book, name)?, name);
    }
    Ok(out)
}

/// The error every checked total in a test module raises on i64 overflow.
pub(crate) fn overflow(test_id: &str) -> AuditError {
    AuditError::Config(format!("{test_id}: a total overflowed i64 paise"))
}

/// A count as a figure value.
pub(crate) fn count(test_id: &str, n: usize) -> Result<Value> {
    Ok(Value::Int(i64::try_from(n).map_err(|_| overflow(test_id))?))
}

#[cfg(test)]
mod tests {
    use super::guid_tail12;

    #[test]
    fn the_guid_tail_counts_characters_not_bytes() {
        assert_eq!(guid_tail12("abc"), "abc");
        assert_eq!(guid_tail12("0123456789abcdef"), "456789abcdef");
        // Python: "é".join(...)[-12:] -- 12 characters, 13 bytes here.
        assert_eq!(guid_tail12("xxé0123456789a"), "é0123456789a");
        assert_eq!(guid_tail12("ééééééééééééé"), "éééééééééééé");
        assert_eq!(guid_tail12(""), "");
    }
}
