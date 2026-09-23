//! The review dialog's text (#239): what it shows, and its refusals.
use super::*;

const BATCH: &str = "bridge-6c79872c-aab6-4be5-a181-18182c8148be";

fn row(entries: usize, narration: &str) -> ReadVoucher {
    serde_json::from_value(row_json(entries, narration)).unwrap()
}

fn row_json(entries: usize, narration: &str) -> Value {
    json!({
        "remote_id":null,"guid":"g-1","alter_id":10,"date":"20260907","voucher_type":"Journal",
        "narration":narration,"voucher_number":"2","master_id":"5","cancelled":false,"optional":false,
        "effective_date":null,"amounts":(0..entries).map(|index| json!({"ledger":format!("Ledger {index}"),"amount":"-1.00","is_deemed_positive":"Yes"})).collect::<Vec<_>>(),
    })
}

fn doubt() -> Value {
    json!({"state":"posted_under_changed_masters","ledgers":["Cash"]})
}

#[test]
fn the_review_shows_the_doubt_and_the_voucher_as_read() {
    let preview = review_preview(BATCH, "Books", &doubt(), &row(2, "Paid")).unwrap();
    for shown in [
        "Record that you reviewed ONE Journal",
        "\"Cash\"",
        "Dr -1.00  \"Ledger 0\"",
        "ALTERID: 10",
        "reconciliation_required",
    ] {
        assert!(preview.contains(shown), "{shown}: {preview}");
    }
    assert!(
        !preview.contains("Post"),
        "never reads as a post: {preview}"
    );
}

#[test]
fn a_review_too_long_to_show_is_refused_not_truncated() {
    assert!(review_preview(BATCH, "Books", &doubt(), &row(12, "Paid")).is_ok());
    assert_eq!(
        review_preview(BATCH, "Books", &doubt(), &row(40, "Paid")),
        Err("ack_review_too_large".to_string())
    );
}

#[test]
fn a_value_with_a_line_break_or_hidden_character_is_refused() {
    assert_eq!(
        review_preview(BATCH, "Books", &doubt(), &row(2, "Paid\r\nmore")),
        Err("ack_review_layout_text".to_string())
    );
    assert_eq!(
        review_preview(BATCH, "Books", &doubt(), &row(2, "Pa\u{200B}id")),
        Err("ack_review_format_text".to_string())
    );
}

/// Every field the verification read returns is in the fingerprint: each,
/// changed alone, changes it.
#[test]
fn each_fingerprinted_field_changed_alone_changes_the_fingerprint() {
    let base = row_json(2, "Paid");
    let original = voucher_fingerprint(&row(2, "Paid"));
    let changes: [(&str, Value); 12] = [
        ("/guid", json!("g-2")),
        ("/master_id", json!("6")),
        ("/remote_id", json!("r-1")),
        ("/date", json!("20260908")),
        ("/effective_date", json!("20260907")),
        ("/voucher_type", json!("Payment")),
        ("/voucher_number", json!("3")),
        ("/narration", json!("Paid.")),
        ("/cancelled", json!(true)),
        ("/optional", json!(true)),
        ("/amounts/0/ledger", json!("Ledger 9")),
        ("/amounts/0/amount", json!("-2.00")),
    ];
    for (pointer, value) in changes {
        let mut changed = base.clone();
        *changed
            .pointer_mut(pointer)
            .unwrap_or_else(|| panic!("{pointer}")) = value;
        let changed: ReadVoucher = serde_json::from_value(changed).unwrap();
        assert_ne!(voucher_fingerprint(&changed), original, "{pointer}");
    }
    let mut sign = base.clone();
    *sign.pointer_mut("/amounts/0/is_deemed_positive").unwrap() = json!("No");
    let sign: ReadVoucher = serde_json::from_value(sign).unwrap();
    assert_ne!(voucher_fingerprint(&sign), original, "is_deemed_positive");
    // The ALTERID is bound on its own, not through the fingerprint.
    let mut alter = base;
    *alter.pointer_mut("/alter_id").unwrap() = json!(11);
    let alter: ReadVoucher = serde_json::from_value(alter).unwrap();
    assert_eq!(voucher_fingerprint(&alter), original);
}
