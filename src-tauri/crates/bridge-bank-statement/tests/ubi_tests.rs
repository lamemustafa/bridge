//! The Union Bank of India layout: one printed line per transaction.
//!
//! There is no Python reference for this profile. The shapes were described
//! from the text layer of one real statement (68 rows) without any of its
//! values; every line here is synthetic.

mod common;

use bridge_bank_statement::bank::{Bank, UNRESOLVED};
use bridge_bank_statement::date::{parse_day_month_year_hyphenated, Date};
use bridge_bank_statement::geometry::{Page, Word};
use bridge_bank_statement::mapping::{Mapping, MappingRow};
use bridge_bank_statement::money::Controls;
use bridge_bank_statement::parse::{parse_statement, require_account_match, Row};
use bridge_bank_statement::pipeline::{prepare, StatementRequest};
use bridge_bank_statement::proposals::format_amount;
use common::*;

const HEADER: &str = "Date Transaction Id Remarks Amount( ) Balance( )";

/// A page from whole printed lines, one word per space-separated token, each
/// line 12pt below the last.
fn text_page(printed: &[&str]) -> Page {
    let mut words = Vec::new();
    for (index, line) in printed.iter().enumerate() {
        let y = 40.0 + 12.0 * index as f64;
        let mut x = 10.0;
        for token in line.split(' ').filter(|token| !token.is_empty()) {
            let width = 4.2 * token.chars().count() as f64;
            words.push(Word::new(x, y, x + width, y + 9.0, token));
            x += width + 4.2;
        }
    }
    words
}

fn statement(pages: &[&[&str]]) -> Vec<Page> {
    let count = pages.len();
    pages
        .iter()
        .enumerate()
        .map(|(index, lines)| {
            let footer = format!("Page {} of {count}", index + 1);
            let mut all: Vec<&str> = lines.to_vec();
            all.push(&footer);
            text_page(&all)
        })
        .collect()
}

const PAGE_ONE: &[&str] = &[
    "SYNTHETIC STATEMENT - NOT A REAL ACCOUNT",
    "Account Number 000000000007788",
    "Statement Period 01-08-2026 to 31-08-2026",
    HEADER,
    "01-08-2026 A12345678 UPIAB/612345678901/CR/NORTHWIND TRADERS/ZZZZ/nw@okzz 1,500.00(Cr) 11500.00(Cr)",
    "02-08-2026 A1234567 NEFT:BLUE RIVER CO ZZZZN12345678901 250.50(Dr) 11249.50(Cr)",
    "03-08-2026 A123456 BY CASH 750.50(Cr) 12000.00(Cr)",
];

const PAGE_TWO: &[&str] = &[
    HEADER,
    "04-08-2026 A12345 IMPSAB/712345678901/GREEN FIELD LTD/9000000001 13000.00(Dr) 1000.00(Dr)",
    "05-08-2026 A9AA99999 MOBFT/SOME THREE WORDS/812345678901 1000.00(Cr) 0.00(Cr)",
];

fn rows() -> Vec<Row> {
    parse_statement(&statement(&[PAGE_ONE, PAGE_TWO]), Bank::Ubi).unwrap()
}

#[test]
fn single_line_rows_parse_with_the_side_from_the_suffix() {
    let rows = rows();
    let cells: Vec<[&str; 5]> = rows
        .iter()
        .map(|row| {
            [
                row.get("date"),
                row.get("ref"),
                row.get("dr"),
                row.get("cr"),
                row.get("bal"),
            ]
        })
        .collect();
    assert_eq!(
        cells,
        [
            ["01-08-2026", "A12345678", "", "1500.00", "11500.00"],
            ["02-08-2026", "A1234567", "250.50", "", "11249.50"],
            ["03-08-2026", "A123456", "", "750.50", "12000.00"],
            // an overdrawn (Dr) balance is negative
            ["04-08-2026", "A12345", "13000.00", "", "-1000.00"],
            ["05-08-2026", "A9AA99999", "", "1000.00", "0.00"],
        ]
    );
    assert_eq!(rows[2].get("narr"), "BY CASH");
    assert_eq!(rows[2].get("narr_spaced"), "BY CASH");
}

#[test]
fn union_bank_parties_and_references() {
    let rows = rows();
    let parties: Vec<String> = rows.iter().map(|row| Bank::Ubi.party(row)).collect();
    assert_eq!(
        parties,
        [
            "NORTHWIND TRADERS",
            "BLUE RIVER CO",
            "CASH DEPOSIT",
            "GREEN FIELD LTD",
            // MOBFT's text fields have no settled role: suspense, not a guess
            UNRESOLVED,
        ]
    );
    let references: Vec<(String, String)> =
        rows.iter().map(|row| Bank::Ubi.reference(row)).collect();
    let expected = [
        ("UPI", "612345678901"),
        ("NEFT", "ZZZZN12345678901"),
        ("CASH", "A123456"),
        ("IMPS", "712345678901"),
        ("MOBFT", "812345678901"),
    ]
    .map(|(mode, value)| (mode.to_string(), value.to_string()));
    assert_eq!(references, expected);
}

fn party_of(remarks: &str) -> String {
    Bank::Ubi.party(&row(&[("narr", remarks)]))
}

#[test]
fn a_union_bank_party_is_named_only_where_the_shape_settles_it() {
    assert_eq!(
        party_of("IMPSAR/612345678901/NORTH STAR/00000000000001"),
        "NORTH STAR"
    );
    assert_eq!(party_of("CLG/ACME EXPORTS LTD"), "ACME EXPORTS LTD");
    assert_eq!(
        party_of("ANN.FEE0000000000000000DATE OF ISSUANCE01-01-2026A"),
        "CARD ANNUAL FEE"
    );
    for unsettled in [
        // a debit UPI shape was never observed
        "UPIAB/612345678901/DR/NORTHWIND TRADERS/ZZZZ/nw@okzz",
        // without a reference-shaped tail the name has no end
        "NEFT:BLUE RIVER CO",
        "NEFT:BLUE RIVER CO LIMITED",
        // IMPSAR's tail is 14 or 11 digits, IMPSAB's 10
        "IMPSAR/612345678901/NORTH STAR/9000000001",
        "IMPSAB/612345678901/NORTH STAR/00000000000001",
        "MOBFT/SOME THREE WORDS/TOKEN/812345678901",
        "CLG/ACME/EXTRA",
        "BY CASH DEPOSIT",
        "SOMETHING ELSE",
    ] {
        assert_eq!(party_of(unsettled), UNRESOLVED, "{unsettled}");
    }
}

#[test]
fn a_date_led_line_that_is_not_a_row_is_refused() {
    for malformed in [
        // an amount without its side
        "06-08-2026 A12345678 BY CASH 10.00 12010.00(Cr)",
        // a side that is not Cr or Dr
        "06-08-2026 A12345678 BY CASH 10.00(CR) 12010.00(Cr)",
        // three decimal places
        "06-08-2026 A12345678 BY CASH 10.000(Cr) 12010.00(Cr)",
        // no remarks
        "06-08-2026 A12345678 10.00(Cr) 12010.00(Cr)",
    ] {
        let page: Vec<&str> = [HEADER, PAGE_ONE[4], malformed].to_vec();
        let refusal = refuses(
            parse_statement(&statement(&[&page]), Bank::Ubi),
            "malformed_row",
        );
        assert_eq!(refusal.row, Some(2), "{malformed}");
    }
}

#[test]
fn an_unrecognised_line_between_rows_is_refused_and_after_them_is_not() {
    // a wrapped remark: the tail of row 1's cell printed on its own line
    let wrapped = [HEADER, PAGE_ONE[4], "PAYMENT FOR INVOICE", PAGE_ONE[5]];
    let refusal = refuses(
        parse_statement(&statement(&[&wrapped]), Bank::Ubi),
        "unexpected_line_in_table",
    );
    assert_eq!(refusal.row, Some(2));

    // also across a page break
    let first = [HEADER, PAGE_ONE[4], "PAYMENT FOR INVOICE"];
    let second = [HEADER, PAGE_ONE[5]];
    refuses(
        parse_statement(&statement(&[&first, &second]), Bank::Ubi),
        "unexpected_line_in_table",
    );

    // the column header printed again inside a page is furniture, not a stray
    let repeated = [HEADER, PAGE_ONE[4], HEADER, PAGE_ONE[5]];
    assert_eq!(
        parse_statement(&statement(&[&repeated]), Bank::Ubi)
            .unwrap()
            .len(),
        2
    );

    // a row whose remarks carry the header's words is still a row
    let lookalike = [
        HEADER,
        "01-08-2026 A12345678 Remarks Transaction Date Id 1,500.00(Cr) 11500.00(Cr)",
    ];
    assert_eq!(
        parse_statement(&statement(&[&lookalike]), Bank::Ubi)
            .unwrap()
            .len(),
        1
    );

    // a non-row line carrying the header's words out of order is not the header
    let scrambled = [
        HEADER,
        PAGE_ONE[4],
        "Remarks Date Transaction Id NOTE",
        PAGE_ONE[5],
    ];
    refuses(
        parse_statement(&statement(&[&scrambled]), Bank::Ubi),
        "unexpected_line_in_table",
    );

    // ... and a row whose remarks carry a page footer's words is still a row
    let footer_lookalike = [
        HEADER,
        "01-08-2026 A12345678 INVOICE Page 1 of 1 CHARGES 1,500.00(Cr) 11500.00(Cr)",
    ];
    assert_eq!(
        parse_statement(&statement(&[&footer_lookalike]), Bank::Ubi)
            .unwrap()
            .len(),
        1
    );

    let trailing = [HEADER, PAGE_ONE[4], "** END OF STATEMENT **"];
    assert_eq!(
        parse_statement(&statement(&[&trailing]), Bank::Ubi)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn every_page_must_print_its_own_page_footer() {
    let complete = statement(&[PAGE_ONE, PAGE_TWO]);
    assert_eq!(parse_statement(&complete, Bank::Ubi).unwrap().len(), 5);

    // a missing page: the remaining pages still say "of 3"
    let mut missing = statement(&[PAGE_ONE, PAGE_TWO, &[HEADER]]);
    missing.remove(1);
    refuses(
        parse_statement(&missing, Bank::Ubi),
        "page_sequence_unproven",
    );

    // pages out of order
    let mut swapped = statement(&[PAGE_ONE, PAGE_TWO]);
    swapped.swap(0, 1);
    refuses(
        parse_statement(&swapped, Bank::Ubi),
        "page_sequence_unproven",
    );

    // a row's remarks do not stand in for the missing footer
    refuses(
        parse_statement(
            &[text_page(&[
                HEADER,
                "01-08-2026 A12345678 INVOICE Page 1 of 1 CHARGES 1,500.00(Cr) 11500.00(Cr)",
            ])],
            Bank::Ubi,
        ),
        "page_sequence_unproven",
    );

    // no footer at all
    refuses(
        parse_statement(&[text_page(PAGE_ONE)], Bank::Ubi),
        "page_sequence_unproven",
    );
}

#[test]
fn only_the_account_number_line_binds() {
    let pages = statement(&[&[
        "Account Number 000000000007788",
        "Savings Account No 9**** *1234",
        "CIF ID 000001234",
        HEADER,
    ]]);
    assert_eq!(
        require_account_match(&pages, Bank::Ubi, "UBI SB xx7788").unwrap(),
        "000000000007788"
    );
    refuses(
        require_account_match(&pages, Bank::Ubi, "UBI SB xx1234"),
        "account_not_in_statement",
    );
}

#[test]
fn hyphenated_dates_are_strict() {
    assert_eq!(
        parse_day_month_year_hyphenated("29-02-2028"),
        Date::new(2028, 2, 29)
    );
    for bad in [
        "29-02-2026",
        "31-04-2026",
        "1-08-2026",
        "01-8-2026",
        "01-08-26",
        "01/08/2026",
        "01-08-2026 ",
        "00-08-2026",
    ] {
        assert_eq!(parse_day_month_year_hyphenated(bad), None, "{bad}");
    }
}

fn request<'a>(bank: Bank, controls: &'a Controls, mapping: &'a Mapping) -> StatementRequest<'a> {
    StatementRequest {
        bank,
        account_label: "UBI SB xx7788",
        controls,
        bank_ledger: "Union Bank",
        suspense_ledger: "Suspense",
        mapping,
        date_from: None,
        date_to: None,
    }
}

fn no_mapping() -> Mapping {
    Mapping::from_rows(std::iter::empty::<MappingRow>()).unwrap()
}

#[test]
fn a_union_bank_statement_proves_itself_without_printed_totals() {
    let pages = statement(&[PAGE_ONE, PAGE_TWO]);
    let mapping = no_mapping();
    let controls = Controls::parse_optional("10,000.00", "0.00", None, None).unwrap();
    let parsed = prepare(&pages, &request(Bank::Ubi, &controls, &mapping)).unwrap();
    assert_eq!(parsed.statement_rows, 5);
    assert_eq!(format_amount(&parsed.totals.debits), "13250.50");
    assert_eq!(format_amount(&parsed.totals.credits), "3250.50");
    assert_eq!(parsed.build.proposals.len(), 5);

    // supplied totals are still checked
    let wrong = Controls::parse("10,000.00", "0.00", "13,250.50", "3,250.51").unwrap();
    refuses(
        prepare(&pages, &request(Bank::Ubi, &wrong, &mapping)),
        "control_total_mismatch",
    );
    // and the opening balance still ties the first row
    let off = Controls::parse_optional("10,000.01", "0.00", None, None).unwrap();
    refuses(
        prepare(&pages, &request(Bank::Ubi, &off, &mapping)),
        "balance_chain_broken",
    );
}

#[test]
fn a_layout_that_prints_totals_cannot_skip_them() {
    let mapping = no_mapping();
    let controls = Controls::parse_optional("10,000.00", "0.00", None, None).unwrap();
    for bank in [Bank::Sbi, Bank::Hdfc] {
        refuses(
            prepare(&[], &request(bank, &controls, &mapping)),
            "control_totals_required",
        );
    }
    refuses(
        Controls::parse_optional("1.00", "1.00", Some("1.00"), None),
        "control_totals_incomplete",
    );
    refuses(
        Controls::parse_optional("1.00", "1.00", None, Some("1.00")),
        "control_totals_incomplete",
    );
}
