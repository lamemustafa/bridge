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

/// The FOREX capture as captured, its three composite balances intact: the
/// classified parse excludes the three `$` ledgers before parsing any balance,
/// so the read no longer refuses, and the seven rupee ledgers keep their
/// parsed balances.
#[test]
fn the_classified_snapshot_excludes_foreign_ledgers_before_parsing_their_balances() {
    let snapshot = crate::native_outstandings::parse_native_ledger_snapshot_classified(
        &forex_book(),
        &forex_base(),
    )
    .unwrap();
    assert_eq!(
        snapshot
            .foreign
            .iter()
            .map(|ledger| ledger.ledger.as_str())
            .collect::<Vec<_>>(),
        ["BRIDGE FX DEBTOR A", "FX USD Debtor 01", "FX USD Debtor 02"]
    );
    assert_eq!(
        snapshot
            .base
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        FOREX_ROWS
            .iter()
            .filter(|(_, currency)| *currency != "$")
            .map(|(ledger, _)| *ledger)
            .collect::<Vec<_>>()
    );
    let inr_debtor = snapshot
        .base
        .iter()
        .find(|entry| entry.name == "BRIDGE INR DEBTOR A")
        .unwrap();
    assert_eq!(
        inr_debtor.closing_balance,
        Some(bridge_tally_primitives::ExactDecimal::parse("-3000.00").unwrap())
    );
    assert_eq!(snapshot.unobserved, 0);
}

/// A composite balance on a ledger classified as the base is still refused:
/// only a foreign ledger's balance is excused from the parse.
#[test]
fn a_composite_balance_on_a_base_ledger_still_refuses() {
    let book = forex_book();
    // Relabel the one `$` ledger with a composite opening as a rupee ledger.
    let start = book.find("<LEDGER NAME=\"BRIDGE FX DEBTOR A\"").unwrap();
    let tag = "<CURRENCYNAME TYPE=\"String\">$</CURRENCYNAME>";
    let at = start + book[start..].find(tag).unwrap();
    let mut relabelled = book.clone();
    relabelled.replace_range(
        at..at + tag.len(),
        "<CURRENCYNAME TYPE=\"String\">I\u{20b9}</CURRENCYNAME>",
    );
    assert_eq!(
        crate::native_outstandings::parse_native_ledger_snapshot_classified(
            &relabelled,
            &forex_base()
        ),
        Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance {
            ledger_name: "BRIDGE FX DEBTOR A".to_string()
        })
    );
}

#[test]
fn the_classified_snapshot_refuses_what_the_classification_refuses() {
    assert_eq!(
        crate::native_outstandings::parse_native_ledger_snapshot_classified(
            &forex_book(),
            &BaseCurrencyName::among_several_for_tests("\u{20b9}"),
        ),
        Err(NativeOutstandingsError::LedgerCurrency(
            LedgerCurrencyRefusal::BaseUnmatched { ledger: None }
        ))
    );
    let single = crate::native_outstandings::parse_native_ledger_snapshot_classified(
        &single_currency_book(),
        &rs_base(),
    )
    .unwrap();
    assert_eq!(
        (single.base.len(), single.foreign.len(), single.unobserved),
        (13, 0, 0)
    );
}

// ---- Outstandings over the captured FOREX bills and snapshot (forex PR 2a) ----

use crate::native_outstandings::{
    compute_native_outstandings, compute_native_outstandings_with_exclusions,
    parse_native_bill_rows, parse_native_ledger_snapshot_classified, AgeingAnchor,
    NativeGroupSnapshot, NativeMasterSnapshot,
};
use bridge_tally_primitives::{ExactDecimal, TallyDate};

fn bills() -> Vec<crate::native_outstandings::NativeBillRow> {
    parse_native_bill_rows(
        &decode(include_bytes!(
            "../../tests/fixtures/bills_receivable_forex_live.utf16le.xml"
        )),
        // The book's own `BOOKSFROM`; FX-OPEN-1 is dated the day before it
        // (TALLY_PROTOCOL_REFERENCE §12a.10).
        &TallyDate::parse("20250401").unwrap(),
        &TallyDate::parse("20250930").unwrap(),
    )
    .unwrap()
}

fn amount(value: &str) -> ExactDecimal {
    ExactDecimal::parse(value).unwrap()
}

/// The book's rupee master is `I₹`, one of its two masters. Built through the
/// test-only constructor, as a classification test needs no currency read.
fn forex_snapshot() -> crate::native_outstandings::ClassifiedLedgerSnapshot {
    parse_native_ledger_snapshot_classified(
        &decode(include_bytes!(
            "../../tests/fixtures/ledgers_currency_forex_live.utf16le.xml"
        )),
        &BaseCurrencyName::among_several_for_tests("I\u{20b9}"),
    )
    .unwrap()
}

#[test]
fn the_four_dollar_bills_are_left_out_of_every_figure() {
    let bills = bills();
    assert_eq!(bills.len(), 18);
    let snapshot = forex_snapshot();
    let result = compute_native_outstandings_with_exclusions(
        "BRIDGE CORPUS FOREX",
        &bills,
        &[],
        NativeMasterSnapshot {
            ledgers: &snapshot.base,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        &snapshot.foreign,
        AgeingAnchor::DueDate,
        &TallyDate::parse("20250930").unwrap(),
        0,
    )
    .unwrap();
    // The 14 rupee bills only: INR-OPEN-1 1,000 + INR-INV-1 2,000 + the
    // twelve FX-INV-01..12 on the rupee "FX Party" ledgers, 1,250 to 4,000 in
    // steps of 250 (31,500). The four dollar bills (350,100 read as rupees)
    // are not in it.
    assert_eq!(result.report.receivable_total, amount("34500"));
    let parties = result
        .report
        .top_parties
        .iter()
        .map(|party| party.party.as_str())
        .collect::<Vec<_>>();
    for dollar in ["BRIDGE FX DEBTOR A", "FX USD Debtor 01", "FX USD Debtor 02"] {
        assert!(!parties.contains(&dollar), "{dollar} in {parties:?}");
        assert!(
            result
                .residuals
                .iter()
                .all(|residual| residual.party != dollar),
            "{dollar} has a residual"
        );
    }
    assert_eq!(
        result
            .foreign_currency_ledgers_excluded
            .iter()
            .map(|ledger| ledger.ledger.as_str())
            .collect::<Vec<_>>(),
        ["BRIDGE FX DEBTOR A", "FX USD Debtor 01", "FX USD Debtor 02"]
    );
}

/// Control: without the exclusions, the same captured bills put the dollar
/// amounts into the rupee total. That is the leak this series closes.
#[test]
fn without_exclusions_the_dollar_bills_would_be_counted_as_rupees() {
    let bills = bills();
    let snapshot = forex_snapshot();
    let result = compute_native_outstandings(
        "BRIDGE CORPUS FOREX",
        &bills,
        &[],
        NativeMasterSnapshot {
            ledgers: &snapshot.base,
            groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
        },
        AgeingAnchor::DueDate,
        &TallyDate::parse("20250930").unwrap(),
        0,
    )
    .unwrap();
    assert_eq!(result.report.receivable_total, amount("384600"));
    assert!(result.foreign_currency_ledgers_excluded.is_empty());
}

/// With exclusions, a bill whose party is in neither the base ledgers nor the
/// excluded ones refuses: it could be a foreign ledger that cannot be told
/// apart.
#[test]
fn with_exclusions_a_bill_of_an_unknown_party_refuses() {
    let mut bills = bills();
    let stray = bills
        .iter()
        .position(|bill| bill.party == "FX Party 01")
        .unwrap();
    bills[stray].party = "Not A Ledger".to_string();
    let snapshot = forex_snapshot();
    assert_eq!(
        compute_native_outstandings_with_exclusions(
            "BRIDGE CORPUS FOREX",
            &bills,
            &[],
            NativeMasterSnapshot {
                ledgers: &snapshot.base,
                groups: NativeGroupSnapshot::LegacyFixtureWithoutGroups,
            },
            &snapshot.foreign,
            AgeingAnchor::DueDate,
            &TallyDate::parse("20250930").unwrap(),
            0,
        )
        .err(),
        Some(NativeOutstandingsError::InvalidResponse(
            "bill_party_ledger_unresolved"
        ))
    );
}

/// The compliance source's parse (bridge#551): the same classification of the
/// FOREX capture, admitted only for the company every row's collection-level
/// GUID names. Another company's GUID refuses before any row is classified.
#[test]
fn the_company_checked_classified_snapshot_admits_only_its_own_company() {
    const FOREX_GUID: &str = "b14e9b2d-8a63-4779-804d-25d59eb787eb";
    let unchecked = crate::native_outstandings::parse_native_ledger_snapshot_classified(
        &forex_book(),
        &forex_base(),
    )
    .unwrap();
    let checked = crate::native_outstandings::parse_native_ledger_snapshot_classified_for_company(
        &forex_book(),
        FOREX_GUID,
        &forex_base(),
    )
    .unwrap();
    assert_eq!(checked, unchecked);
    assert!(!checked.foreign.is_empty());
    assert_eq!(
        crate::native_outstandings::parse_native_ledger_snapshot_classified_for_company(
            &forex_book(),
            "61c6de69-1748-461c-ad3f-162cb949df9f",
            &forex_base(),
        ),
        Err(NativeOutstandingsError::InvalidResponse(
            "ledger_response_company_guid_mismatch"
        ))
    );
}

/// The compliance snapshot of the book after a dollar invoice to a rupee party
/// (captured 25 Sep, coherent with the compliance master): the dollar ledgers
/// and the rupee ledgers with a composite balance are named, and only the
/// plain rupee rows are parsed. The outstandings parse of the same bytes still
/// refuses, on the first composite base balance.
#[test]
fn the_compliance_snapshot_sets_mixed_rupee_ledgers_aside_by_name() {
    let snapshot = decode(include_bytes!(
        "../../tests/fixtures/balance_snapshot_forex_live.utf16le.xml"
    ));
    let company = "b14e9b2d-8a63-4779-804d-25d59eb787eb";
    let classified = crate::native_outstandings::parse_compliance_ledger_snapshot_for_company(
        &snapshot,
        company,
        &forex_base(),
    )
    .unwrap();
    let names = |rows: &[String]| rows.to_vec();
    assert_eq!(
        classified
            .foreign
            .iter()
            .map(|ledger| ledger.ledger.clone())
            .collect::<Vec<_>>(),
        ["BRIDGE FX DEBTOR A", "FX USD Debtor 01", "FX USD Debtor 02"]
    );
    assert_eq!(
        names(&classified.mixed),
        ["FX Party 01", "FX Sales", "Profit & Loss A/c"]
    );
    assert_eq!(
        classified
            .base
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["BRIDGE INR DEBTOR A", "Cash", "FX Party 02", "FX Party 03"]
    );
    assert!(matches!(
        crate::native_outstandings::parse_native_ledger_snapshot_classified_for_company(
            &snapshot,
            company,
            &forex_base(),
        ),
        Err(NativeOutstandingsError::ForeignCurrencyLedgerBalance { .. })
    ));
}
