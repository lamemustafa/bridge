//! Column geometry and row rules for the two supported statement layouts.
//!
//! A direct port of the `SBI` and `HDFC` classes in
//! `scripts/bank_statement_import.py`. The reasoning behind each rule — why a
//! boundary is the shape it is, and which real statement it was measured
//! against — is written there at length and is not repeated here; where a rule
//! looks arbitrary, read that docstring before changing it.

use crate::date::{self, Date};
use crate::geometry::Word;
use crate::parse::{Cells, Row};
use crate::text::{is_alnum, is_upper, remove_space, squash, strip};
use regex::Regex;
use std::sync::LazyLock;

/// A statement layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    Sbi,
    Hdfc,
}

/// Why a statement cell is `(x_min, x_max, name)`: see [`Bank::columns`].
pub type Column = (f64, f64, &'static str);
type Anchors = &'static [&'static [&'static str]];

pub const DATE: &str = "date";
pub const NARRATION: &str = "narr";
pub const DEBIT: &str = "dr";
pub const CREDIT: &str = "cr";
pub const BALANCE: &str = "bal";

/// The words a party extractor returns when it could not identify a
/// counterparty. Output, not input: a mapping row naming one is refused.
pub const UNRESOLVED: &str = "UNRESOLVED";
pub const UNNAMED: &str = "UNNAMED";
pub const PARSER_SENTINELS: [&str; 2] = [UNRESOLVED, UNNAMED];

/// The digit count of an ACH bank reference — **exactly** the observed length,
/// not a minimum. A six-digit PIN code printed with a space made a reasoned
/// lower bound misattribute `ACME-400 001` to `ACME`.
pub const ACH_REFERENCE_DIGITS: usize = 10;

macro_rules! pattern {
    ($name:ident, $source:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($source).unwrap());
    };
}

impl Bank {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "sbi" => Some(Self::Sbi),
            "hdfc" => Some(Self::Hdfc),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sbi => "sbi",
            Self::Hdfc => "hdfc",
        }
    }

    /// `(x_min, x_max, name)` in PDF points. Every column not listed in
    /// [`Bank::text_edge`] is concatenated with no separator, because an amount
    /// split across lines (`1,00,000.0` / `0`) must rejoin exactly.
    pub fn columns(self) -> &'static [Column] {
        match self {
            Self::Sbi => &[
                (0.0, 85.0, "date"),
                (85.0, 140.0, "vdt"),
                (140.0, 220.0, "narr"),
                (220.0, 299.0, "ref"),
                (299.0, 356.0, "branch"),
                (356.0, 441.0, "dr"),
                (441.0, 506.0, "cr"),
                (506.0, 9999.0, "bal"),
            ],
            Self::Hdfc => &[
                (0.0, 70.0, "date"),
                (70.0, 280.0, "narr"),
                (280.0, 358.0, "ref"),
                (358.0, 400.0, "vdt"),
                (400.0, 480.0, "dr"),
                (480.0, 560.0, "cr"),
                (560.0, 9999.0, "bal"),
            ],
        }
    }

    /// The wrap edge of a text column, or `None` for a concatenated column.
    pub fn text_edge(self, column: &str) -> Option<f64> {
        match (self, column) {
            (Self::Sbi, "narr") => Some(220.0),
            (Self::Sbi, "ref") => Some(299.0),
            (Self::Hdfc, "narr") => Some(240.0),
            _ => None,
        }
    }

    /// Columns only ever populated on the row carrying the date.
    pub fn row_scoped(self) -> &'static [&'static str] {
        match self {
            Self::Sbi => &[],
            Self::Hdfc => &["ref", "vdt", "dr", "cr", "bal"],
        }
    }

    /// A line carrying every token of any group ends the current page's table.
    pub fn bottom_anchors(self) -> Anchors {
        match self {
            Self::Sbi => &[],
            Self::Hdfc => &[&["HDFC", "BANK", "LIMITED"], &["STATEMENT", "SUMMARY"]],
        }
    }

    /// ... and these end the statement entirely; later pages are not read.
    pub fn end_anchors(self) -> Anchors {
        match self {
            Self::Sbi => &[],
            Self::Hdfc => &[&["STATEMENT", "SUMMARY"]],
        }
    }

    /// A page's table starts below the first line carrying all tokens of the
    /// first anchor group that matches anywhere on the page.
    pub fn top_anchors(self) -> Anchors {
        match self {
            Self::Sbi => &[&["Txn"]],
            // page 1 repeats the column header; later pages only the period line
            Self::Hdfc => &[&["Narration"], &["Statement", "account"]],
        }
    }

    /// The label printed beside the account number. Only that line binds.
    pub fn account_anchors(self) -> Anchors {
        match self {
            Self::Sbi => &[&["Account", "Number"]],
            // "Account Status" and "Account Type" print the same first word
            Self::Hdfc => &[&["Account", "No"]],
        }
    }

    /// A line whose every word is one of these is column furniture.
    pub fn header_words(self) -> &'static [&'static str] {
        match self {
            Self::Sbi => &[
                "Txn",
                "Date",
                "Value",
                "Description",
                "Ref",
                "No./Cheque",
                "No.",
                "Branch",
                "Code",
                "Debit",
                "Credit",
                "Balance",
            ],
            Self::Hdfc => &[],
        }
    }

    pub fn column_of(self, x0: f64, x1: f64) -> &'static str {
        let centre = f64::midpoint(x0, x1);
        let columns = self.columns();
        columns
            .iter()
            .find(|(low, high, _)| *low <= centre && centre < *high)
            .map_or(columns[columns.len() - 1].2, |column| column.2)
    }

    pub fn is_row_start(self, cells: &Cells) -> bool {
        pattern!(
            SBI_DATE,
            r"^\d{1,2} (?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)$"
        );
        pattern!(HDFC_DATE, r"^\d{2}/\d{2}/\d{2}$");
        let cell: &[Word] = cells.get(DATE).map_or(&[], Vec::as_slice);
        match self {
            // SBI prints "1 Aug" then "2026" beneath it: the date cell is two words
            Self::Sbi => {
                let joined = cell
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                SBI_DATE.is_match(strip(&joined))
            }
            Self::Hdfc => cell.len() == 1 && HDFC_DATE.is_match(&cell[0].text),
        }
    }

    pub fn parse_date(self, text: &str) -> Option<Date> {
        match self {
            // "31 Jul" over "2026" in one cell; spacing after the join is not stable
            Self::Sbi => date::parse_day_month_name_year(&remove_space(text)),
            Self::Hdfc => date::parse_day_month_short_year(text),
        }
    }

    /// The date format, for refusal messages.
    pub fn date_format(self) -> &'static str {
        match self {
            Self::Sbi => "%d%b%Y",
            Self::Hdfc => "%d/%m/%y",
        }
    }

    pub fn party(self, row: &Row) -> String {
        match self {
            Self::Sbi => sbi_party(row),
            Self::Hdfc => hdfc_party(row),
        }
    }

    pub fn reference(self, row: &Row) -> (String, String) {
        match self {
            Self::Sbi => sbi_reference(row),
            Self::Hdfc => hdfc_reference(row),
        }
    }
}

fn all_digits(text: &str) -> bool {
    pattern!(DIGITS, r"^\d+$");
    DIGITS.is_match(text)
}

fn rstrip_hyphens(text: &str) -> &str {
    text.trim_end_matches('-')
}

fn sbi_party(row: &Row) -> String {
    pattern!(TRANSFER_TO, r"^TRANSFER TO \d+\s+(.+?)\s*/\s*\d+$");
    pattern!(
        CT_TRANSFER_FROM,
        r"^CT0\S*\s*\S*\s+TRANSFER FROM \d+\s+(.+?)\s*/$"
    );
    pattern!(AFTER_SLASH, r"/\s*(\S.*)$");
    pattern!(TRANSFER_TO_TRAILING, r"^TRANSFER TO (\d+)\s+(.+?)\s*/$");
    pattern!(DIGITS_AND_SPACE, r"^[\d\s]+$");
    pattern!(NEFT, r"NEFT\*(.+)$");
    pattern!(RTGS, r"RTGS UTR NO:\s*.*?\d+-(.+)$");
    pattern!(UPI, r"UPI/(?:DR|CR)/(.+)$");
    pattern!(IMPS, r"IMPS/(.+)$");
    pattern!(MASKED, r"^[A-Za-z]+-\s*[Xx]+\d+-\s*(.*)$");

    let narration = row.get("narr_spaced");
    let reference = row.get("ref_spaced");
    if narration.contains("ATM WDL") {
        return "ATM CASH WITHDRAWAL".to_string();
    }
    for pattern in [&*TRANSFER_TO, &*CT_TRANSFER_FROM] {
        if let Some(found) = pattern.captures(reference) {
            return squash(&found[1]);
        }
    }
    if let Some(found) = AFTER_SLASH.captures(reference) {
        if !all_digits(strip(&found[1])) {
            return squash(&found[1]);
        }
    }
    if let Some(found) = TRANSFER_TO_TRAILING.captures(reference) {
        if !DIGITS_AND_SPACE.is_match(&found[2]) {
            return squash(&found[2]);
        }
    }
    if let Some(found) = NEFT.captures(narration) {
        let parts: Vec<&str> = found[1].split('*').map(strip).collect();
        if parts.len() >= 3 {
            return squash(rstrip_hyphens(parts[2]));
        }
    }
    if let Some(found) = RTGS.captures(narration) {
        return squash(&found[1]);
    }
    if let Some(found) = UPI.captures(narration) {
        let parts: Vec<&str> = found[1].split('/').collect();
        if parts.len() >= 2 {
            return squash(rstrip_hyphens(parts[1]));
        }
    }
    if let Some(found) = IMPS.captures(narration) {
        let parts: Vec<&str> = found[1].split('/').collect();
        if parts.len() >= 2 {
            let inner = MASKED
                .captures(parts[1])
                .map_or(parts[1], |inner| inner.get(1).map_or("", |m| m.as_str()));
            let name = squash(rstrip_hyphens(inner));
            return if name.is_empty() {
                UNNAMED.to_string()
            } else {
                name
            };
        }
    }
    UNRESOLVED.to_string()
}

fn sbi_reference(row: &Row) -> (String, String) {
    pattern!(UTR, r"UTR NO:\s*([A-Z0-9\s]+?)-");
    pattern!(NEFT, r"NEFT\*[^*]*\*([^*]+)\*");
    pattern!(UPI, r"UPI/(?:DR|CR)/([\d\s]+?)/");
    pattern!(UPI_VPA, r"UPI/(?:DR|CR)/[^/]*/[^/]*/[^/]*/([^/]+)/");
    pattern!(IMPS, r"IMPS/([\d\s]+?)/");
    pattern!(ATM, r"ATM CASH\s*([\d\s]+?)\s*[A-Z]");
    pattern!(INB, r"(CT0[\w\s]{6,10}?)\s*TRANSFER");
    pattern!(CHEQUE, r"/\s*(\d+)\s*$");
    pattern!(INTERNAL, r"TO\s*(\d+)");

    let narration = row.get("narr");
    let reference = row.get("ref");
    let pair = |mode: &str, value: String| (mode.to_string(), value);
    if let Some(found) = UTR.captures(narration) {
        let mode = if narration.contains("RTGS") {
            "RTGS"
        } else {
            "NEFT"
        };
        return pair(mode, remove_space(&found[1]));
    }
    if let Some(found) = NEFT.captures(narration) {
        return pair("NEFT", remove_space(&found[1]));
    }
    if let Some(found) = UPI.captures(narration) {
        let tail = UPI_VPA
            .captures(narration)
            .map(|vpa| format!(" ({})", remove_space(&vpa[1])))
            .unwrap_or_default();
        return pair("UPI", remove_space(&found[1]) + &tail);
    }
    if let Some(found) = IMPS.captures(narration) {
        return pair("IMPS", remove_space(&found[1]));
    }
    if let Some(found) = ATM.captures(narration) {
        return pair("ATM", remove_space(&found[1]));
    }
    if let Some(found) = INB.captures(reference) {
        return pair("INB", remove_space(&found[1]));
    }
    if let Some(found) = CHEQUE.captures(reference) {
        return pair("CHQ", found[1].to_string());
    }
    if narration.contains("INT TRF") {
        let value = INTERNAL
            .captures(narration)
            .map(|found| remove_space(&found[1]))
            .unwrap_or_default();
        return pair("INT-TRF", value);
    }
    pair("TXN", String::new())
}

/// A bank reference: long, upper-case alphanumeric, and mostly digits
/// (`_looks_like_utr`). The digit requirement stops `INTERNATIONAL` from
/// terminating its own name.
pub fn looks_like_utr(text: &str) -> bool {
    text.chars().count() >= 10
        && is_alnum(text)
        && is_upper(text)
        && text.chars().filter(|c| is_python_digit(*c)).count() >= 4
}

fn is_python_digit(character: char) -> bool {
    pattern!(DIGIT, r"^\d$");
    let mut buffer = [0u8; 4];
    DIGIT.is_match(character.encode_utf8(&mut buffer))
}

/// The name field of a hyphen-delimited narration, which may itself contain
/// hyphens (`_bounded_party`). An empty result means the narration is not the
/// shape the rule was written for, and the caller sends it to suspense.
fn bounded_party(
    narration: &str,
    prefix_len: usize,
    is_boundary: impl Fn(&str) -> bool,
    skip: usize,
    back: usize,
) -> String {
    let parts: Vec<&str> = narration[prefix_len..].split('-').collect();
    for (index, part) in parts.iter().enumerate() {
        if index < skip {
            continue;
        }
        // the test strips whitespace: a cell wrap also lands inside a reference
        if is_boundary(&remove_space(part)) {
            let end = index.saturating_sub(back).max(skip);
            return strip(&parts[skip..end].join("-")).to_string();
        }
    }
    String::new()
}

fn upi_party(narration: &str) -> String {
    pattern!(LONG_DIGITS, r"^\d{12,}$");
    let by_vpa = bounded_party(narration, "UPI-".len(), |part| part.contains('@'), 0, 0);
    if !by_vpa.is_empty() {
        return by_vpa;
    }
    bounded_party(
        narration,
        "UPI-".len(),
        |part| LONG_DIGITS.is_match(part),
        0,
        1,
    )
}

/// The byte offset just past the first `-` at or after character 5, as
/// `narr[:narr.index("-", 5) + 1]` computes it.
fn prefix_through_hyphen(narration: &str) -> Option<usize> {
    narration
        .char_indices()
        .skip(5)
        .find(|(_, character)| *character == '-')
        .map(|(offset, _)| offset + 1)
}

fn hdfc_party(row: &Row) -> String {
    pattern!(IMPS, r"^IMPS-\d+-");
    pattern!(MASKED_ACCOUNT, r"^[Xx]{4,}\d*$");
    pattern!(NEFT, r"^NEFT (?:CR|DR)-");
    pattern!(TPT, r"^\d{10,}-TPT-[^-]*-(.+)$");
    pattern!(MASKED_UPI, r"^[X]+\d*$");
    static ACH_PARTY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"^ACH D-\s*TP ACH (.+)-\s*((?:\d\s*){{{ACH_REFERENCE_DIGITS}}})$"
        ))
        .unwrap()
    });

    // the space-preserving reading: de-wrapping welds `ACME INDUSTRIES`
    let spaced = row.get("narr_spaced");
    let narration = if spaced.is_empty() {
        row.get("narr")
    } else {
        spaced
    };
    let resolved = |name: String| {
        if name.is_empty() {
            UNRESOLVED.to_string()
        } else {
            squash(&name)
        }
    };
    if IMPS.is_match(narration) {
        let Some(prefix) = prefix_through_hyphen(narration) else {
            return UNRESOLVED.to_string();
        };
        return resolved(bounded_party(
            narration,
            prefix,
            |part| MASKED_ACCOUNT.is_match(part),
            0,
            1,
        ));
    }
    if NEFT.is_match(narration) {
        let Some(prefix) = prefix_through_hyphen(narration) else {
            return UNRESOLVED.to_string();
        };
        return resolved(bounded_party(narration, prefix, looks_like_utr, 1, 1));
    }
    if let Some(found) = TPT.captures(narration) {
        return squash(&found[1]);
    }
    if narration.starts_with("UPI-") {
        let candidate = squash(&upi_party(narration));
        if candidate.is_empty() {
            return UNRESOLVED.to_string();
        }
        return if MASKED_UPI.is_match(&candidate) {
            UNNAMED.to_string()
        } else {
            candidate
        };
    }
    if let Some(found) = ACH_PARTY.captures(narration) {
        return squash(&found[1]);
    }
    if narration.starts_with("EMI ") {
        return "EMI".to_string();
    }
    if narration.starts_with("DEBIT CARD") {
        return "DEBIT CARD FEE".to_string();
    }
    if narration.contains("INSTAALERTCHG") {
        return "BANK CHARGES".to_string();
    }
    UNRESOLVED.to_string()
}

fn hdfc_reference(row: &Row) -> (String, String) {
    pattern!(IMPS, r"^IMPS-(\d+)-");
    pattern!(UPI, r"^UPI-.*?-(\d{12,})-");
    pattern!(NEFT, r"^NEFT (?:CR|DR)-.*?-([A-Z]{2,}\w+)$");
    let narration = row.get("narr");
    let raw = row.get("ref");
    let trimmed = raw.trim_start_matches('0');
    let reference = if trimmed.is_empty() { raw } else { trimmed };
    for (mode, pattern) in [("IMPS", &*IMPS), ("UPI", &*UPI), ("NEFT", &*NEFT)] {
        if let Some(found) = pattern.captures(narration) {
            return (mode.to_string(), found[1].to_string());
        }
    }
    let mode = if narration.starts_with("NEFT") {
        "NEFT"
    } else if narration.starts_with("ACH") {
        "ACH"
    } else {
        "TXN"
    };
    (mode.to_string(), reference.to_string())
}
