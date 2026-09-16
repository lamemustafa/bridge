//! Proposal construction, ported from `scripts/bank_statement_import.test.py`.
//!
//! The reference emitted XML and a REMOTEID; Bridge's writer owns both now, so
//! these tests assert the `build_import_xml` payload shape instead, and that no
//! identity is minted here.

mod common;

use bridge_bank_statement::bank::Bank;
use bridge_bank_statement::date::Date;
use bridge_bank_statement::mapping::{Mapping, MappingRow};
use bridge_bank_statement::parse::Row;
use bridge_bank_statement::proposals::{
    build, group_counterparties, selfcheck, Build, BuildOptions, Disposition, Side,
    StatementRecord, VoucherType,
};
use common::*;

fn options<'a>(suspense: &'a str) -> BuildOptions<'a> {
    BuildOptions {
        bank_ledger: "Bank",
        suspense_ledger: suspense,
        account_label: "ACC",
        account_number: "00000000001234",
        date_from: None,
        date_to: None,
    }
}

fn mapping(rows: &[(&str, &str, &str)]) -> Mapping {
    Mapping::from_rows(
        rows.iter()
            .enumerate()
            .map(|(index, (party, ledger, treatment))| MappingRow {
                origin: format!("row {index}"),
                party: party.to_string(),
                ledger: ledger.to_string(),
                treatment: Some(treatment.to_string()),
            }),
    )
    .unwrap()
}

fn upi(date: &str, name: &str, reference: &str, dr: &str, cr: &str, bal: &str) -> Row {
    row(&[
        ("date", date),
        ("narr", &format!("UPI-{name}-9@x-ABCD0001-{reference}-P")),
        ("ref", "1"),
        ("dr", dr),
        ("cr", cr),
        ("bal", bal),
    ])
}

#[test]
fn build_treatments() {
    let rows = [
        upi("01/08/26", "ALPHA", "111111111111", "10.00", "", "990.00"),
        upi(
            "02/08/26",
            "OWN ACCT",
            "222222222222",
            "20.00",
            "",
            "970.00",
        ),
        upi("03/08/26", "GHOST", "333333333333", "", "30.00", "1000.00"),
    ];
    let mapping = mapping(&[
        ("ALPHA", "Alpha Ledger", "contra"),
        ("OWN ACCT", "", "skip"),
    ]);
    let Build { proposals, records } =
        build(&rows, Bank::Hdfc, &mapping, &options("SUSPENSE ACC")).unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(records.len(), 3);
    assert_eq!(proposals[0].voucher_type, VoucherType::Contra);
    assert_eq!(
        records
            .iter()
            .map(|record| record.disposition)
            .collect::<Vec<_>>(),
        [
            Disposition::Voucher(VoucherType::Contra),
            Disposition::Skipped,
            Disposition::Voucher(VoucherType::Receipt)
        ]
    );
    // a skipped row still records its label, so what was left out is visible
    assert!(!records[1].bridge_txn_id.is_empty());
    assert!(records[2].suspense);
    assert!(proposals[1]
        .narration
        .contains("reallocate from SUSPENSE ACC"));
    assert!(proposals[1].narration.contains("GHOST"));

    // payload shape: two entries, Dr first, amounts to two places
    let contra = &proposals[0];
    assert_eq!(contra.date, "2026-08-01");
    assert_eq!(contra.entries[0].ledger, "Alpha Ledger");
    assert_eq!(contra.entries[0].side, Side::Dr);
    assert_eq!(contra.entries[1].ledger, "Bank");
    assert_eq!(contra.entries[1].side, Side::Cr);
    assert_eq!(contra.entries[0].amount, "10.00");
    let receipt = &proposals[1];
    assert_eq!(
        (receipt.entries[0].ledger.as_str(), receipt.entries[0].side),
        ("Bank", Side::Dr)
    );
    assert_eq!(
        (receipt.entries[1].ledger.as_str(), receipt.entries[1].side),
        ("SUSPENSE ACC", Side::Cr)
    );

    // exactly the fields build_import_xml takes, and no identity of our own
    let json = serde_json::to_value(&proposals[0]).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "bridge_txn_id",
            "date",
            "entries",
            "narration",
            "voucher_type"
        ]
    );
    let text = json.to_string();
    for forbidden in [
        "REMOTEID",
        "remote_id",
        "batch_id",
        "<VOUCHER",
        "company_guid",
    ] {
        assert!(!text.contains(forbidden), "{forbidden}");
    }
    assert!(regex::Regex::new(r"^[A-Za-z0-9_-]{1,64}$")
        .unwrap()
        .is_match(&proposals[0].bridge_txn_id));
}

#[test]
fn a_row_landing_in_suspense_is_flagged_loosely_and_named_exactly() {
    let rows = [upi(
        "01/08/26",
        "ALPHA",
        "111111111111",
        "10.00",
        "",
        "990.00",
    )];
    let built = build(
        &rows,
        Bank::Hdfc,
        &mapping(&[("ALPHA", "suspense-acc", "auto")]),
        &options("SUSPENSE ACC"),
    )
    .unwrap();
    assert!(built.records[0].suspense);
    assert!(built.proposals[0].narration.contains("UNIDENTIFIED"));
    assert!(built.proposals[0]
        .narration
        .contains("reallocate from suspense-acc"));
    assert_eq!(built.proposals[0].entries[0].ledger, "suspense-acc");

    let built = build(
        &rows,
        Bank::Hdfc,
        &mapping(&[("ALPHA", "A-B", "auto")]),
        &options("A B"),
    )
    .unwrap();
    assert!(built.records[0].suspense);
    assert!(built.proposals[0].narration.contains("reallocate from A-B"));
    assert_eq!(built.proposals[0].entries[0].ledger, "A-B");
}

#[test]
fn transaction_labels_derive_from_the_row_not_its_position() {
    let alpha = upi("01/08/26", "ALPHA", "111111111111", "10.00", "", "990.00");
    let beta = upi("01/08/26", "BETA", "222222222222", "20.00", "", "970.00");
    let none = Mapping::default();
    let one = build(
        &[alpha.clone(), beta.clone()],
        Bank::Hdfc,
        &none,
        &options("SUSP"),
    )
    .unwrap();
    let earlier = upi("31/07/26", "GAMMA", "333333333333", "5.00", "", "1000.00");
    let two = build(
        &[earlier, alpha.clone(), beta],
        Bank::Hdfc,
        &none,
        &options("SUSP"),
    )
    .unwrap();
    assert_eq!(one.records[0].bridge_txn_id, two.records[1].bridge_txn_id);
    assert_eq!(one.records[1].bridge_txn_id, two.records[2].bridge_txn_id);
    assert_ne!(one.records[0].bridge_txn_id, one.records[1].bridge_txn_id);

    // a corrected mapping keeps every label, which is what an amendment names
    let remapped = build(
        std::slice::from_ref(&alpha),
        Bank::Hdfc,
        &mapping(&[("ALPHA", "Alpha Ledger", "auto")]),
        &options("SUSP"),
    )
    .unwrap();
    assert_eq!(
        remapped.records[0].bridge_txn_id,
        one.records[0].bridge_txn_id
    );

    refuses(
        build(&[alpha.clone(), alpha], Bank::Hdfc, &none, &options("SUSP")),
        "duplicate_statement_row",
    );
}

#[test]
fn identical_same_day_payments_differ_by_their_running_balance() {
    // The same payee, amount, date and narration twice in one day: only the
    // balance the bank printed after each tells them apart. Without it in the
    // label the second would be refused as a duplicate of the first.
    let first = upi("01/08/26", "ALPHA", "111111111111", "10.00", "", "990.00");
    let second = upi("01/08/26", "ALPHA", "111111111111", "10.00", "", "980.00");
    let built = build(
        &[first, second],
        Bank::Hdfc,
        &Mapping::default(),
        &options("SUSP"),
    )
    .unwrap();
    assert_ne!(
        built.records[0].bridge_txn_id,
        built.records[1].bridge_txn_id
    );
}

#[test]
fn transaction_labels_bind_the_statement_account() {
    let alpha = upi("01/08/26", "ALPHA", "111111111111", "10.00", "", "990.00");
    let one = build(
        std::slice::from_ref(&alpha),
        Bank::Hdfc,
        &Mapping::default(),
        &options("SUSP"),
    )
    .unwrap();
    let mut other = options("SUSP");
    other.account_number = "00000000005678";
    let two = build(&[alpha], Bank::Hdfc, &Mapping::default(), &other).unwrap();
    assert_ne!(one.records[0].bridge_txn_id, two.records[0].bridge_txn_id);
}

#[test]
fn impossible_dates_are_typed() {
    let rows = [upi("31/02/26", "A", "111111111111", "10.00", "", "990.00")];
    let refusal = refuses(
        build(&rows, Bank::Hdfc, &Mapping::default(), &options("SUSP")),
        "unparseable_date",
    );
    assert_eq!(refusal.row, Some(1));
}

#[test]
fn empty_selection_is_not_a_successful_import() {
    let rows = [upi("01/08/26", "A", "111111111111", "10.00", "", "990.00")];
    let mut window = options("SUSP");
    window.date_from = Date::new(2025, 1, 1);
    window.date_to = Date::new(2025, 12, 31);
    refuses(
        build(&rows, Bank::Hdfc, &Mapping::default(), &window),
        "empty_selection",
    );
}

#[test]
fn a_voucher_with_identical_legs_is_refused() {
    let rows = [upi(
        "01/08/26",
        "ALPHA",
        "111111111111",
        "10.00",
        "",
        "990.00",
    )];
    let mut same = options("SUSP");
    same.bank_ledger = "HDFC BANK LTD.";
    refuses(
        build(
            &rows,
            Bank::Hdfc,
            &mapping(&[("ALPHA", "hdfc bank ltd.", "auto")]),
            &same,
        ),
        "self_cancelling_voucher",
    );
}

#[test]
fn rows_bridge_cannot_build_are_refused_here_with_their_row() {
    let none = Mapping::default();
    let zero = [upi("01/08/26", "A", "111111111111", "", "0.00", "990.00")];
    assert_eq!(
        refuses(
            build(&zero, Bank::Hdfc, &none, &options("SUSP")),
            "zero_amount_row"
        )
        .row,
        Some(1)
    );
    let empty = [upi("01/08/26", "A", "111111111111", "", "", "990.00")];
    refuses(
        build(&empty, Bank::Hdfc, &none, &options("SUSP")),
        "row_without_amount",
    );
    let both = [upi(
        "01/08/26",
        "A",
        "111111111111",
        "1.00",
        "2.00",
        "990.00",
    )];
    refuses(
        build(&both, Bank::Hdfc, &none, &options("SUSP")),
        "two_sided_row",
    );
    let mut blank = options("SUSP");
    blank.bank_ledger = "  ";
    refuses(
        build(
            &[upi("01/08/26", "A", "111111111111", "1.00", "", "1.00")],
            Bank::Hdfc,
            &none,
            &blank,
        ),
        "blank_ledger",
    );
    let marked = [upi(
        "01/08/26",
        "X[bridge:Y",
        "111111111111",
        "1.00",
        "",
        "1.00",
    )];
    refuses(
        build(&marked, Bank::Hdfc, &none, &options("SUSP")),
        "narration_not_admissible",
    );
    refuses(
        build(
            &[upi("01/08/26", "A", "111111111111", "1.00", "", "1.00")],
            Bank::Hdfc,
            &mapping(&[("A", "Ledger\u{7}", "auto")]),
            &options("SUSP"),
        ),
        "ledger_not_admissible",
    );
    // an amount with one decimal place is written with two
    let one_place = build(
        &[upi("01/08/26", "A", "111111111111", "12.5", "", "1.00")],
        Bank::Hdfc,
        &none,
        &options("SUSP"),
    )
    .unwrap();
    assert_eq!(one_place.proposals[0].entries[0].amount, "12.50");
}

#[test]
fn selfcheck_rejects_bad_proposals() {
    let rows = [upi("01/08/26", "P", "111111111111", "10.00", "", "990.00")];
    let good = build(&rows, Bank::Hdfc, &Mapping::default(), &options("SUSP")).unwrap();
    let counted = selfcheck(&good, "Bank").unwrap();
    assert_eq!(counted.vouchers, 1);
    assert!(counted
        .bank_out
        .numeric_eq(&bridge_tally_primitives::ExactDecimal::parse("10.00").unwrap()));
    assert!(counted.bank_in.is_zero());
    // the bank leg is recognised by the loose fold, not string equality
    assert!(!selfcheck(&good, "bank").unwrap().bank_out.is_zero());

    let mut unbalanced = good.clone();
    unbalanced.proposals[0].entries[0].amount = "11.00".to_string();
    refuses(selfcheck(&unbalanced, "Bank"), "unbalanced_voucher");
    let mut one_sided = good.clone();
    one_sided.proposals[0].entries[1].side = Side::Dr;
    refuses(selfcheck(&one_sided, "Bank"), "unbalanced_voucher");
    let mut silent = good.clone();
    silent.proposals[0].narration = " ".to_string();
    refuses(selfcheck(&silent, "Bank"), "empty_narration");
    let mut miscounted = good.clone();
    miscounted.records.push(miscounted.records[0].clone());
    refuses(selfcheck(&miscounted, "Bank"), "manifest_count_mismatch");
}

fn record(party: &str, amount: &str) -> StatementRecord {
    StatementRecord {
        row: 1,
        date: "2026-08-01".to_string(),
        disposition: Disposition::Voucher(VoucherType::Receipt),
        amount: amount.to_string(),
        party: party.to_string(),
        ledger: "SUSP".to_string(),
        suspense: true,
        bridge_txn_id: "st-x".to_string(),
    }
}

#[test]
fn counterparties_group_by_mapping_key() {
    let groups = group_counterparties(&[
        record("MERCURY MANUFACTURERS", "60000.00"),
        record("MERCURY M ANUFACTURERS", "220000.00"),
        record("AMBIKA INDUSTRIES", "1000.00"),
    ])
    .unwrap();
    assert_eq!(groups.len(), 2);
    // one line for two spellings, labelled with the one the bank printed
    assert_eq!(groups[0].party, "MERCURY MANUFACTURERS");
    assert_eq!(groups[0].total, "280000.00");
    assert_eq!(groups[0].rows, 2);
    assert_eq!(groups[0].also_printed_as, ["MERCURY M ANUFACTURERS"]);
    assert_eq!(groups[1].party, "AMBIKA INDUSTRIES");
}
