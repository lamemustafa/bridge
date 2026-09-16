//! The PDFium path, on synthetic encrypted statements.
//!
//! These need the PDFium shared library, so they are ignored by default. Run
//! them with `BRIDGE_PDFIUM_LIBRARY=/path/to/libpdfium.dylib cargo test -p
//! bridge-bank-statement -- --ignored`; without the variable they fail rather
//! than pass vacuously.
//!
//! The strongest claim here is row equality: PDFium's words, fed through the
//! parser, produce exactly the rows that `pdftotext -bbox-layout` words of the
//! same file produce. The `.pdftotext.xml` beside each PDF is poppler's output
//! for it (see fixtures/PROVENANCE.md), and the pinned digests were computed by
//! the Python reference over that same capture.

mod common;

use bridge_bank_statement::bank::Bank;
use bridge_bank_statement::bbox::read_pages;
use bridge_bank_statement::geometry::{lines, Page};
use bridge_bank_statement::mapping::{Mapping, MappingRow};
use bridge_bank_statement::money::{reconcile, verify_against_statement, Controls};
use bridge_bank_statement::parse::{parse_pages, parse_statement, require_account_match, Row};
use bridge_bank_statement::pdf::{engine, extract_pages, PdfEngine};
use bridge_bank_statement::proposals::{build, selfcheck, BuildOptions, VoucherType};
use common::*;
use std::path::PathBuf;

fn pdfium() -> &'static PdfEngine {
    let path = std::env::var_os("BRIDGE_PDFIUM_LIBRARY")
        .map(PathBuf::from)
        .expect("BRIDGE_PDFIUM_LIBRARY must name the PDFium shared library");
    engine(&path).unwrap()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn pdf(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap()
}

fn poppler(name: &str) -> Vec<Page> {
    read_pages(&std::fs::read_to_string(fixture(name)).unwrap())
}

/// `json.dumps(row, sort_keys=True)` for ASCII rows.
fn python_json(row: &Row) -> String {
    let fields: Vec<String> = row
        .iter()
        .map(|(key, value)| format!("{}: {}", serde_json::json!(key), serde_json::json!(value)))
        .collect();
    format!("{{{}}}", fields.join(", "))
}

fn assert_python_digests(rows: &[Row], bank: Bank, expected: [&str; 4]) {
    let parties = digest(rows.iter().map(|row| bank.party(row)));
    let references = digest(rows.iter().map(|row| {
        let (mode, value) = bank.reference(row);
        format!("{mode}:{value}")
    }));
    let narrations = digest(rows.iter().map(|row| row.get("narr").to_string()));
    let whole = digest(rows.iter().map(python_json));
    assert_eq!(
        [parties.as_str(), &references, &narrations, &whole],
        expected
    );
}

/// How PDFium's word boxes differ from poppler's, matching words line by line:
/// the largest horizontal difference, and the spread of the vertical shift (a
/// shift that is the same for every word cannot change grouping or anchors).
fn box_differences(left: &[Page], right: &[Page]) -> (f64, f64, f64) {
    assert_eq!(left.len(), right.len(), "page count");
    let mut horizontal: f64 = 0.0;
    let mut tops: Vec<f64> = Vec::new();
    let mut bottoms: Vec<f64> = Vec::new();
    for (left, right) in left.iter().zip(right) {
        let (left, right) = (lines(left), lines(right));
        assert_eq!(left.len(), right.len(), "line count");
        for (left, right) in left.iter().zip(&right) {
            let texts = |line: &bridge_bank_statement::geometry::Line| {
                line.words
                    .iter()
                    .map(|word| word.text.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(texts(left), texts(right));
            for (a, b) in left.words.iter().zip(&right.words) {
                horizontal = horizontal.max((a.x0 - b.x0).abs()).max((a.x1 - b.x1).abs());
                tops.push(a.y0 - b.y0);
                bottoms.push(a.y1 - b.y1);
            }
        }
    }
    let spread = |values: &[f64]| {
        let low = values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        high - low
    };
    (horizontal, spread(&tops), spread(&bottoms))
}

fn assert_geometry_matches_poppler(pages: &[Page], reference: &[Page]) {
    let (horizontal, top_spread, bottom_spread) = box_differences(pages, reference);
    assert!(
        horizontal < 0.01,
        "horizontal edges differ from poppler's by {horizontal}pt"
    );
    // measured: y0 sits 1.43pt above and y1 0.99pt below poppler's on 7pt
    // Courier, identically for every word
    assert!(top_spread < 0.01, "the y0 shift varies by {top_spread}pt");
    assert!(
        bottom_spread < 0.01,
        "the y1 shift varies by {bottom_spread}pt"
    );
}

#[test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
fn pdfium_reproduces_poppler_rows_on_the_synthetic_hdfc_statement() {
    let bank = Bank::Hdfc;
    let pages = extract_pages(pdfium(), &pdf("hdfc-synthetic.pdf"), "synthetic-user-4321").unwrap();
    let reference = poppler("hdfc-synthetic.pdftotext.xml");
    assert_geometry_matches_poppler(&pages, &reference);

    let rows = parse_pages(&pages, bank);
    assert_eq!(rows, parse_pages(&reference, bank));
    assert_eq!(rows.len(), 6, "the post-footer row and page 3 are not rows");
    assert_python_digests(
        &rows,
        bank,
        [
            "46702cd6f1f58c77",
            "eaef9767ccaa7c8d",
            "677f33d5118c1331",
            "ea0877741deab473",
        ],
    );

    // the 12-digit reference really was split by the PDF, and rejoined
    assert!(rows[0].get("narr_spaced").contains("6123 45678901"));
    assert!(rows[0].get("narr").contains("-612345678901-"));
    assert_eq!(
        bank.reference(&rows[0]),
        ("UPI".to_string(), "612345678901".to_string())
    );
    assert_eq!(bank.party(&rows[4]), "SILVER OAK MUTUAL");

    // the phone line also ends 4321, but only the account line binds
    assert_eq!(
        require_account_match(&pages, bank, "xx4321").unwrap(),
        "00000000004321"
    );
    refuses(
        require_account_match(&pages, bank, "xx9876"),
        "account_not_in_statement",
    );

    let controls = Controls::parse("1,000.00", "1,02,200.00", "8,800.00", "1,10,000.00").unwrap();
    reconcile(&rows, &controls.opening, &controls.closing).unwrap();
    verify_against_statement(
        &rows,
        controls.debits.as_ref().unwrap(),
        controls.credits.as_ref().unwrap(),
    )
    .unwrap();

    let mapping = Mapping::from_rows(
        [
            ("NORTHWIND TRADERS", "Northwind Traders", "auto"),
            ("SILVER OAK MUTUAL", "Silver Oak Mutual Fund", "auto"),
        ]
        .map(|(party, ledger, treatment)| MappingRow {
            origin: party.to_string(),
            party: party.to_string(),
            ledger: ledger.to_string(),
            treatment: Some(treatment.to_string()),
        }),
    )
    .unwrap();
    let built = build(
        &rows,
        bank,
        &mapping,
        &BuildOptions {
            bank_ledger: "Synthetic Bank Ledger",
            suspense_ledger: "Suspense",
            account_label: "Synthetic CA xx4321",
            account_number: "00000000004321",
            date_from: None,
            date_to: None,
        },
    )
    .unwrap();
    let counted = selfcheck(&built, "Synthetic Bank Ledger").unwrap();
    assert_eq!(counted.vouchers, 6);
    assert_eq!(built.proposals[0].voucher_type, VoucherType::Receipt);
    assert_eq!(built.proposals[0].entries[1].ledger, "Northwind Traders");
    assert_eq!(
        built
            .records
            .iter()
            .filter(|record| record.suspense)
            .count(),
        4
    );
}

#[test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
fn an_owner_password_only_statement_opens_with_its_owner_password() {
    let bank = Bank::Sbi;
    let bytes = pdf("sbi-owner-password-only.pdf");
    let pages = extract_pages(pdfium(), &bytes, "synthetic-owner-7788").unwrap();
    let reference = poppler("sbi-owner-password-only.pdftotext.xml");
    assert_geometry_matches_poppler(&pages, &reference);
    let rows = parse_pages(&pages, bank);
    assert_eq!(rows, parse_pages(&reference, bank));
    assert_python_digests(
        &rows,
        bank,
        [
            "88bbb8b9dde12f19",
            "7c9463bf9aeacaff",
            "49c5c45254af2049",
            "61743d7843375634",
        ],
    );
    assert!(rows[0].get("narr").contains("UPI/CR/712345678901/"));
    assert!(rows[0].get("narr_spaced").contains("71234567890 1"));
    assert_eq!(
        bank.reference(&rows[0]),
        ("UPI".to_string(), "712345678901 (blue@okzz)".to_string())
    );
    assert_eq!(
        require_account_match(&pages, bank, "xx7788").unwrap(),
        "00000000007788"
    );
    let controls = Controls::parse("50,000.00", "51,998.50", "1,250.75", "3,249.25").unwrap();
    reconcile(&rows, &controls.opening, &controls.closing).unwrap();
    verify_against_statement(
        &rows,
        controls.debits.as_ref().unwrap(),
        controls.credits.as_ref().unwrap(),
    )
    .unwrap();

    for wrong in ["", "wrong", "synthetic-owner-778"] {
        refuses(extract_pages(pdfium(), &bytes, wrong), "unreadable_pdf");
    }
}

#[test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
fn a_union_bank_statement_reads_one_row_per_line() {
    let bank = Bank::Ubi;
    let pages = extract_pages(pdfium(), &pdf("ubi-synthetic.pdf"), "synthetic-user-7788").unwrap();
    let rows = parse_statement(&pages, bank).unwrap();
    let read: Vec<[&str; 6]> = rows
        .iter()
        .map(|row| {
            [
                row.get("date"),
                row.get("ref"),
                row.get("narr"),
                row.get("dr"),
                row.get("cr"),
                row.get("bal"),
            ]
        })
        .collect();
    assert_eq!(
        read,
        [
            [
                "01-08-2026",
                "A12345678",
                "UPIAB/612345678901/CR/NORTHWIND TRADERS/ZZZZ/nw@okzz",
                "",
                "1500.00",
                "11500.00"
            ],
            [
                "02-08-2026",
                "A1234567",
                "NEFT:BLUE RIVER CO ZZZZN12345678901",
                "250.50",
                "",
                "11249.50"
            ],
            ["03-08-2026", "A123456", "BY CASH", "", "750.50", "12000.00"],
            [
                "04-08-2026",
                "A12345",
                "IMPSAB/712345678901/GREEN FIELD LTD/9000000001",
                "13000.00",
                "",
                "-1000.00"
            ],
            [
                "05-08-2026",
                "A9AA99999",
                "MOBFT/SOME THREE WORDS/812345678901",
                "",
                "1000.00",
                "0.00"
            ],
            [
                "06-08-2026",
                "A1234",
                "CLG/SILVER OAK MUTUAL",
                "",
                "500.00",
                "500.00"
            ],
        ]
    );
    // the masked line and the CIF ID also print digits; only the account line binds
    assert_eq!(
        require_account_match(&pages, bank, "UBI SB xx7788").unwrap(),
        "000000000007788"
    );
    refuses(
        require_account_match(&pages, bank, "UBI SB xx1234"),
        "account_not_in_statement",
    );
    let controls = Controls::parse_optional("10,000.00", "500.00", None, None).unwrap();
    reconcile(&rows, &controls.opening, &controls.closing).unwrap();
}

#[test]
#[ignore = "needs PDFium: set BRIDGE_PDFIUM_LIBRARY and run with --ignored"]
fn malformed_input_is_refused_not_parsed() {
    let bytes = pdf("hdfc-synthetic.pdf");
    refuses(extract_pages(pdfium(), b"not a pdf", "x"), "unreadable_pdf");
    refuses(
        extract_pages(pdfium(), &bytes[..bytes.len() / 2], "synthetic-user-4321"),
        "unreadable_pdf",
    );
    refuses(
        extract_pages(pdfium(), &bytes, "synthetic\0user"),
        "unusable_password",
    );
    refuses(
        extract_pages(pdfium(), &pdf("hdfc-rotated.pdf"), "synthetic-user-4321"),
        "unsupported_page_rotation",
    );
    // a different library path cannot be bound once one is
    refuses(
        engine(std::path::Path::new("/nonexistent/libpdfium")),
        "pdf_engine_unavailable",
    );
}
