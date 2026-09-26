use super::*;

/// Filter rows as `WithheldVoucher::filter_view` shapes them; not Tally output.
fn withheld_rows(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "guid": format!("guid-{index:04}"), "date": "20260915", "voucher_type": "Sales",
                "voucher_number": index.to_string(), "party": "a party",
                "amounts": [{"ledger": "a ledger"}], WITHHELD_MARKER: WITHHELD_FOREIGN_CURRENCY,
            })
        })
        .collect()
}

#[test]
fn the_withheld_listing_is_capped_and_in_window_order() {
    let listed = listed_withheld(&withheld_rows(MAX_WITHHELD_LISTED + 1));
    assert_eq!(listed.len(), MAX_WITHHELD_LISTED);
    assert_eq!(listed[0]["guid"], "guid-0000");
    assert_eq!(listed[MAX_WITHHELD_LISTED - 1]["guid"], format!("guid-{:04}", MAX_WITHHELD_LISTED - 1));
}

#[test]
fn a_listed_withheld_voucher_names_no_party_ledger_or_amount() {
    let listed = listed_withheld(&withheld_rows(1));
    let keys: Vec<&String> = listed[0].as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["cause", "date", "guid", "voucher_number", "voucher_type"]);
    assert_eq!(listed[0]["cause"], WITHHELD_FOREIGN_CURRENCY);
}
