//! Compare PDFium's reading of a statement with `pdftotext`'s, counts only.
//!
//! The parser's PDFium path is proven equal to `pdftotext -bbox-layout` only on
//! synthetic PDFs (non-embedded Courier, RC4). This is the measurement for a
//! real statement, meant to run on the operator's own machine: it prints how
//! many pages, words, lines and rows each reading produced, how many rows are
//! identical, and **which columns** differ, never a cell's value, a word, a
//! name, an amount or the account number. Its output is safe to paste into an
//! issue.
//!
//! ```text
//! pdftotext -bbox-layout -upw "$(cat statement.password)" statement.pdf poppler.xml
//! cargo run --example compare_extraction -- \
//!     --pdfium /path/to/libpdfium.dylib --pdf statement.pdf \
//!     --password-file statement.password --bank hdfc --poppler-xml poppler.xml
//! ```
//!
//! `pdftotext` takes the password as an argument, which other local users can
//! see in the process list while it runs; this tool reads it only from a file
//! that must not be readable by group or others. Delete `poppler.xml` after:
//! it holds the statement's full text.

use bridge_bank_statement::bank::Bank;
use bridge_bank_statement::bbox::read_pages;
use bridge_bank_statement::geometry::{lines, Page};
use bridge_bank_statement::parse::{parse_statement, Row};
use bridge_bank_statement::pdf::{engine, extract_pages};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

struct Arguments {
    pdfium: PathBuf,
    pdf: PathBuf,
    password_file: PathBuf,
    bank: Bank,
    poppler_xml: PathBuf,
}

const OPTIONS: [&str; 5] = ["pdfium", "pdf", "password-file", "bank", "poppler-xml"];

fn arguments() -> Result<Arguments, String> {
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    // No argument is ever echoed: a misplaced token may be the password.
    let mut args = std::env::args().skip(1).enumerate();
    while let Some((position, key)) = args.next() {
        let Some(name) = key.strip_prefix("--").filter(|name| OPTIONS.contains(name)) else {
            return Err(format!(
                "argument {} is not one of --{}",
                position + 1,
                OPTIONS.join(", --")
            ));
        };
        let (_, value) = args
            .next()
            .ok_or_else(|| format!("argument {} needs a value", position + 1))?;
        named.insert(name.to_string(), value);
    }
    let mut take = |name: &str| {
        named
            .remove(name)
            .ok_or_else(|| format!("--{name} is required"))
    };
    Ok(Arguments {
        pdfium: take("pdfium")?.into(),
        pdf: take("pdf")?.into(),
        password_file: take("password-file")?.into(),
        bank: Bank::from_name(&take("bank")?).ok_or("--bank must be sbi, hdfc or ubi")?,
        poppler_xml: take("poppler-xml")?.into(),
    })
}

fn read_password(path: &PathBuf) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|_| "password file unreadable")?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err("password file must not be readable by group or others (chmod 600)".into());
        }
    }
    let text = std::fs::read_to_string(path).map_err(|_| "password file unreadable")?;
    Ok(text.trim_end_matches(['\r', '\n']).to_string())
}

fn words(pages: &[Page]) -> usize {
    pages.iter().map(Vec::len).sum()
}

fn line_count(pages: &[Page]) -> usize {
    pages.iter().map(|page| lines(page).len()).sum()
}

/// The largest horizontal edge difference between words matched line by line,
/// when both readings grouped every page into the same words; `None` otherwise.
fn horizontal_agreement(left: &[Page], right: &[Page]) -> Option<f64> {
    if left.len() != right.len() {
        return None;
    }
    let mut largest: f64 = 0.0;
    for (left, right) in left.iter().zip(right) {
        let (left, right) = (lines(left), lines(right));
        if left.len() != right.len() {
            return None;
        }
        for (left, right) in left.iter().zip(&right) {
            if left.words.len() != right.words.len()
                || left
                    .words
                    .iter()
                    .zip(&right.words)
                    .any(|(a, b)| a.text != b.text)
            {
                return None;
            }
            for (a, b) in left.words.iter().zip(&right.words) {
                largest = largest.max((a.x0 - b.x0).abs()).max((a.x1 - b.x1).abs());
            }
        }
    }
    Some(largest)
}

fn column_names(rows: &[Row]) -> Vec<String> {
    let mut names: Vec<String> = rows
        .iter()
        .flat_map(|row| row.iter().map(|(name, _)| name.to_string()))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn main() -> ExitCode {
    match run() {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("compare_extraction: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let arguments = arguments()?;
    let password = read_password(&arguments.password_file)?;
    let bytes = std::fs::read(&arguments.pdf).map_err(|_| "statement unreadable")?;
    let engine = engine(&arguments.pdfium).map_err(|refusal| refusal.category.to_string())?;
    let pdfium_pages =
        extract_pages(engine, &bytes, &password).map_err(|refusal| refusal.category.to_string())?;
    drop(password);
    let xml =
        std::fs::read_to_string(&arguments.poppler_xml).map_err(|_| "poppler XML unreadable")?;
    let poppler_pages = read_pages(&xml);
    drop(xml);
    if poppler_pages.is_empty() {
        // a wrong or truncated file would otherwise read as a disagreement
        return Err("the poppler XML has no pages; pass `pdftotext -bbox-layout` output".into());
    }

    let outcome = |pages: &[Page]| parse_statement(pages, arguments.bank);
    let (pdfium_rows, poppler_rows) = (outcome(&pdfium_pages), outcome(&poppler_pages));
    let rows_report = |rows: &Result<Vec<Row>, bridge_bank_statement::Refusal>| match rows {
        Ok(rows) => serde_json::json!({"rows": rows.len()}),
        Err(refusal) => serde_json::json!({"refused": refusal.category, "row": refusal.row}),
    };

    let mut comparison = serde_json::json!(null);
    if let (Ok(left), Ok(right)) = (&pdfium_rows, &poppler_rows) {
        let mut differing_by_column: BTreeMap<String, usize> = BTreeMap::new();
        let mut identical = 0;
        let mut first_difference = None;
        // Rows are paired by position, so once one reading gains or loses a row
        // every later pair differs; `first_differing_row` is the useful number then.
        for (index, (a, b)) in left.iter().zip(right).enumerate() {
            if a == b {
                identical += 1;
                continue;
            }
            first_difference.get_or_insert(index + 1);
            let mut names = column_names(std::slice::from_ref(a));
            names.extend(column_names(std::slice::from_ref(b)));
            names.sort();
            names.dedup();
            for name in names {
                if a.get(&name) != b.get(&name) {
                    *differing_by_column.entry(name).or_default() += 1;
                }
            }
        }
        comparison = serde_json::json!({
            "compared_rows": left.len().min(right.len()),
            "identical_rows": identical,
            "row_count_equal": left.len() == right.len(),
            "first_differing_row": first_difference,
            "differing_rows_by_column": differing_by_column,
        });
    }

    let report = serde_json::json!({
        "bank": arguments.bank.name(),
        "pdfium": {
            "pages": pdfium_pages.len(),
            "words": words(&pdfium_pages),
            "lines": line_count(&pdfium_pages),
            "parse": rows_report(&pdfium_rows),
        },
        "pdftotext": {
            "pages": poppler_pages.len(),
            "words": words(&poppler_pages),
            "lines": line_count(&poppler_pages),
            "parse": rows_report(&poppler_rows),
        },
        "same_words_and_lines": horizontal_agreement(&pdfium_pages, &poppler_pages).is_some(),
        "largest_horizontal_edge_difference_pt":
            horizontal_agreement(&pdfium_pages, &poppler_pages).map(|value| (value * 1000.0).round() / 1000.0),
        "rows": comparison,
    });
    serde_json::to_string_pretty(&report).map_err(|_| "report serialization failed".to_string())
}
