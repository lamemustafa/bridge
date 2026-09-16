//! Geometry, profiles and account binding, ported from
//! `scripts/bank_statement_import.test.py`.

mod common;

use bridge_bank_statement::bank::{Bank, UNRESOLVED};
use bridge_bank_statement::geometry::{dewrap, lines, matches, WRAP_TOLERANCE};
use bridge_bank_statement::parse::{parse_pages, require_account_match, Row};
use common::*;

fn parties(rows: &[Row], bank: Bank) -> Vec<String> {
    rows.iter().map(|row| bank.party(row)).collect()
}

fn references(rows: &[Row], bank: Bank) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let (mode, value) = bank.reference(row);
            format!("{mode}:{value}")
        })
        .collect()
}

fn narrations(rows: &[Row]) -> Vec<String> {
    rows.iter().map(|row| row.get("narr").to_string()).collect()
}

#[test]
fn parse_real_hdfc_capture() {
    let bank = Bank::Hdfc;
    let pages = capture("hdfc-bbox-capture.xml");
    let rows = parse_pages(&pages, bank);
    assert_eq!(rows.len(), 14);

    let unresolved: Vec<&str> = rows
        .iter()
        .filter(|row| bank.party(row) == UNRESOLVED)
        .map(|row| row.get("narr"))
        .collect();
    assert!(unresolved.is_empty(), "{unresolved:?}");
    // digests pinned by the Python suite over the same capture
    assert_eq!(
        digest(parties(&rows, bank)),
        "f079dbf8cc126ee0",
        "{:?}",
        parties(&rows, bank)
    );
    assert_eq!(
        digest(references(&rows, bank)),
        "25ddb159d0c3e2b7",
        "{:?}",
        references(&rows, bank)
    );

    // a narration wrapped across four printed lines, rejoined in full
    assert_eq!(
        rows[4].get("narr"),
        "UPI-ZZZZW ZZZZZK ZZZZZZW-ZZZZZB.ZZZZZK@K ZQ-ZZZZ1111114-111111111113-ZZZZZZV FROMZZZZG"
    );
    assert_eq!(bank.party(&rows[4]), "ZZZZW ZZZZZK ZZZZZZW");
    // ... and its 12-digit reference survived the wrap intact
    assert_eq!(
        bank.reference(&rows[4]),
        ("UPI".to_string(), "111111111113".to_string())
    );
    assert_eq!(
        digest(narrations(&rows)),
        "4c1a76b6a6f582c5",
        "{:?}",
        narrations(&rows)
    );

    assert_eq!(rows[0].get("ref"), "1111111111111111");
    assert_eq!(rows[0].get("vdt"), "04/08/26");
    for (index, row) in rows.iter().enumerate() {
        assert_ne!(
            row.get("dr").is_empty(),
            row.get("cr").is_empty(),
            "row {}",
            index + 1
        );
        assert!(!row.get("bal").is_empty(), "row {}", index + 1);
        assert_eq!(bank.parse_date(row.get("date")).unwrap().year, 2026);
    }

    // page 2 ends at STATEMENT SUMMARY and page 3 is never read
    assert_eq!(pages.len(), 3);
    assert!(rows.iter().all(|row| !row.get("narr").contains("SUMMARY")));
    assert!(!rows.last().unwrap().get("narr").ends_with(' '));
    assert_eq!(
        parse_pages(&pages[..2], bank),
        rows,
        "page 3 must contribute nothing"
    );

    // the account number is bound from its own header line, not from the table
    require_account_match(&pages, bank, "HDFC CA xx1111").unwrap();
    // 1112 is the MICR tail; 1113-1115 occur in table references; 9876 is absent
    for wrong in ["xx1112", "xx1113", "xx1114", "xx1115", "xx9876"] {
        refuses(
            require_account_match(&pages, bank, &format!("HDFC CA {wrong}")),
            "account_not_in_statement",
        );
    }
}

#[test]
fn real_hdfc_capture_binds_the_account_no_geometry_only() {
    let bank = Bank::Hdfc;
    let pages = capture("hdfc-bbox-capture.xml");
    type Placed = (String, String, String, String, String);
    let selected: Vec<Vec<Placed>> = lines(&pages[0])
        .iter()
        .filter(|line| matches(line, bank.account_anchors()))
        .map(|line| {
            line.words
                .iter()
                .filter(|word| word.text == "Account" || word.text == "No")
                .map(|word| {
                    (
                        format!("{:.3}", word.x0),
                        format!("{:.3}", word.y0),
                        format!("{:.3}", word.x1),
                        format!("{:.3}", word.y1),
                        word.text.clone(),
                    )
                })
                .collect()
        })
        .collect();
    let expected = vec![vec![
        (
            "340.157".to_string(),
            "149.001".to_string(),
            "367.261".to_string(),
            "156.201".to_string(),
            "Account".to_string(),
        ),
        (
            "369.261".to_string(),
            "149.001".to_string(),
            "379.037".to_string(),
            "156.201".to_string(),
            "No".to_string(),
        ),
    ]];
    assert_eq!(selected, expected);
    assert_eq!(
        require_account_match(&pages, bank, "xx1111111").unwrap(),
        "1".repeat(14)
    );
}

#[test]
fn parse_real_sbi_capture() {
    let bank = Bank::Sbi;
    let pages = capture("sbi-bbox-capture.xml");
    let rows = parse_pages(&pages, bank);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|row| bank.party(row) != UNRESOLVED));
    assert_eq!(digest(parties(&rows, bank)), "9182a433650d104c");
    assert_eq!(digest(references(&rows, bank)), "52f8f2b555195dae");
    for row in &rows {
        let date = bank.parse_date(row.get("date")).unwrap();
        assert_eq!((date.year, date.month, date.day), (2026, 7, 1));
        for furniture in ["Description", "No./Cheque", "Balance"] {
            assert!(!row.get("narr_spaced").contains(furniture), "{furniture}");
        }
    }
    let (mode, value) = bank.reference(&rows[0]);
    assert_eq!(mode, "UPI");
    assert!(value.starts_with("111111111111"));
    assert_eq!(digest(narrations(&rows)), "949f0d93d6c532ab");
    assert_eq!(rows[0].get("ref"), "TRANSFER TO 1111111111203 /");
    require_account_match(&pages, bank, "SBI CA xx1111").unwrap();
    refuses(
        require_account_match(&pages, bank, "SBI CA xx9876"),
        "account_not_in_statement",
    );
}

#[test]
fn parse_hdfc_page() {
    let bank = Bank::Hdfc;
    let rows = parse_pages(&[hdfc_page()], bank);
    assert_eq!(rows.len(), 2, "the post-footer row is not a row");

    let first = &rows[0];
    assert_eq!(
        first.get("narr"),
        "UPI-NORTH-WIND-north@zzz-ZZZZ0001-123456789012-PAYMENT"
    );
    assert_eq!(first.get("date"), "01/08/26");
    assert_eq!(first.get("dr"), "");
    assert_eq!(first.get("cr"), "10000.00");
    assert_eq!(first.get("bal"), "11000.00");
    assert_eq!(bank.party(first), "NORTH-WIND");
    assert_eq!(
        bank.reference(first),
        ("UPI".to_string(), "123456789012".to_string())
    );

    let second = &rows[1];
    assert_eq!(
        second.get("narr"),
        "NEFT DR-ZZZZ0000001-ACME EXPORTS-MUM-ZZZZZ00000000000-BB"
    );
    assert_eq!(second.get("dr"), "2500.50");
    assert_eq!(second.get("cr"), "");
    assert_eq!(bank.party(second), "ACME EXPORTS");
    // row-scoped: the stray continuation fragment did not reach the reference
    assert_eq!(first.get("ref"), "0000123456789012");
}

#[test]
fn parse_sbi_page() {
    let bank = Bank::Sbi;
    let rows = parse_pages(&[sbi_page(), sbi_page_2()], bank);
    assert_eq!(rows.len(), 2);
    let row = &rows[0];
    assert!(!row.get("narr").contains("Description") && !row.get("bal").contains("Balance"));
    let date = bank.parse_date(row.get("date")).unwrap();
    assert_eq!((date.year, date.month, date.day), (2026, 8, 1));
    assert_eq!(row.get("dr"), "5000.00");
    assert_eq!(row.get("bal"), "95000.00");
    assert!(row.get("narr").contains("ZZZZ111111111"));
    assert!(row.get("narr_spaced").contains("NORTH WIND TRADERS"));
    assert_eq!(bank.party(row), "NORTH WIND TRADERS");
    for furniture in ["Description", "Branch", "Credit", "Balance"] {
        assert!(!row.get("narr_spaced").contains(furniture), "{furniture}");
    }
    let second = bank.parse_date(rows[1].get("date")).unwrap();
    assert_eq!((second.month, second.day), (8, 2));
    assert_eq!(rows[1].get("cr"), "1000.00");
}

#[test]
fn account_binding() {
    let bank = Bank::Hdfc;
    let pages = [hdfc_page()];
    require_account_match(&pages, bank, "HDFC CA xx1234").unwrap();
    refuses(
        require_account_match(&pages, bank, "HDFC CA xx9876"),
        "account_not_in_statement",
    );
    refuses(
        require_account_match(&pages, bank, "HDFC CA"),
        "unbindable_account",
    );
    // 9012 ends the UPI reference on row 1: a table value must not bind
    refuses(
        require_account_match(&pages, bank, "HDFC CA xx9012"),
        "account_not_in_statement",
    );
    // nor may the other header value
    refuses(
        require_account_match(&pages, bank, "HDFC CA xx4230"),
        "account_not_in_statement",
    );
    refuses(
        require_account_match(
            &[page(&[(10.0, &[(2.0, 60.0, "nothing")])])],
            bank,
            "HDFC CA xx1234",
        ),
        "no_account_number_line",
    );
}

#[test]
fn account_identity_comes_from_the_statement() {
    let bank = Bank::Hdfc;
    let pages = capture("hdfc-bbox-capture.xml");
    let numbers: std::collections::BTreeSet<String> =
        ["HDFC CA xx1111", "xx11111", "1111111111111"]
            .iter()
            .map(|tail| require_account_match(&pages, bank, tail).unwrap())
            .collect();
    assert_eq!(numbers.into_iter().collect::<Vec<_>>(), ["11111111111111"]);

    refuses(
        require_account_match(&pages, bank, "xx55"),
        "unbindable_account",
    );
    let two_numbers = page(&[
        (
            52.0,
            &[
                (340.0, 380.0, "Account"),
                (382.0, 396.0, "No"),
                (397.0, 400.0, ":"),
                (403.0, 470.0, "00000000001234"),
                (474.0, 520.0, "99001234"),
            ],
        ),
        (100.0, &[(5.0, 30.0, "Date"), (72.0, 120.0, "Narration")]),
    ]);
    refuses(
        require_account_match(std::slice::from_ref(&two_numbers), bank, "xx1234"),
        "ambiguous_account_match",
    );
    assert_eq!(
        require_account_match(&[two_numbers], bank, "xx0000001234").unwrap(),
        "00000000001234"
    );
}

fn hdfc_party(narration: &str) -> String {
    Bank::Hdfc.party(&row(&[("narr", narration), ("narr_spaced", narration)]))
}

#[test]
fn hyphenated_counterparties_survive_every_narration_shape() {
    for (narration, expected) in [
        (
            "UPI-ACME-INDUSTRIES-acme@ok-HDFC0001-123456789012-P",
            "ACME-INDUSTRIES",
        ),
        ("UPI-XXXXXX0000-ZZZZ0000001-888888888888-PAYMENT", "UNNAMED"),
        (
            "IMPS-999999999999-ACME INDUSTRIES-ZZZZ-XXXXXXXX0000-US",
            "ACME INDUSTRIES",
        ),
        (
            "IMPS-999999999999-ACME-INDUSTRIES-ZZZZ-XXXXXXXX0000-US",
            "ACME-INDUSTRIES",
        ),
        (
            "NEFT DR-ZZZZ0000000-ACME INTL-MUM-ZZZZZ00000000000-BB",
            "ACME INTL",
        ),
        (
            "NEFT DR-ZZZZ0000000-ACME-INTL-MUM-ZZZZZ00000000000-BB",
            "ACME-INTL",
        ),
        (
            "NEFT CR-ZZZZ0000000-INTERNATIONAL-MUM-ZZZZZ00000000000-B",
            "INTERNATIONAL",
        ),
        ("NEFT DR-ZZZZ0ZZZZZZ-GST-MUM-ZZZ ZZ00000000000-BB0", "GST"),
        ("UPI-NOSTRUCTURE-HERE", "UNRESOLVED"),
        ("IMPS-1-WEIRD", "UNRESOLVED"),
        ("NEFT DR-ONLY-TWO", "UNRESOLVED"),
    ] {
        // the Python test passes only `narr`; HDFC reads `narr_spaced` when set
        let only_narr = Bank::Hdfc.party(&row(&[("narr", narration)]));
        assert_eq!(only_narr, expected, "{narration}");
    }
}

#[test]
fn dewrap_joins_hard_wraps_without_a_space() {
    let edge = 100.0;
    let fragments = |pairs: &[(&str, f64)]| -> Vec<(String, f64)> {
        pairs
            .iter()
            .map(|(text, x)| (text.to_string(), *x))
            .collect()
    };
    assert_eq!(
        dewrap(
            &fragments(&[("UPI/DR/1234", 99.5), ("56789012/X", 60.0)]),
            edge,
            WRAP_TOLERANCE
        ),
        "UPI/DR/123456789012/X"
    );
    assert_eq!(
        dewrap(
            &fragments(&[("NORTH", 40.0), ("WIND", 45.0)]),
            edge,
            WRAP_TOLERANCE
        ),
        "NORTH WIND"
    );
    assert_eq!(dewrap(&[], edge, WRAP_TOLERANCE), "");
}

#[test]
fn a_transaction_naming_the_bank_is_not_the_footer() {
    let bank = Bank::Hdfc;
    let tricky = page(&[
        (
            100.0,
            &[
                (5.0, 30.0, "Date"),
                (72.0, 120.0, "Narration"),
                (282.0, 340.0, "Chq./Ref.No."),
                (402.0, 452.0, "Withdrawal"),
                (562.0, 600.0, "Closing"),
            ],
        ),
        (
            120.0,
            &[
                (2.0, 60.0, "01/08/26"),
                (72.0, 110.0, "TPT-"),
                (112.0, 150.0, "HDFC"),
                (152.0, 185.0, "BANK"),
                (187.0, 230.0, "LIMITED"),
                (282.0, 350.0, "0000000000000001"),
                (402.0, 460.0, "10.00"),
                (562.0, 620.0, "990.00"),
            ],
        ),
        (
            140.0,
            &[
                (2.0, 60.0, "02/08/26"),
                (72.0, 200.0, "UPI-BETA-b@z-ZZZZ1-222222222222-P"),
                (282.0, 350.0, "0000000000000002"),
                (402.0, 460.0, "20.00"),
                (562.0, 620.0, "970.00"),
            ],
        ),
        (170.0, hdfc_footer()),
        (
            190.0,
            &[
                (2.0, 60.0, "03/08/26"),
                (72.0, 200.0, "UPI-GHOST-g@z-ZZZZ1-333333333333-X"),
                (402.0, 460.0, "1.00"),
                (562.0, 620.0, "969.00"),
            ],
        ),
    ]);
    let rows = parse_pages(&[tricky], bank);
    assert_eq!(rows.len(), 2);
    assert!(rows[0].get("narr_spaced").contains("HDFC BANK LIMITED"));
    assert!(rows.iter().all(|row| !row.get("narr").contains("GHOST")));
}

#[test]
fn ach_party_ends_at_the_final_bank_reference() {
    assert_eq!(
        hdfc_party("ACH D- TP ACH STUDIO-54 INDUSTRIES-1234567890"),
        "STUDIO-54 INDUSTRIES"
    );
    assert_eq!(
        hdfc_party("ACH D- TP ACH ACME TRADERS-1234567890"),
        "ACME TRADERS"
    );
    assert!(!hdfc_party("ACH D- TP ACH UNIT-7 METALS-1234567890").contains("1234567890"));
    assert_eq!(
        hdfc_party("ACH D- TP ACH ACME TRADERS-12345 67890"),
        "ACME TRADERS"
    );
    assert_eq!(
        hdfc_party("ACH D- TP ACH STUDIO-54 INDUSTRIES-12345 67890"),
        "STUDIO-54 INDUSTRIES"
    );
    assert_eq!(
        hdfc_party("ACH D- TP ACH UNIT-7 METALS-12 345 67890"),
        "UNIT-7 METALS"
    );
}

#[test]
fn a_wrapped_ach_reference_survives_the_parser() {
    let one = |first: &str, continuation: &str| {
        let rows = parse_pages(&[ach_wrap_page(first, continuation)], Bank::Hdfc);
        assert_eq!(rows.len(), 1);
        rows.into_iter().next().unwrap()
    };
    // 1. the wrap falls inside the reference
    let row = one("ACH D- TP ACH ACME TRADERS-12345", "67890");
    assert_eq!(
        row.get("narr_spaced"),
        "ACH D- TP ACH ACME TRADERS-12345 67890"
    );
    assert_eq!(Bank::Hdfc.party(&row), "ACME TRADERS");
    assert_eq!(row.get("narr"), "ACH D- TP ACH ACME TRADERS-1234567890");
    // 2. the wrap falls between two words of the name; `narr` welds them
    let row = one("ACH D- TP ACH NORTHWIND", "TRADERS-1234567890");
    assert_eq!(row.get("narr"), "ACH D- TP ACH NORTHWINDTRADERS-1234567890");
    assert_eq!(Bank::Hdfc.party(&row), "NORTHWIND TRADERS");
    // 3. recorded residual: a wrap inside one word leaves a space
    let row = one("ACH D- TP ACH NORTHWIND TRAD", "ERS-1234567890");
    assert_eq!(Bank::Hdfc.party(&row), "NORTHWIND TRAD ERS");
}

#[test]
fn an_ach_reference_must_be_reference_shaped() {
    for tail in [
        "STUDIO-54",
        "UNIT-7",
        "SHOP-2024",
        "STUDIO-5 4",
        "SHOP-1 2 3",
        "ACME-400 001",
        "ACME-400001",
        "TRADERS-560 034",
        "CORP-110001",
    ] {
        assert_eq!(
            hdfc_party(&format!("ACH D- TP ACH {tail}")),
            "UNRESOLVED",
            "{tail}"
        );
    }
    for digits in ["123456789", "12345678901", "123456789012"] {
        assert_eq!(
            hdfc_party(&format!("ACH D- TP ACH ACME-{digits}")),
            "UNRESOLVED",
            "{digits}"
        );
    }
    for (tail, expected) in [
        ("STUDIO-54 INDUSTRIES-1234567890", "STUDIO-54 INDUSTRIES"),
        ("ACME TRADERS-12345 67890", "ACME TRADERS"),
        ("ACME TRADERS- 1234567890", "ACME TRADERS"),
        ("STUDIO-54 INDUSTRIES- 1234567890", "STUDIO-54 INDUSTRIES"),
        ("UNIT-7 METALS-12 345 67890", "UNIT-7 METALS"),
    ] {
        assert_eq!(
            hdfc_party(&format!("ACH D- TP ACH {tail}")),
            expected,
            "{tail}"
        );
    }
}

#[test]
fn impossible_dates_do_not_parse() {
    assert!(Bank::Hdfc.parse_date("31/02/26").is_none());
    assert!(Bank::Hdfc.parse_date("29/02/24").is_some());
    assert!(
        Bank::Sbi.parse_date("1 aug2026").is_some(),
        "%b is case-insensitive"
    );
    assert!(Bank::Sbi.parse_date("31 Sep 2026").is_none());
    // leftover characters are refused, not ignored
    assert!(Bank::Hdfc.parse_date("01/08/266").is_none());
    // one deliberate divergence: ASCII digits only
    assert!(Bank::Hdfc.parse_date("٠١/٠٨/٢٦").is_none());
}
