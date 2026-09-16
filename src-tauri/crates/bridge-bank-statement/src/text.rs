//! String folds, ported with Python's semantics rather than Rust's defaults.
//!
//! The Python reference (`scripts/bank_statement_import.py`) uses `str.strip`,
//! `str.isspace`, `str.upper` and `re.sub(r"\s+", ...)`. Rust's
//! `char::is_whitespace` is the Unicode `White_Space` property, which omits
//! the four information separators U+001C..U+001F that Python's `isspace`
//! accepts, so every whitespace test here goes through [`is_space`].

/// Python's `str.isspace` for one character.
pub fn is_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// Python's `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `_squash`: every whitespace run becomes one space, then strip.
pub fn squash(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if is_space(character) {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(character);
        }
    }
    out
}

/// `_strip`: remove every whitespace character.
pub fn remove_space(text: &str) -> String {
    text.chars()
        .filter(|character| !is_space(*character))
        .collect()
}

/// `_key`: the mapping key. Whitespace-insensitive, everything else
/// significant, case folded with Unicode upper-casing (`straße` → `STRASSE`,
/// as Python's `str.upper` does).
///
/// Whitespace is folded because a PDF cell wrap splits one counterparty's name
/// two ways in one statement (`MERCURYM ANUFACTURERS` / `MERCURYMANUFACTURERS`).
/// Punctuation is kept: dropping it collapsed `A & B` with `AB` and posted two
/// payees to one ledger. See the Python docstring for the measurement.
pub fn mapping_key(text: &str) -> String {
    text.to_uppercase()
        .chars()
        .filter(|character| !is_space(*character))
        .collect()
}

/// `_ledger_key`: a fold deliberately **looser** than Tally's measured
/// master-name identity (protocol reference §9.4b).
///
/// Used only where looseness fails safe: refusing a voucher whose two legs
/// collapse together, and flagging a row as landing in suspense. **Never use it
/// to bind a name to a master** — that is `validate_masters`' job, and a sole
/// candidate under a loose fold is not a resolution.
pub fn ledger_key(name: &str) -> String {
    squash(&name.replace('-', " ")).to_uppercase()
}

/// Python's `str.isupper()`: at least one cased character, and no lower-case
/// one.
pub fn is_upper(text: &str) -> bool {
    let mut cased = false;
    for character in text.chars() {
        if character.is_lowercase() {
            return false;
        }
        if character.is_uppercase() {
            cased = true;
        }
    }
    cased
}

/// Python's `str.isalnum()` on a nonempty string.
pub fn is_alnum(text: &str) -> bool {
    !text.is_empty() && text.chars().all(char::is_alphanumeric)
}
