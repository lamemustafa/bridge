use super::*;
use crate::native_outstandings::{
    parse_company_currency, parse_native_ledger_snapshot, NativeOutstandingsError,
};

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

fn forex_book() -> String {
    decode(include_bytes!(
        "../../tests/fixtures/ledgers_currency_forex_live.utf16le.xml"
    ))
}

/// The base of a captured single-currency book (`Rs.` / "Indian Rupees").
fn rs_base() -> BaseCurrencyName {
    let currency = parse_company_currency(&decode(include_bytes!(
        "../../tests/fixtures/currency_inr_legacy_live.utf16le.xml"
    )))
    .unwrap();
    BaseCurrencyName::of_single_master(&currency).expect("one master")
}

/// `ledgers_currency_forex_live`'s ledgers and currencies, in read order.
/// [`the_forex_capture_parses_to_these_pairs_once_its_composite_balances_are_neutralised`]
/// checks this table against the capture itself.
const FOREX_ROWS: [(&str, &str); 10] = [
    ("BRIDGE FX DEBTOR A", "$"),
    ("BRIDGE INR DEBTOR A", "I\u{20b9}"),
    ("Cash", "I\u{20b9}"),
    ("FX Party 01", "I\u{20b9}"),
    ("FX Party 02", "I\u{20b9}"),
    ("FX Party 03", "I\u{20b9}"),
    ("FX Sales", "I\u{20b9}"),
    ("FX USD Debtor 01", "$"),
    ("FX USD Debtor 02", "$"),
    ("Profit & Loss A/c", "I\u{20b9}"),
];

fn forex_rows() -> impl Iterator<Item = (&'static str, Option<&'static str>)> {
    FOREX_ROWS
        .iter()
        .map(|(ledger, currency)| (*ledger, Some(*currency)))
}

fn forex_base() -> BaseCurrencyName {
    BaseCurrencyName::among_several_for_tests("I\u{20b9}")
}

#[test]
fn a_base_is_only_taken_from_a_book_with_one_currency_master() {
    assert_eq!(rs_base().name(), "Rs.");
    // The captured FOREX currencies: `$` is read first, so `symbol` is `$`,
    // which is not the base. No base can be built from it.
    let several = parse_company_currency(&decode(include_bytes!(
        "../../tests/fixtures/currency_multi_live.utf16le.xml"
    )))
    .unwrap();
    assert_eq!(several.currency_count, 2);
    assert_eq!(several.symbol, "$");
    assert_eq!(BaseCurrencyName::of_single_master(&several), None);
}

#[test]
fn a_single_currency_book_parses_every_ledger_currency_as_its_one_master() {
    let rows = parse_native_ledger_snapshot(&single_currency_book()).unwrap();
    assert_eq!(rows.len(), 13);
    assert!(rows
        .iter()
        .all(|row| row.currency_name.as_deref() == Some("Rs.")));
    let classified = classify_ledger_currencies(
        &rs_base(),
        rows.iter()
            .map(|row| (row.name.as_str(), row.currency_name.as_deref())),
    )
    .unwrap();
    assert_eq!(classified, LedgerCurrencies::default());
}

#[test]
fn the_forex_capture_refuses_on_its_composite_balance_as_before() {
    assert_eq!(
        parse_native_ledger_snapshot(&forex_book()),
        Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance {
            ledger_name: "BRIDGE FX DEBTOR A".to_string()
        })
    );
}

/// The three composite balances (two closings, one opening) replaced by a plain amount, so that the
/// snapshot parses and every (ledger, currency) pair comes from the capture.
#[test]
fn the_forex_capture_parses_to_these_pairs_once_its_composite_balances_are_neutralised() {
    let book = forex_book();
    let mut neutral = book.clone();
    for composite in [
        "-$ 1100.00 @ I\u{20b9} 86/$  = -I\u{20b9} 94600.00",
        "-$ 500.00 @ I\u{20b9} 84/$  = -I\u{20b9} 42000.00",
        "-$ 2000.00 @ I\u{20b9} 86/$  = -I\u{20b9} 172000.00",
    ] {
        assert_eq!(book.matches(composite).count(), 1, "{composite}");
        neutral = neutral.replace(composite, "0.00");
    }
    let rows = parse_native_ledger_snapshot(&neutral).unwrap();
    let pairs = rows
        .iter()
        .map(|row| (row.name.as_str(), row.currency_name.as_deref().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(pairs, FOREX_ROWS.to_vec());
    let classified = classify_ledger_currencies(
        &forex_base(),
        rows.iter()
            .map(|row| (row.name.as_str(), row.currency_name.as_deref())),
    )
    .unwrap();
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
    for missing in [None, Some(""), Some("  ")] {
        let rows = forex_rows().map(|(ledger, currency)| {
            (
                ledger,
                if ledger == "FX USD Debtor 01" {
                    missing
                } else {
                    currency
                },
            )
        });
        assert_eq!(
            classify_ledger_currencies(&forex_base(), rows),
            Err(LedgerCurrencyRefusal::Unobserved {
                ledger: "FX USD Debtor 01".to_string()
            }),
            "{missing:?}"
        );
    }
}

#[test]
fn several_masters_with_no_ledger_in_the_base_refuse_rather_than_exclude_all() {
    // A base NAME that matches no row, e.g. the company's ORIGINALNAME `₹`
    // taken in place of the master's NAME: every ledger would otherwise be
    // "foreign" and the read merely partial.
    for base in ["\u{20b9}", "I\u{20b9} ", "Rs."] {
        let refused = classify_ledger_currencies(
            &BaseCurrencyName::among_several_for_tests(base),
            forex_rows(),
        )
        .unwrap_err();
        assert_eq!(
            refused,
            LedgerCurrencyRefusal::BaseUnmatched { ledger: None },
            "{base:?}"
        );
        assert_eq!(refused.code(), "ledger_currency_base_unmatched");
    }
    assert_eq!(
        classify_ledger_currencies(&forex_base(), []),
        Err(LedgerCurrencyRefusal::BaseUnmatched { ledger: None })
    );
}

#[test]
fn one_master_takes_a_missing_currency_as_the_base_and_counts_it() {
    let rows = [
        ("Cash", None),
        ("Sales", Some("")),
        ("Sundry", Some(" ")),
        ("Sharma Traders", Some("Rs.")),
    ];
    let classified = classify_ledger_currencies(&rs_base(), rows).unwrap();
    assert!(classified.foreign.is_empty());
    assert_eq!(classified.unobserved, 3);
    // No ledger at all is no disagreement with one master.
    assert_eq!(
        classify_ledger_currencies(&rs_base(), []),
        Ok(LedgerCurrencies::default())
    );
}

#[test]
fn one_master_refuses_a_ledger_that_names_another_currency() {
    let rows = [("Cash", Some("Rs.")), ("Sales", Some("$"))];
    let refused = classify_ledger_currencies(&rs_base(), rows).unwrap_err();
    assert_eq!(
        refused,
        LedgerCurrencyRefusal::BaseUnmatched {
            ledger: Some("Sales".to_string())
        }
    );
    assert_eq!(refused.code(), "ledger_currency_base_unmatched");
}

#[test]
fn the_snapshot_reads_an_empty_currency_as_none_and_refuses_a_duplicate() {
    let book = single_currency_book();
    let tag = "<CURRENCYNAME TYPE=\"String\">Rs.</CURRENCYNAME>";
    let first = book.find(tag).expect("the capture carries the element");
    let with = |replacement: &str| {
        let mut changed = book.clone();
        changed.replace_range(first..first + tag.len(), replacement);
        changed
    };
    for empty in [
        "<CURRENCYNAME TYPE=\"String\"/>",
        "<CURRENCYNAME TYPE=\"String\"></CURRENCYNAME>",
        "<CURRENCYNAME TYPE=\"String\">  </CURRENCYNAME>",
        "",
    ] {
        let rows = parse_native_ledger_snapshot(&with(empty)).unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row.currency_name.is_none())
                .count(),
            1,
            "{empty:?}"
        );
    }
    for twice in [
        format!("{tag}{tag}"),
        format!("<CURRENCYNAME TYPE=\"String\"/>{tag}"),
        format!("{tag}<CURRENCYNAME TYPE=\"String\"/>"),
    ] {
        assert_eq!(
            parse_native_ledger_snapshot(&with(&twice)),
            Err(NativeOutstandingsError::InvalidResponse(
                "ledger_duplicate_currency_name"
            )),
            "{twice}"
        );
    }
}
