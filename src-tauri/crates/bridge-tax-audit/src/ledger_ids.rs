//! Stable tags for figure/finding/evidence ids, keyed by Tally identity (GUID) rather than by
//! ledger display name. A byte-for-byte port of the reference Python implementation's
//! `tae/ledger_ids.py`: same hash (sha1, first 8 hex characters of the raw UTF-8 bytes), same
//! fallback (hash the name itself when it does not name a real `Book` ledger).
//!
//! Why: a ledger's display NAME is not stable across two reads of the same unchanged master (a
//! read-shape/serialisation artifact -- e.g. `ROUND OFF` -> `Round Off`, `NSC` -> `N.S.C.` --
//! while the master's own ALTERID is unchanged, proving no one edited it). A figure id built as
//! `hash(name)` therefore churns for no reason connected to the books; `Ledger::guid` is Tally's
//! own identity and does not move on a rename.
//!
//! `cash_44ab`, `cash_payments_40a3` and `depreciation` are ported 1:1 from the reference engine
//! and compared byte-for-byte against it (tests/fixtures/golden); `cash_payments_40a3` and
//! `depreciation` call [`stable_ledger_tag`] exactly where the reference engine's own modules
//! call `tae.ledger_ids.stable_ledger_tag`. Any change to the hash or the fallback here must be
//! mirrored in `tae/ledger_ids.py` in the same change, or parity breaks silently.

use sha1::{Digest, Sha1};

use crate::book::{Book, Ledger};
use crate::error::{AuditError, Result};

fn missing_guid(what: &str) -> AuditError {
    AuditError::Config(format!(
        "{what} has no Tally GUID; refusing to derive a stable id from its name"
    ))
}

/// Short, stable, non-reversible-in-practice tag for a figure/finding/evidence id, from a Tally
/// GUID: the first 8 hex characters of the sha1 of the GUID's raw UTF-8 bytes. Errs when `guid`
/// is blank -- refuse, never fall back to a name hash (mirrors the reference implementation's
/// `MissingGuid`).
pub fn guid_tag(guid: &str, what: &str) -> Result<String> {
    if guid.is_empty() {
        return Err(missing_guid(what));
    }
    Ok(crate::canonical::hex(&Sha1::digest(guid.as_bytes()))[..8].to_string())
}

/// See [`guid_tag`].
pub fn ledger_tag(ledger: &Ledger) -> Result<String> {
    guid_tag(&ledger.guid, &format!("ledger {:?}", ledger.name))
}

/// A guid-based tag when `name` names a real ledger in `book.ledgers` (the case at risk of the
/// read-shape rename churn this module exists to fix); the plain sha1 hash of `name` itself
/// otherwise -- for a `name` that is NOT a Book ledger: a client-config alias, a synthetic
/// sentinel bucket (e.g. `cash_payments_40a3::UNIDENTIFIED_PARTY`), or a Trial Balance row with no
/// matching ledger master. None of those is itself read fresh from Tally's ledger export on every
/// capture, so none of them carries this module's rename-churn risk; hashing the string itself is
/// already stable for them, and refusing would drop a real row for no benefit.
pub fn stable_ledger_tag(book: &Book, name: &str) -> Result<String> {
    match book.ledgers.get(name) {
        Some(ledger) => ledger_tag(ledger),
        None => Ok(crate::canonical::hex(&Sha1::digest(name.as_bytes()))[..8].to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::Book;
    use std::collections::BTreeMap;

    // Pinned against the reference implementation's tae.ledger_ids (computed with the Python
    // function itself, 2026-09-19): guid_tag("abc") and stable_ledger_tag on a book with one
    // real ledger, "ROUND OFF"/guid "g-1", plus a name that is not a book ledger at all.
    #[test]
    fn guid_tag_matches_the_reference_implementation() {
        assert_eq!(guid_tag("abc", "x").unwrap(), "a9993e36");
        assert_eq!(guid_tag("g-1", "x").unwrap(), "6d5494d3");
        assert_eq!(guid_tag("1753bdf6-000000b1", "x").unwrap(), "f9c58209");
    }

    #[test]
    fn guid_tag_blank_refuses() {
        assert!(guid_tag("", "ledger 'X'").is_err());
    }

    fn book_with(name: &str, guid: &str) -> Book {
        let mut ledgers = BTreeMap::new();
        ledgers.insert(
            name.to_string(),
            Ledger {
                name: name.to_string(),
                parent: "Indirect Expenses".to_string(),
                chain: vec!["Indirect Expenses".to_string()],
                chain_complete: true,
                opening_paise: 0,
                guid: guid.to_string(),
                masterid: None,
            },
        );
        Book {
            company_name: "Co".to_string(),
            company_guid: "co-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers,
            vouchers: Vec::new(),
            tb: BTreeMap::new(),
        }
    }

    #[test]
    fn stable_ledger_tag_uses_guid_not_name() {
        let old = book_with("ROUND OFF", "g-1");
        let renamed = book_with("Round Off", "g-1");
        assert_eq!(
            stable_ledger_tag(&old, "ROUND OFF").unwrap(),
            stable_ledger_tag(&renamed, "Round Off").unwrap()
        );
        // Pinned value: sha1("g-1")[..8], same as guid_tag_matches_the_reference_implementation.
        assert_eq!(stable_ledger_tag(&old, "ROUND OFF").unwrap(), "6d5494d3");
    }

    #[test]
    fn stable_ledger_tag_real_ledger_with_no_guid_refuses() {
        let b = book_with("X", "");
        assert!(stable_ledger_tag(&b, "X").is_err());
    }

    #[test]
    fn stable_ledger_tag_name_not_a_book_ledger_falls_back_to_name_hash() {
        let b = book_with("Round Off", "g-4");
        // Pinned against the reference implementation's
        // hashlib.sha1(name.encode()).hexdigest()[:8].
        assert_eq!(
            stable_ledger_tag(&b, "(within supplier goods invoices)").unwrap(),
            "ba9d4928"
        );
    }
}
