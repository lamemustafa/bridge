//! Mapping keys and the mapping loader, ported from
//! `scripts/bank_statement_import.test.py`.

mod common;

use bridge_bank_statement::mapping::{Mapping, MappingRow, Treatment};
use bridge_bank_statement::text::{ledger_key, mapping_key};
use common::*;

fn load(text: &str) -> Result<Mapping, bridge_bank_statement::Refusal> {
    Mapping::from_csv(text.as_bytes())
}

#[test]
fn mapping_key_survives_non_ascii_scripts() {
    assert!(!mapping_key("पार्टी").is_empty());
    let names = ["पार्टी", "पारटी", "ஏபிசி", "কোম্পানি"];
    let keys: std::collections::BTreeSet<String> =
        names.iter().map(|name| mapping_key(name)).collect();
    assert_eq!(keys.len(), names.len());
}

#[test]
fn mapping_key_keeps_punctuation_significant() {
    for (left, right) in [
        ("A & B", "AB"),
        ("S.K. Minerals", "SK Minerals"),
        ("M/s Mercury", "Ms Mercury"),
        ("Shree-Ram Traders", "Shree Ram Traders"),
    ] {
        assert_ne!(mapping_key(left), mapping_key(right), "{left} / {right}");
    }
    assert_eq!(
        mapping_key("MERCURYM ANUFACTURERS"),
        mapping_key("MERCURYMANUFACTURERS")
    );
    assert_eq!(
        mapping_key("ZEPHYR MANUFACTURING"),
        mapping_key("ZEPHYRMANUFACTURING")
    );
    assert_eq!(mapping_key("m/s mercury"), mapping_key("M/S MERCURY"));
    assert_eq!(mapping_key("A&B"), "A&B");
}

#[test]
fn mapping_key_ignores_wrap_spacing() {
    let mapping =
        load("party,ledger,treatment\nZEPHYR MANUFACTURING,M/s Zephyr,auto\nOWN ACCOUNT,,skip\n")
            .unwrap();
    for spelling in [
        "ZEPHYRM ANUFACTURING",
        "ZEPHYRMANUFACTURING",
        "Zephyr Manufacturing",
    ] {
        assert_eq!(
            mapping.get(spelling),
            Some(&("M/s Zephyr".to_string(), Treatment::Auto)),
            "{spelling}"
        );
    }
    assert_eq!(mapping.get("OWN ACCOUNT").unwrap().1, Treatment::Skip);
}

#[test]
fn mapping_refuses_ambiguous_input() {
    refuses(
        load("name,ledger,treatment\nA,L,auto\n"),
        "mapping_headers_missing",
    );
    refuses(
        load("party,ledger,treatment\nZEPHYR MANUFACTURING,Ledger One,auto\nZEPHYRMANUFACTURING,Ledger Two,auto\n"),
        "mapping_key_collision",
    );
    let same = load(
        "party,ledger,treatment\nZEPHYR MANUFACTURING,One,auto\nZEPHYRMANUFACTURING,One,auto\n",
    )
    .unwrap();
    assert_eq!(
        same.get("ZEPHYRMANUFACTURING"),
        Some(&("One".to_string(), Treatment::Auto))
    );
    let two = load("party,ledger,treatment\nA & B,One,auto\nAB,Two,auto\n").unwrap();
    assert_eq!(
        two.get("A & B"),
        Some(&("One".to_string(), Treatment::Auto))
    );
    assert_eq!(two.get("AB"), Some(&("Two".to_string(), Treatment::Auto)));
    refuses(
        load("party,ledger,treatment\nOWN,,contra\n"),
        "contra_without_ledger",
    );
    refuses(
        load("party,ledger,treatment\nA,L,transfer\n"),
        "unknown_treatment",
    );
    assert_eq!(
        load("party,ledger,treatment\n---,L,auto\n")
            .unwrap()
            .get("---"),
        Some(&("L".to_string(), Treatment::Auto))
    );
    refuses(
        load("party,Party,ledger,treatment\nA,,L,auto\n"),
        "mapping_headers_duplicated",
    );
    let padded = load(" Party , Ledger , Treatment \nAcme,Acme Ledger,auto\n").unwrap();
    assert_eq!(
        padded.get("ACME"),
        Some(&("Acme Ledger".to_string(), Treatment::Auto))
    );
    // a blank treatment defaults to auto; a whitespace-only one does not
    assert_eq!(
        load("party,ledger,treatment\nA,L,\n")
            .unwrap()
            .get("A")
            .unwrap()
            .1,
        Treatment::Auto
    );
    refuses(
        load("party,ledger,treatment\nA,L,  \n"),
        "unknown_treatment",
    );
}

#[test]
fn a_mapping_may_not_claim_the_unresolved_sentinel() {
    for sentinel in ["UNNAMED", "UNRESOLVED"] {
        let refusal = refuses(
            load(&format!(
                "party,ledger,treatment\n{sentinel},Some Ledger,auto\n"
            )),
            "mapping_claims_a_sentinel",
        );
        assert!(refusal.message.contains("could NOT identify"));
    }
    refuses(
        load("party,ledger,treatment\nun resolved,L,skip\n"),
        "mapping_claims_a_sentinel",
    );
    assert!(
        !load("party,ledger,treatment\nUNRESOLVED TRADING CO,L,auto\n")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn inline_rows_follow_the_same_rules() {
    let rows = |party: &str, ledger: &str, treatment: Option<&str>| {
        vec![MappingRow {
            origin: "mapping[0]".to_string(),
            party: party.to_string(),
            ledger: ledger.to_string(),
            treatment: treatment.map(str::to_string),
        }]
    };
    assert_eq!(
        Mapping::from_rows(rows("Acme", "Acme Ledger", None))
            .unwrap()
            .get("ACME"),
        Some(&("Acme Ledger".to_string(), Treatment::Auto))
    );
    refuses(
        Mapping::from_rows(rows("UNRESOLVED", "L", None)),
        "mapping_claims_a_sentinel",
    );
    refuses(
        Mapping::from_rows(rows("Own", "", Some("contra"))),
        "contra_without_ledger",
    );
}

#[test]
fn ledger_key_folds_exactly_what_its_docstring_claims() {
    let same = |left: &str, right: &str| ledger_key(left) == ledger_key(right);
    assert!(same("bridge probe ledger a", "BRIDGE PROBE LEDGER A"));
    assert!(same("BRIDGE PROBE LEDGER A", "BRIDGE-PROBE-LEDGER-A"));
    assert!(same("A B ", "A B"));
    assert!(same("A-B", "A B"));
    assert!(same("A  B", "A B"));
    assert!(same("  A B", "A B"));
    assert!(same("A   ", "A"));
    assert!(same("straße", "STRASSE"));
    assert!(same("A\tB", "A B"));
    assert!(same("A\u{a0}B", "A B"));
    assert!(!same("ZZ Ram AND Sons", "ZZ Ram & Sons"));
    assert!(!same("AB", "A & B"));
    assert!(!same("A_B", "A B"));
    assert!(!same("A/B", "A B"));
    assert!(!same("A\u{2013}B", "A B"));
}
