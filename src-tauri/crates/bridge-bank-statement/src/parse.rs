//! Statement rows from word boxes, and the binding of a statement to its
//! account.

use crate::bank::{page_footer, Bank, Layout, LineKind};
use crate::geometry::{dewrap, lines, matches, Line, Page, Word, WRAP_TOLERANCE};
use crate::refusal::Refusal;
use crate::text::squash;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

/// The words of one visual line, bucketed by column.
pub type Cells = BTreeMap<&'static str, Vec<Word>>;

/// One statement row: a value per column, plus `<name>_spaced` for each text
/// column. Both readings of a wrapped cell are kept on purpose: the de-wrapped
/// form is the only one where a reference number is intact, the space-joined
/// form the only one that never welds two words together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row(BTreeMap<String, String>);

impl Row {
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        )
    }

    /// The cell's value, or empty when the column is absent.
    pub fn get(&self, column: &str) -> &str {
        self.0.get(column).map_or("", String::as_str)
    }

    pub fn set(&mut self, column: &str, value: impl Into<String>) {
        self.0.insert(column.to_string(), value.into());
    }

    /// Every column and value, in column-name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// y of the line that starts this page's transaction table, or `None`.
fn table_top(page_lines: &[Line], bank: Bank) -> Option<f64> {
    bank.top_anchors().iter().find_map(|anchor| {
        page_lines
            .iter()
            .find(|line| matches(line, &[anchor]))
            .map(|line| line.y)
    })
}

/// Statement rows, in printed order (`parse_pages`).
///
/// Page anchors, column bounds, row-start detection, multi-line row assembly
/// and the wrap heuristic, reachable without a PDF.
pub fn parse_pages(pages: &[Page], bank: Bank) -> Vec<Row> {
    let mut rows: Vec<BTreeMap<&'static str, Vec<(String, f64)>>> = Vec::new();
    let mut stop = false;
    for page in pages {
        if stop {
            break;
        }
        let page_lines = lines(page);
        let Some(top) = table_top(&page_lines, bank) else {
            continue;
        };
        if !bank.end_anchors().is_empty()
            && page_lines
                .iter()
                .any(|line| matches(line, bank.end_anchors()))
        {
            stop = true;
        }
        for line in &page_lines {
            if line.y <= top {
                continue;
            }
            let header = bank.header_words();
            if !header.is_empty()
                && line
                    .words
                    .iter()
                    .all(|word| header.contains(&word.text.as_str()))
            {
                continue;
            }
            let mut cells = Cells::new();
            for word in &line.words {
                cells
                    .entry(bank.column_of(word.x0, word.x1))
                    .or_default()
                    .push(word.clone());
            }
            // A line that opens a transaction is a transaction whatever else it
            // says: the date decides, and the footer anchor only breaks ties.
            let started = bank.is_row_start(&cells);
            if !started && !bank.bottom_anchors().is_empty() && matches(line, bank.bottom_anchors())
            {
                break;
            }
            if started {
                rows.push(
                    bank.columns()
                        .iter()
                        .map(|(_, _, name)| (*name, Vec::new()))
                        .collect(),
                );
            }
            let Some(current) = rows.last_mut() else {
                continue;
            };
            for (name, cell) in &cells {
                if !started && bank.row_scoped().contains(name) {
                    continue;
                }
                let text = cell
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let right = cell
                    .iter()
                    .map(|word| word.x1)
                    .fold(f64::NEG_INFINITY, f64::max);
                current.entry(name).or_default().push((text, right));
            }
        }
    }

    rows.into_iter()
        .map(|cells| {
            let mut row = Row::default();
            for (_, _, name) in bank.columns() {
                let fragments = cells.get(name).map_or(&[][..], Vec::as_slice);
                if let Some(edge) = bank.text_edge(name) {
                    row.set(name, dewrap(fragments, edge, WRAP_TOLERANCE));
                    let spaced = fragments
                        .iter()
                        .map(|(text, _)| text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ");
                    row.set(&format!("{name}_spaced"), squash(&spaced));
                } else {
                    let joined: String = fragments.iter().map(|(text, _)| text.as_str()).collect();
                    row.set(name, joined.replace(',', ""));
                }
            }
            row
        })
        .collect()
}

/// Statement rows for any layout, or the refusal a single-line table raises.
///
/// A column layout cannot tell a stray line from a wrapped cell, so it has no
/// refusals of its own here; the balance replay is its proof.
pub fn parse_statement(pages: &[Page], bank: Bank) -> Result<Vec<Row>, Refusal> {
    match bank.layout() {
        Layout::Columns => Ok(parse_pages(pages, bank)),
        Layout::SingleLine => parse_single_line_pages(pages, bank),
    }
}

/// Rows of a table that prints each transaction on one line.
///
/// Stricter than the column reader, because it can be: a line that opens with
/// a date but is not a row refuses, and so does any unrecognised line followed
/// by a later row, since in a one-line-per-row table that line can only be a
/// wrapped cell or a row this rule misread. Unrecognised lines after the last
/// row are allowed.
///
/// With no printed totals, a dropped whole page whose two sides cancel is what
/// the balance replay cannot see, so every page must also print `Page N of M`
/// with N its position and M the document's page count.
fn parse_single_line_pages(pages: &[Page], bank: Bank) -> Result<Vec<Row>, Refusal> {
    let mut rows = Vec::new();
    let mut stray = false;
    for (index, page) in pages.iter().enumerate() {
        let page_lines = lines(page);
        let texts: Vec<Vec<&str>> = page_lines
            .iter()
            .map(|line| line.words.iter().map(|word| word.text.as_str()).collect())
            .collect();
        // a row's remarks cannot stand in for the page's footer
        if !texts.iter().any(|words| {
            !matches!(bank.classify_line(words), LineKind::Row(_))
                && page_footer(words) == Some((index + 1, pages.len()))
        }) {
            return Err(Refusal::new(
                "page_sequence_unproven",
                format!(
                    "page {} does not print \"Page {} of {}\"; without printed totals, every page must be accounted for",
                    index + 1,
                    index + 1,
                    pages.len()
                ),
            ));
        }
        let Some(top) = table_top(&page_lines, bank) else {
            continue;
        };
        for (line, words) in page_lines.iter().zip(&texts) {
            if line.y <= top {
                continue;
            }
            let number = rows.len() + 1;
            match bank.classify_line(words) {
                LineKind::Row(row) => {
                    if stray {
                        return Err(Refusal::at_row(
                            "unexpected_line_in_table",
                            number,
                            format!(
                                "an unrecognised line precedes row {number}; in a one-line-per-row table it is a wrapped cell or a misread row"
                            ),
                        ));
                    }
                    rows.push(row);
                }
                LineKind::Furniture => {}
                LineKind::MalformedRow => {
                    return Err(Refusal::at_row(
                        "malformed_row",
                        number,
                        format!(
                            "row {number} opens with a date but is not the {} row shape",
                            bank.name().to_uppercase()
                        ),
                    ));
                }
                LineKind::Other => stray = true,
            }
        }
    }
    Ok(rows)
}

static DIGIT_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").unwrap());

/// Digit runs printed on the statement's own account-number line, from the
/// first page that has one (`account_number_runs`).
///
/// Only the line the bank labels as the account number: a header block also
/// prints a phone number, a customer id, an IFSC, a MICR code and a postcode,
/// and on a real statement four different wrong tails matched those.
pub fn account_number_runs(pages: &[Page], bank: Bank) -> BTreeSet<String> {
    let mut runs = BTreeSet::new();
    if bank.account_anchors().is_empty() {
        return runs;
    }
    for page in pages {
        for line in lines(page) {
            if !matches(&line, bank.account_anchors()) {
                continue;
            }
            for word in &line.words {
                runs.extend(
                    DIGIT_RUN
                        .find_iter(&word.text)
                        .map(|found| found.as_str().to_string()),
                );
            }
        }
        if !runs.is_empty() {
            break;
        }
    }
    runs
}

/// The digits of the operator's free-form account label.
pub fn account_digits(account_tail: &str) -> String {
    DIGIT_RUN
        .find_iter(account_tail)
        .map(|found| found.as_str())
        .collect()
}

/// Refuse a statement that does not print the account being posted to
/// (`require_account_match`). Returns the statement's own account number.
///
/// The running-balance proof validates the PDF's own arithmetic and nothing
/// else, so it is equally happy to certify the wrong account's statement. This
/// is the one check that ties the document to the ledger.
///
/// Residual, stated rather than assumed: the label line can carry a second
/// number (HDFC prints a product code beside it), so a tail that happens to end
/// that number would also pass.
pub fn require_account_match(
    pages: &[Page],
    bank: Bank,
    account_tail: &str,
) -> Result<String, Refusal> {
    let digits = account_digits(account_tail);
    let digit_count = digits.chars().count();
    if digit_count < 4 {
        return Err(Refusal::new(
            "unbindable_account",
            format!(
                "the account label carries {digit_count} digits; at least 4 are needed to bind the statement to the ledger"
            ),
        ));
    }
    let runs = account_number_runs(pages, bank);
    if runs.is_empty() {
        return Err(Refusal::new(
            "no_account_number_line",
            format!(
                "this statement prints no account-number line; the layout has changed, or this is not a {} statement",
                bank.name().to_uppercase()
            ),
        ));
    }
    let matched: Vec<&String> = runs.iter().filter(|run| run.ends_with(&digits)).collect();
    match matched.as_slice() {
        [] => Err(Refusal::new(
            "account_not_in_statement",
            "no number on this statement's account-number line ends with the account digits supplied; refusing to post it anywhere",
        )),
        [only] => Ok((*only).clone()),
        _ => Err(Refusal::new(
            "ambiguous_account_match",
            "the account digits match more than one number on the account-number line; supply more digits",
        )),
    }
}
