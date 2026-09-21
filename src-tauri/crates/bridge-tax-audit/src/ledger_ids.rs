//! Stable tags for figure/finding/evidence ids, keyed by Tally identity (GUID) rather than
//! ledger display name. A byte-for-byte port of the reference engine's implementation; the rule
//! is written down in full in `docs/tax-audit/parity-spec-v1.md` §11 -- this module's own
//! comments summarise it, that section is the contract.
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
//! call their equivalent. Any change to the hash, the fallback, or the duplicate-GUID refusal
//! here must be mirrored in the reference engine in the same change (and in
//! `docs/tax-audit/parity-spec-v1.md` §11), or parity breaks silently.

use std::collections::BTreeMap;

use sha1::{Digest, Sha1};

use crate::book::{Book, Ledger};
use crate::error::{AuditError, Result};

fn missing_guid(what: &str) -> AuditError {
    AuditError::MissingGuid(what.to_string())
}

/// Strip surrounding whitespace, then lower-case, exactly as the reference's `str.strip().lower()`
/// (Python's whitespace and Unicode 15.1 case mapping: `support::py_strip`, `py_lower`). A Tally
/// GUID is hex digits and hyphens, but a read carrying anything else must still tag as the
/// reference tags it. Two engines, or two Tally exports of the same GUID in different casing, must agree
/// on the same tag; the binding logic elsewhere in this stack already treats GUIDs as
/// case-insensitive, so the tag has to match that, not hash the raw bytes
/// (`docs/tax-audit/parity-spec-v1.md` §11).
fn normalize_guid(guid: &str) -> String {
    crate::support::py_lower(crate::support::py_strip(guid))
}

/// Short, stable, non-reversible-in-practice tag for a figure/finding/evidence id, from a Tally
/// GUID: the first 8 hex characters of the sha1 of the normalised (`normalize_guid`) GUID. Errs
/// when the normalised GUID is blank -- refuse, never fall back to a name hash (mirrors the
/// reference implementation's `MissingGuid`).
pub fn guid_tag(guid: &str, what: &str) -> Result<String> {
    let normalized = normalize_guid(guid);
    if normalized.is_empty() {
        return Err(missing_guid(what));
    }
    Ok(crate::canonical::hex(&Sha1::digest(normalized.as_bytes()))[..8].to_string())
}

/// See [`guid_tag`]. A ledger with a blank (post-normalisation) GUID falls back to hashing the
/// ledger's own NAME instead of refusing -- `tally-read-v1` does not require ledger GUIDs, and
/// refusing here turned a formerly working depreciation run into a hard failure on an
/// otherwise-valid read (bridge PR #511 review). The id for such a ledger is stable only WITHIN
/// one read: a later read that renames the same GUID-less ledger will still change its id,
/// because there is no Tally identity left to anchor it to.
pub fn ledger_tag(ledger: &Ledger) -> Result<String> {
    if normalize_guid(&ledger.guid).is_empty() {
        return Ok(crate::canonical::hex(&Sha1::digest(ledger.name.as_bytes()))[..8].to_string());
    }
    guid_tag(&ledger.guid, &format!("ledger {:?}", ledger.name))
}

/// A guid-based tag when `name` names a real ledger in `book.ledgers` (the case at risk of the
/// read-shape rename churn this module exists to fix); the plain sha1 hash of `name` itself
/// otherwise -- for a `name` that is NOT a Book ledger: a client-config alias, a synthetic
/// sentinel bucket (e.g. `cash_payments_40a3::UNIDENTIFIED_PARTY`), or a Trial Balance row with no
/// matching ledger master. None of those is itself read fresh from Tally's ledger export on every
/// capture, so none of them carries this module's rename-churn risk; hashing the string itself is
/// already stable for them, and refusing would drop a real row for no benefit. When `name` DOES
/// name a real Book ledger but that ledger's own GUID is blank, see [`ledger_tag`] -- this
/// delegates to it and no longer errs for that case.
pub fn stable_ledger_tag(book: &Book, name: &str) -> Result<String> {
    match book.ledgers.get(name) {
        Some(ledger) => ledger_tag(ledger),
        None => Ok(crate::canonical::hex(&Sha1::digest(name.as_bytes()))[..8].to_string()),
    }
}

/// Errs with [`AuditError::DuplicateGuid`] when two different names in `ledgers` normalise
/// (`normalize_guid`) to the same non-blank Tally GUID -- a corrupt read, not a legitimate case:
/// Tally does not hand out one GUID to two masters. Left unrefused, [`guid_tag`]/[`ledger_tag`]/
/// [`stable_ledger_tag`] would silently hand both ledgers the same figure/finding id, merging
/// their rows under one id, which is worse than a refusal. Ledgers with a blank GUID are exempt
/// (that is [`ledger_tag`]'s name-hash fallback's concern, not this one's -- two GUID-less
/// ledgers sharing "no identity" is not the corrupt-read case this guards against). Called from
/// [`crate::book::load_book`], once per read, over every ledger.
pub fn check_no_duplicate_ledger_guids(ledgers: &BTreeMap<String, Ledger>) -> Result<()> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for (name, ledger) in ledgers {
        let normalized = normalize_guid(&ledger.guid);
        if normalized.is_empty() {
            continue;
        }
        if let Some(prior) = seen.get(normalized.as_str()) {
            if *prior != name.as_str() {
                return Err(AuditError::DuplicateGuid(format!(
                    "ledgers {prior:?} and {name:?} share the same Tally GUID {:?} (normalised); \
                     refusing -- this would give them the same figure id",
                    ledger.guid
                )));
            }
        }
        seen.insert(normalized, name.as_str());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::Book;
    use std::collections::BTreeMap;

    // Pinned against the reference implementation's tae.ledger_ids (computed with the Python
    // function itself, 2026-09-19; the synthetic all-zero GUID's tag was computed with that same
    // function on 2026-09-21): guid_tag("abc") and stable_ledger_tag on a book with one
    // real ledger, "ROUND OFF"/guid "g-1", plus a name that is not a book ledger at all.
    #[test]
    fn guid_tag_matches_the_reference_implementation() {
        assert_eq!(guid_tag("abc", "x").unwrap(), "a9993e36");
        assert_eq!(guid_tag("g-1", "x").unwrap(), "6d5494d3");
        assert_eq!(
            guid_tag("00000000-0000-4000-8000-0000000000b1", "x").unwrap(),
            "d2101a72"
        );
    }

    /// The reference normalises with Python's `str.strip().lower()`, so a non-ASCII or
    /// control-character GUID must hash as it does there. Expected tags are the reference's own
    /// (`tae.ledger_ids.guid_tag`, Python 3.13).
    #[test]
    fn guid_tag_strips_and_lowercases_as_python_does() {
        for (guid, want) in [
            ("\u{c9}BC-1", "eae20bd6"),
            ("\u{1c}abc-1", "097be456"),
            ("abc-1\u{1f}", "097be456"),
            ("\u{a0}abc-1\u{2003}", "097be456"),
            ("\u{212a}-1", "4136a771"),
        ] {
            assert_eq!(guid_tag(guid, "ledger").unwrap(), want, "{guid:?}");
        }
    }

    #[test]
    fn guid_tag_blank_refuses() {
        assert!(matches!(
            guid_tag("", "ledger 'X'"),
            Err(AuditError::MissingGuid(_))
        ));
    }

    #[test]
    fn guid_tag_whitespace_only_refuses() {
        // Trimming must happen BEFORE the blank check, or "   " would hash as a non-blank GUID.
        assert!(matches!(
            guid_tag("   ", "ledger 'X'"),
            Err(AuditError::MissingGuid(_))
        ));
    }

    #[test]
    fn guid_tag_normalises_mixed_case_before_hashing() {
        // Pinned against the reference implementation: guid_tag of the lowercase form is the
        // same "d2101a72" as guid_tag_matches_the_reference_implementation below pins for the
        // already-lowercase GUID -- a mixed-case export of the same identity must produce the
        // identical tag, not a different one.
        assert_eq!(
            guid_tag("00000000-0000-4000-8000-0000000000B1", "x").unwrap(),
            guid_tag("00000000-0000-4000-8000-0000000000b1", "x").unwrap()
        );
        assert_eq!(
            guid_tag("00000000-0000-4000-8000-0000000000B1", "x").unwrap(),
            "d2101a72"
        );
    }

    #[test]
    fn guid_tag_trims_surrounding_whitespace_before_hashing() {
        assert_eq!(
            guid_tag("  00000000-0000-4000-8000-0000000000b1  ", "x").unwrap(),
            guid_tag("00000000-0000-4000-8000-0000000000b1", "x").unwrap()
        );
    }

    fn ledger_with(name: &str, guid: &str) -> Ledger {
        Ledger {
            name: name.to_string(),
            parent: "Indirect Expenses".to_string(),
            chain: vec!["Indirect Expenses".to_string()],
            chain_complete: true,
            master_opening_paise: 0,
            guid: guid.to_string(),
            masterid: None,
        }
    }

    fn book_with(name: &str, guid: &str) -> Book {
        book_with_ledgers(vec![ledger_with(name, guid)])
    }

    fn book_with_ledgers(ledgers: Vec<Ledger>) -> Book {
        let mut map = BTreeMap::new();
        for l in ledgers {
            map.insert(l.name.clone(), l);
        }
        Book {
            company_name: "Co".to_string(),
            company_guid: "co-guid".to_string(),
            read_at: String::new(),
            groups: BTreeMap::new(),
            group_masters: BTreeMap::new(),
            ledgers: map,
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
    fn ledger_tag_blank_guid_falls_back_to_name_hash() {
        // A Book ledger with a blank GUID no longer errs (bridge PR #511 review): tally-read-v1
        // does not require ledger GUIDs. Pinned against the reference implementation's
        // hashlib.sha1(name.encode()).hexdigest()[:8].
        let l = ledger_with("X", "");
        assert_eq!(ledger_tag(&l).unwrap(), "c032adc1");
    }

    #[test]
    fn stable_ledger_tag_real_ledger_with_no_guid_falls_back_to_name_hash() {
        let b = book_with("X", "");
        assert_eq!(stable_ledger_tag(&b, "X").unwrap(), "c032adc1");
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

    #[test]
    fn check_no_duplicate_ledger_guids_refuses_when_two_ledgers_share_a_normalised_guid() {
        let ledgers = book_with_ledgers(vec![
            ledger_with("RAM TRADERS", "G-1"),
            ledger_with("SHYAM TRADERS", " g-1 "),
        ])
        .ledgers;
        let err = check_no_duplicate_ledger_guids(&ledgers).unwrap_err();
        assert!(matches!(err, AuditError::DuplicateGuid(_)));
    }

    #[test]
    fn check_no_duplicate_ledger_guids_allows_two_guidless_ledgers() {
        // Blank GUIDs are exempt: two ledgers with "no identity" is not the corrupt-read case
        // this guards against (they already get the name-hash fallback in ledger_tag).
        let ledgers = book_with_ledgers(vec![
            ledger_with("RAM TRADERS", ""),
            ledger_with("SHYAM TRADERS", ""),
        ])
        .ledgers;
        assert!(check_no_duplicate_ledger_guids(&ledgers).is_ok());
    }

    #[test]
    fn check_no_duplicate_ledger_guids_allows_distinct_guids() {
        let ledgers = book_with_ledgers(vec![
            ledger_with("RAM TRADERS", "g-1"),
            ledger_with("SHYAM TRADERS", "g-2"),
        ])
        .ledgers;
        assert!(check_no_duplicate_ledger_guids(&ledgers).is_ok());
    }
}
