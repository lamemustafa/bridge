//! The review dialog's text (#239): what it shows, and its refusals.
use super::*;

const BATCH: &str = "bridge-6c79872c-aab6-4be5-a181-18182c8148be";
const MARKER: &str = "[BRIDGE:9c8d8de4-c06c-847b-8309-60ba702bf663]";

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
    let mut voucher = row_json(2, "Paid");
    voucher["amounts"][1]["is_deemed_positive"] = json!("No");
    voucher["amounts"][1]["amount"] = json!("1.00");
    let voucher: ReadVoucher = serde_json::from_value(voucher).unwrap();
    let preview = review_preview(BATCH, MARKER, "Books", &doubt(), &voucher).unwrap();
    for shown in [
        "Record that you reviewed ONE Journal in \"Books\"",
        "  \"Cash\"",
        "Date: \"20260907\"  Voucher number: \"2\"  ALTERID: 10",
        "Narration:\n  \"Paid\"",
        "Dr -1.00  \"Ledger 0\"",
        "Cr 1.00  \"Ledger 1\"",
        &format!("Batch: {BATCH}"),
        "reconciliation_required",
    ] {
        assert!(preview.contains(shown), "{shown}: {preview}");
    }
    assert!(preview.starts_with("Record that you reviewed ONE Journal"));
    assert!(
        preview.contains(&format!("Choosing \"{REVIEW_BUTTON}\"")),
        "names the button the platform shows: {preview}"
    );
}

/// Each cap refuses on its own: every fixture below exceeds exactly one, so
/// a cap that stopped being checked lets its fixture through.
#[test]
fn each_review_cap_refuses_on_its_own() {
    let wide_ledgers = |entries: usize| {
        let mut voucher = row(entries, "Paid");
        for (index, entry) in voucher.entries.iter_mut().enumerate() {
            entry.ledger = format!("{index:02}{}", "L".repeat(83));
        }
        voucher
    };
    let long_doubt = json!({"state":"posted_under_changed_masters","ledgers":["D".repeat(90)]});
    let cases = [
        ("line_width", row(2, &"n".repeat(120)), doubt()),
        ("lines", row(20, "Paid"), doubt()),
        (
            "characters",
            {
                let mut voucher = wide_ledgers(10);
                voucher.narration = Some("n".repeat(90));
                voucher
            },
            long_doubt,
        ),
    ];
    for (cap, voucher, doubt) in cases {
        let preview = review_preview(BATCH, MARKER, "Books", &doubt, &voucher);
        assert_eq!(preview, Err("ack_review_too_large".to_string()), "{cap}");
        let rendered = render_review_text(BATCH, MARKER, "Books", &doubt, &voucher).unwrap();
        assert_eq!(caps_exceeded(&rendered), [cap], "{cap}: only its own cap");
    }
}

#[test]
fn a_review_too_long_to_show_is_refused_not_truncated() {
    assert!(review_preview(BATCH, MARKER, "Books", &doubt(), &row(10, "Paid")).is_ok());
    assert_eq!(
        review_preview(BATCH, MARKER, "Books", &doubt(), &row(40, "Paid")),
        Err("ack_review_too_large".to_string())
    );
}

#[test]
fn a_value_with_a_line_break_or_hidden_character_is_refused() {
    // Each value from Tally or the doubt, carrying each kind of character:
    // a CR LF, a line separator (U+2028), a zero-width space and a
    // right-to-left override.
    for (bad, code) in [
        ("\r\n", "ack_review_layout_text"),
        ("\u{2028}", "ack_review_layout_text"),
        ("\u{200B}", "ack_review_format_text"),
        ("\u{202E}", "ack_review_format_text"),
    ] {
        let text = format!("Pa{bad}id");
        let mut ledger = row_json(2, "Paid");
        ledger["amounts"][0]["ledger"] = json!(text);
        let ledger: ReadVoucher = serde_json::from_value(ledger).unwrap();
        let named = json!({"state":"posted_under_changed_masters","ledgers":[text]});
        for (source, preview) in [
            (
                "narration",
                review_preview(BATCH, MARKER, "Books", &doubt(), &row(2, &text)),
            ),
            (
                "company",
                review_preview(BATCH, MARKER, &text, &doubt(), &row(2, "Paid")),
            ),
            (
                "doubt ledger",
                review_preview(BATCH, MARKER, "Books", &named, &row(2, "Paid")),
            ),
            (
                "entry ledger",
                review_preview(BATCH, MARKER, "Books", &doubt(), &ledger),
            ),
        ] {
            assert_eq!(preview, Err(code.to_string()), "{source} with {bad:?}");
        }
    }
}

/// Only this batch's own marker at the end of the narration is left out;
/// text after it, a second marker and another batch's marker are all shown,
/// because the record binds the whole narration.
#[test]
fn the_narration_is_shown_whole_except_this_batchs_trailing_marker() {
    let shown = |narration: &str| {
        let preview = review_preview(BATCH, MARKER, "Books", &doubt(), &row(2, narration)).unwrap();
        preview
            .split_once("Narration:\n  ")
            .unwrap()
            .1
            .lines()
            .next()
            .unwrap()
            .to_string()
    };
    assert_eq!(shown(&format!("Paid {MARKER}")), "\"Paid\"");
    assert_eq!(shown(MARKER), "\"\"");
    // A second marker stays visible: only the exact trailing one is left out.
    assert_eq!(
        shown(&format!("Paid {MARKER} {MARKER}")),
        format!("\"Paid {MARKER}\"")
    );
    let other = "[BRIDGE:00000000-0000-4000-8000-000000000000]";
    for narration in [
        format!("Paid {MARKER} added later"),
        format!("Paid {other}"),
        format!("Paid {other} {MARKER}").replace(&format!(" {MARKER}"), ""),
        format!("Paid{MARKER}"),
    ] {
        assert_eq!(shown(&narration), format!("{narration:?}"), "{narration}");
    }
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

/// A posted line, as `post_import` saves one: its marker is what
/// `admit_review` finds the voucher by.
fn posted_line() -> ImportLedgerLine {
    serde_json::from_value(json!({
        "batch_id":BATCH, "identity_scheme":"batch_v1",
        "company_guid":"61c6de69-1748-461c-ad3f-162cb949df9f",
        "txn_ids":["T1"],"date_from":"20260907","date_to":"20260907",
        "sha256":"", "built_at":"2026-09-06T21:40:26.641Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":8,"master_value":7},
        "vouchers":[{"bridge_txn_id":"T1","date":"20260907","voucher_type":"Journal",
            "narration":null,"reference":null,"voucher_number":null,
            "entries":[{"ledger":"Cash","amount":"1.00","side":"Dr"},{"ledger":"Sales","amount":"1.00","side":"Cr"}]}]
    }))
    .unwrap()
}

/// The guards `admit_review` keeps although `posted_verified` implies them
/// today: each refuses on its own if that ever stops being true.
#[test]
fn each_readback_guard_refuses_on_its_own_even_under_posted_verified() {
    let imports = tempfile::tempdir().unwrap();
    let line = posted_line();
    let doubt = br#"{"state":"posted_under_changed_masters","ledgers":["Cash"]}"#;
    fs::write(masters_doubt_path(imports.path(), BATCH), doubt).unwrap();
    fs::write(masters_check_path(imports.path(), BATCH), doubt).unwrap();
    let payload = json!({"result":{
        "dispatch":{"response_state":"response_clean"},
        "counts":{"posted_verified":1},"duplicates":[]
    }});
    let tag = line.attribution_tag(&line.vouchers[0]);
    let marked = || row_json(2, &format!("Paid [BRIDGE:{tag}]"));
    let admit = |row: Value| {
        let row: ReadVoucher = serde_json::from_value(row).unwrap();
        admit_review(imports.path(), &line, &payload, &[row]).map(|_| ())
    };
    assert_eq!(admit(marked()), Ok(()), "the control is admitted");
    for (pointer, value) in [
        ("/cancelled", json!(true)),
        ("/optional", json!(true)),
        ("/cancelled", Value::Null),
        ("/guid", Value::Null),
        ("/master_id", Value::Null),
        ("/alter_id", Value::Null),
    ] {
        let mut row = marked();
        *row.pointer_mut(pointer).unwrap() = value.clone();
        assert_eq!(
            admit(row),
            Err("ack_readback_not_matched".to_string()),
            "{pointer} = {value}"
        );
    }
}

/// Realistic lengths fit: a changed ledger with a long name, and a bank
/// narration carrying Bridge's marker, which the review does not repeat.
#[test]
fn a_long_ledger_name_and_a_marked_narration_fit_the_review() {
    let ledger = "Bridge Nested Debtor WR4 Long Registered Name Private Limited";
    let doubt = json!({"state":"posted_under_changed_masters","ledgers":[ledger, "Cash"]});
    let narration = "NEFT CR XXXX0001234 ACME TRADERS PVT LTD INV 2026-27/0045 AUG [BRIDGE:9c8d8de4-c06c-847b-8309-60ba702bf663]";
    let preview = review_preview(BATCH, MARKER, "Books", &doubt, &row(2, narration)).unwrap();
    assert!(preview.contains(&format!("  \"{ledger}\"")), "{preview}");
    assert!(preview.contains("INV 2026-27/0045 AUG\""), "{preview}");
    assert!(!preview.contains("[BRIDGE:"), "{preview}");
}
