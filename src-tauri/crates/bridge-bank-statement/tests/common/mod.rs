//! Shared fixtures for the port of `scripts/bank_statement_import.test.py`.
//!
//! Three levels of fixture, as in the reference suite: sanitised real
//! `pdftotext -bbox-layout` captures (the only geometry this repository did not
//! write), constructed pages at the same geometry (for negative cases a capture
//! cannot be made to contain), and row maps for the pure functions downstream.
//!
//! Refusals are asserted by category, never by "some error happened".
#![allow(dead_code)]

use bridge_bank_statement::bbox::read_pages;
use bridge_bank_statement::geometry::{Page, Word};
use bridge_bank_statement::parse::Row;
use bridge_bank_statement::Refusal;
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use std::path::PathBuf;

pub fn refuses<T: Debug>(result: Result<T, Refusal>, category: &str) -> Refusal {
    match result {
        Err(refusal) => {
            assert_eq!(
                refusal.category, category,
                "expected refusal {category:?}, got {refusal}"
            );
            refusal
        }
        Ok(value) => panic!("expected refusal {category:?}, call succeeded with {value:?}"),
    }
}

/// `(x0, x1, text)`.
pub type Cell<'a> = (f64, f64, &'a str);
/// `(y, cells)`.
pub type ConstructedLine<'a> = (f64, &'a [Cell<'a>]);

/// One constructed page, each word 9pt tall.
pub fn page(lines: &[ConstructedLine<'_>]) -> Page {
    lines
        .iter()
        .flat_map(|(y, cells)| {
            cells
                .iter()
                .map(move |(x0, x1, text)| Word::new(*x0, *y, *x1, y + 9.0, *text))
        })
        .collect()
}

pub fn row(pairs: &[(&str, &str)]) -> Row {
    Row::from_pairs(pairs.iter().copied())
}

pub fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

pub fn capture(name: &str) -> Vec<Page> {
    let path = repository_root().join("scripts/fixtures").join(name);
    read_pages(&std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{}: {error}", path.display());
    }))
}

/// The reference suite's `digest`: sha256 over newline-joined values, first 16
/// hex digits. The pinned values below were computed by the Python suite, so
/// matching them is parity with the reference, not agreement with ourselves.
pub fn digest<I: IntoIterator<Item = String>>(values: I) -> String {
    let joined = values.into_iter().collect::<Vec<_>>().join("\n");
    Sha256::digest(joined.as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const HDFC_HEADER_ROW: &[(f64, f64, &str)] = &[
    (5.0, 30.0, "Date"),
    (72.0, 120.0, "Narration"),
    (282.0, 340.0, "Chq./Ref.No."),
    (360.0, 380.0, "Value"),
    (382.0, 396.0, "Dt"),
    (402.0, 452.0, "Withdrawal"),
    (454.0, 474.0, "Amt."),
    (482.0, 522.0, "Deposit"),
    (524.0, 544.0, "Amt."),
    (562.0, 600.0, "Closing"),
    (602.0, 640.0, "Balance"),
];

const HDFC_ACCOUNT_ROW: &[(f64, f64, &str)] = &[
    (340.0, 380.0, "Account"),
    (382.0, 396.0, "No"),
    (397.0, 400.0, ":"),
    (403.0, 470.0, "00000000001234"),
];

const HDFC_FOOTER: &[(f64, f64, &str)] = &[
    (28.0, 60.0, "HDFC"),
    (62.0, 95.0, "BANK"),
    (97.0, 140.0, "LIMITED"),
];

/// `HDFC_PAGE`: the real column geometry, a mid-reference wrap, a stray
/// fragment in a row-scoped column, and a row below the footer.
pub fn hdfc_page() -> Page {
    page(&[
        (52.0, HDFC_ACCOUNT_ROW),
        // a second header number: where a customer id or phone number sits
        (
            56.0,
            &[
                (340.0, 380.0, "Cust"),
                (382.0, 396.0, "ID"),
                (397.0, 400.0, ":"),
                (403.0, 470.0, "00000000004230"),
            ],
        ),
        (
            60.0,
            &[
                (70.0, 200.0, "Statement"),
                (205.0, 260.0, "of"),
                (265.0, 340.0, "account"),
            ],
        ),
        (100.0, HDFC_HEADER_ROW),
        (
            120.0,
            &[
                (2.0, 60.0, "01/08/26"),
                (72.0, 238.0, "UPI-NORTH-WIND-north@zzz-ZZZZ0001-1234"),
                (282.0, 350.0, "0000123456789012"),
                (360.0, 398.0, "01/08/26"),
                (482.0, 540.0, "10,000.00"),
                (562.0, 620.0, "11,000.00"),
            ],
        ),
        (
            132.0,
            &[
                (72.0, 200.0, "56789012-PAYMENT"),
                (282.0, 330.0, "CONTINUED"),
            ],
        ),
        (
            150.0,
            &[
                (2.0, 60.0, "02/08/26"),
                (72.0, 110.0, "NEFT"),
                (112.0, 190.0, "DR-ZZZZ0000001-ACME"),
                (192.0, 230.0, "EXPORTS-MUM-ZZZZZ00000000000-BB"),
                (282.0, 350.0, "ZZZZZ00000000000"),
                (360.0, 398.0, "02/08/26"),
                (402.0, 460.0, "2,500.50"),
                (562.0, 620.0, "8,499.50"),
            ],
        ),
        (170.0, HDFC_FOOTER),
        (
            190.0,
            &[
                (2.0, 60.0, "03/08/26"),
                (72.0, 200.0, "UPI-GHOST-g@z-ZZZZ0001-999999999999-X"),
                (402.0, 460.0, "1.00"),
                (562.0, 620.0, "8,498.50"),
            ],
        ),
    ])
}

/// One ACH row whose narration wraps onto a second line at the 240 edge.
pub fn ach_wrap_page(first_line: &str, continuation: &str) -> Page {
    page(&[
        (52.0, HDFC_ACCOUNT_ROW),
        (100.0, HDFC_HEADER_ROW),
        (
            120.0,
            &[
                (2.0, 60.0, "01/08/26"),
                (72.0, 238.0, first_line),
                (282.0, 350.0, "0000123456789012"),
                (360.0, 398.0, "01/08/26"),
                (402.0, 460.0, "1,000.00"),
                (562.0, 620.0, "9,000.00"),
            ],
        ),
        (132.0, &[(72.0, 200.0, continuation)]),
        (170.0, HDFC_FOOTER),
    ])
}

const SBI_HEADER: &[(f64, f64, &str)] = &[
    (88.0, 110.0, "Value"),
    (112.0, 135.0, "Date"),
    (142.0, 190.0, "Description"),
    (222.0, 235.0, "Ref"),
    (237.0, 250.0, "No."),
    (300.0, 330.0, "Branch"),
    (332.0, 350.0, "Code"),
    (400.0, 430.0, "Debit"),
    (460.0, 490.0, "Credit"),
    (510.0, 545.0, "Balance"),
];

/// `SBI_PAGE`: the date stacked over the year, and a three-line header below
/// the anchor.
pub fn sbi_page() -> Page {
    page(&[
        (
            60.0,
            &[
                (2.0, 40.0, "Account"),
                (45.0, 90.0, "Number:"),
                (95.0, 200.0, "00000000007777"),
            ],
        ),
        (90.0, &[(2.0, 20.0, "Txn"), (22.0, 45.0, "Date")]),
        (100.0, SBI_HEADER),
        (
            115.0,
            &[
                (2.0, 8.0, "1"),
                (10.0, 30.0, "Aug"),
                (88.0, 130.0, "1 Aug 2026"),
                (142.0, 218.0, "TO TRANSFER- INB NEFT UTR NO: ZZZZ1111"),
                (222.0, 296.0, "NEFT INB: ZZZZZZZZZ9 TRANSFER TO 000"),
                (360.0, 430.0, "5000.00"),
                (510.0, 570.0, "95000.00"),
            ],
        ),
        (
            127.0,
            &[
                (2.0, 25.0, "2026"),
                (142.0, 200.0, "11111- NORTH WIND TRADERS"),
                (222.0, 280.0, "0000000 / NORTH WIND TRADERS"),
            ],
        ),
    ])
}

/// `SBI_PAGE_2`: the whole header repeats while page 1's last row is in progress.
pub fn sbi_page_2() -> Page {
    page(&[
        (90.0, &[(2.0, 20.0, "Txn"), (22.0, 45.0, "Date")]),
        (100.0, SBI_HEADER),
        (
            115.0,
            &[
                (2.0, 8.0, "2"),
                (10.0, 30.0, "Aug"),
                (142.0, 200.0, "BY TRANSFER- ZEPHYR LTD"),
                (222.0, 280.0, "TRANSFER FROM 0000000 / ZEPHYR LTD"),
                (460.0, 500.0, "1000.00"),
                (510.0, 570.0, "96000.00"),
            ],
        ),
        (127.0, &[(2.0, 25.0, "2026")]),
    ])
}

pub fn hdfc_header_row() -> &'static [(f64, f64, &'static str)] {
    HDFC_HEADER_ROW
}

pub fn hdfc_account_row() -> &'static [(f64, f64, &'static str)] {
    HDFC_ACCOUNT_ROW
}

pub fn hdfc_footer() -> &'static [(f64, f64, &'static str)] {
    HDFC_FOOTER
}
