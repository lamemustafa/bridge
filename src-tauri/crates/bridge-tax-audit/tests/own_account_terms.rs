// SPDX-License-Identifier: Apache-2.0
//! `[roles].own_account_narration_terms` is read by `cash_book_integrity` alone, so a malformed
//! value fails that test and no other, and a well-formed one reaches part 3.

mod common;

use bridge_tax_audit::{
    cash_book_integrity_canonical, rules_for, trial_balance_canonical, Engagement,
};
use serde_json::Value;

/// The synthetic engagement with one `[roles]` line added.
fn engagement_with(terms_line: &str) -> Engagement {
    let text = std::fs::read_to_string(common::fixtures().join("synthetic-engagement.toml"))
        .unwrap()
        .replacen(
            "round_off_ledgers = [\"Round Off\"]\n",
            &format!("round_off_ledgers = [\"Round Off\"]\n{terms_line}\n"),
            1,
        );
    assert!(text.contains(terms_line), "the [roles] anchor moved");
    Engagement::from_toml(&text, &common::fixtures()).unwrap()
}

fn terms_count(doc: &Value) -> Value {
    doc["figures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "cash_book_integrity.own_account_terms_count")
        .unwrap()["value"]
        .clone()
}

#[test]
fn a_malformed_value_fails_cash_book_integrity_alone() {
    for line in [
        "own_account_narration_terms = \"SELF\"",
        "own_account_narration_terms = [\"SELF\", 7]",
        "own_account_narration_terms = { SELF = 1 }",
    ] {
        let e = engagement_with(line);
        let rules = rules_for(&e).unwrap();
        assert!(trial_balance_canonical(&e, &rules).is_ok(), "{line}");
        let err = cash_book_integrity_canonical(&e, &rules)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("cash_book_integrity: [roles].own_account_narration_terms"),
            "{line}: {err}"
        );
    }
}

#[test]
fn a_list_of_strings_reaches_part_3() {
    let e = engagement_with("own_account_narration_terms = [\"SELF TRANSFER\", \"Own A/c\"]");
    let doc = cash_book_integrity_canonical(&e, &rules_for(&e).unwrap()).unwrap();
    assert_eq!(terms_count(&doc), 2);
}

#[test]
fn an_absent_key_is_no_terms() {
    let e = engagement_with("");
    let doc = cash_book_integrity_canonical(&e, &rules_for(&e).unwrap()).unwrap();
    assert_eq!(terms_count(&doc), 0);
}
