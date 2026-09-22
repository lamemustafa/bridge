use super::*;
use crate::native_outstandings::{parse_native_ledger_snapshot, NativeOutstandingsError};

fn decode(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn single_currency_book() -> String {
    decode(include_bytes!(
        "../../tests/fixtures/ledgers_currency_single_live.utf16le.xml"
    ))
}

/// `ledgers_currency_forex_live`'s ledgers and `CURRENCYNAME`s, in read
/// order. Two `$` rows close composite, which the snapshot parser refuses
/// before classification today, so the pairs are listed here rather than
/// parsed.
const FOREX_ROWS: [(&str, &str); 10] = [
    ("BRIDGE FX DEBTOR A", "$"),
    ("BRIDGE INR DEBTOR A", "I₹"),
    ("Cash", "I₹"),
    ("FX Party 01", "I₹"),
    ("FX Party 02", "I₹"),
    ("FX Party 03", "I₹"),
    ("FX Sales", "I₹"),
    ("FX USD Debtor 01", "$"),
    ("FX USD Debtor 02", "$"),
    ("Profit & Loss A/c", "I₹"),
];

fn forex_rows() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    FOREX_ROWS
        .iter()
        .map(|(ledger, currency)| (*ledger, Some(*currency)))
}

#[test]
fn a_single_currency_book_parses_every_ledger_currency_as_its_one_master() {
    let rows = parse_native_ledger_snapshot(&single_currency_book()).unwrap();
    assert_eq!(rows.len(), 13);
    assert!(rows
        .iter()
        .all(|row| row.currency_name.as_deref() == Some("Rs.")));
    let classified = classify_ledger_currencies(
        "Rs.",
        1,
        rows.iter()
            .map(|row| (row.name.as_str(), row.currency_name.as_deref())),
    )
    .unwrap();
    assert_eq!(classified, LedgerCurrencies::default());
}

#[test]
fn the_forex_book_names_its_three_dollar_ledgers_foreign() {
    let classified = classify_ledger_currencies("I₹", 2, forex_rows()).unwrap();
    assert_eq!(
        classified.foreign,
        ["BRIDGE FX DEBTOR A", "FX USD Debtor 01", "FX USD Debtor 02"]
            .map(|ledger| ForeignCurrencyLedger {
                ledger: ledger.to_string(),
                currency: "$".to_string(),
            })
            .to_vec()
    );
    assert_eq!(classified.unobserved, 0);
}

#[test]
fn several_masters_refuse_a_ledger_with_no_currency() {
    let rows = forex_rows().map(|(ledger, currency)| {
        (
            ledger,
            (ledger != "FX USD Debtor 01").then_some(currency).flatten(),
        )
    });
    assert_eq!(
        classify_ledger_currencies("I₹", 2, rows),
        Err(LedgerCurrencyRefusal::Unobserved {
            ledger: "FX USD Debtor 01".to_string()
        })
    );
    // An empty element is no currency either.
    let rows = forex_rows()
        .map(|(ledger, currency)| (ledger, if ledger == "Cash" { Some("") } else { currency }));
    assert_eq!(
        classify_ledger_currencies("I₹", 2, rows)
            .unwrap_err()
            .code(),
        "ledger_currency_unobserved"
    );
}

#[test]
fn several_masters_with_no_ledger_in_the_base_refuse_rather_than_exclude_all() {
    // A base NAME that matches no row, e.g. read in a different prefix form:
    // every ledger would otherwise be "foreign" and the read merely partial.
    for base in ["₹", "I₹ ", "Rs."] {
        let refused = classify_ledger_currencies(base, 2, forex_rows()).unwrap_err();
        assert_eq!(
            refused,
            LedgerCurrencyRefusal::BaseUnmatched { ledger: None },
            "{base:?}"
        );
        assert_eq!(refused.code(), "ledger_currency_base_unmatched");
    }
}

#[test]
fn one_master_takes_a_missing_currency_as_the_base_and_counts_it() {
    let rows = [
        ("Cash", None),
        ("Sales", Some("")),
        ("Sharma Traders", Some("Rs.")),
    ];
    let classified = classify_ledger_currencies("Rs.", 1, rows).unwrap();
    assert!(classified.foreign.is_empty());
    assert_eq!(classified.unobserved, 2);
}

#[test]
fn one_master_refuses_a_ledger_that_names_another_currency() {
    let rows = [("Cash", Some("Rs.")), ("Sales", Some("$"))];
    assert_eq!(
        classify_ledger_currencies("Rs.", 1, rows),
        Err(LedgerCurrencyRefusal::BaseUnmatched {
            ledger: Some("Sales".to_string())
        })
    );
}

#[test]
fn no_master_or_an_empty_base_name_refuses() {
    for (base, count) in [("Rs.", 0), ("", 1), ("", 2)] {
        assert_eq!(
            classify_ledger_currencies(base, count, [("Cash", Some("Rs."))]),
            Err(LedgerCurrencyRefusal::BaseUnmatched { ledger: None }),
            "{base:?} {count}"
        );
    }
}

#[test]
fn the_snapshot_reads_an_empty_currency_as_none_and_refuses_a_duplicate() {
    let book = single_currency_book();
    let tag = "<CURRENCYNAME TYPE=\"String\">Rs.</CURRENCYNAME>";
    let first = book.find(tag).expect("the capture carries the element");
    let mut empty = book.clone();
    empty.replace_range(first..first + tag.len(), "<CURRENCYNAME TYPE=\"String\"/>");
    let rows = parse_native_ledger_snapshot(&empty).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.currency_name.is_none())
            .count(),
        1
    );
    let mut absent = book.clone();
    absent.replace_range(first..first + tag.len(), "");
    let rows = parse_native_ledger_snapshot(&absent).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.currency_name.is_none())
            .count(),
        1
    );
    let mut twice = book.clone();
    twice.insert_str(first, tag);
    assert_eq!(
        parse_native_ledger_snapshot(&twice),
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_duplicate_currency_name"
        ))
    );
}
