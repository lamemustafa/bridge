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
    let other = "[BRIDGE:00000000-0000-4000-8000-000000000000]";
    // Another batch's marker stays visible before this batch's trailing one.
    assert_eq!(
        shown(&format!("Paid {other} {MARKER}")),
        format!("\"Paid {other}\"")
    );
    // A second marker stays visible: only the exact trailing one is left out.
    assert_eq!(
        shown(&format!("Paid {MARKER} {MARKER}")),
        format!("\"Paid {MARKER}\"")
    );
    for narration in [
        format!("Paid {MARKER} added later"),
        format!("Paid {other}"),
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
        admit_review(imports.path(), &line, &payload, &[row], DoubtKind::Masters).map(|_| ())
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

/// A posted batch of `count` Journals, T1..Tn, as `post_import` saves one.
fn posted_batch(count: usize) -> ImportLedgerLine {
    let mut line = posted_line();
    let first = line.vouchers[0].clone();
    line.vouchers = (1..=count)
        .map(|index| {
            let mut voucher = first.clone();
            voucher.bridge_txn_id = format!("T{index}");
            voucher
        })
        .collect();
    line.txn_ids = (1..=count).map(|index| format!("T{index}")).collect();
    line
}

/// The batch's vouchers as read back, each carrying its own marker.
fn batch_rows(line: &ImportLedgerLine) -> Vec<ReadVoucher> {
    line.vouchers
        .iter()
        .enumerate()
        .map(|(index, voucher)| {
            let tag = line.attribution_tag(voucher);
            let mut row = row_json(2, &format!("Paid [BRIDGE:{tag}]"));
            row["guid"] = json!(format!("g-{index}"));
            row["master_id"] = json!(format!("{}", 5 + index));
            row["alter_id"] = json!(10 + index as u64);
            serde_json::from_value(row).unwrap()
        })
        .collect()
}

const STEP_DOUBT: &[u8] = br#"{"state":"unmatched","target_voucher_step":{"before":10,"after":14,"step":4,"reported_created":3,"matches_created":false}}"#;
const MASTERS_DOUBT: &[u8] = br#"{"state":"posted_under_changed_masters","ledgers":["Cash"]}"#;

fn verified(count: usize) -> Value {
    json!({"result":{
        "dispatch":{"response_state":"response_clean"},
        "counts":{"posted_verified":count},"duplicates":[]
    }})
}

/// Which doubt a review is for: the named one, if the batch can hold it; the
/// one observed; never a guess between two.
#[test]
fn a_review_names_its_doubt_and_two_doubts_need_a_name() {
    use MastersRecord::{Doubt, NoDoubt, Pending};
    let doubt = || Doubt {
        raw: b"{}".to_vec(),
    };
    let both = [
        (DoubtKind::Masters, doubt()),
        (DoubtKind::BatchStep, doubt()),
    ];
    assert_eq!(select_doubt(None, &both), Err("ack_doubt_ambiguous"));
    assert_eq!(
        select_doubt(Some(DoubtKind::BatchStep), &both),
        Ok(DoubtKind::BatchStep)
    );
    let step_only = [
        (DoubtKind::Masters, NoDoubt),
        (DoubtKind::BatchStep, doubt()),
    ];
    assert_eq!(select_doubt(None, &step_only), Ok(DoubtKind::BatchStep));
    // A doubt held only by the check record is observed too (#722): beside
    // another doubt it needs a name, and alone it is the one chosen.
    let masters_unavailable = [
        (DoubtKind::Masters, MastersRecord::DoubtRecordUnavailable),
        (DoubtKind::BatchStep, doubt()),
    ];
    assert_eq!(
        select_doubt(None, &masters_unavailable),
        Err("ack_doubt_ambiguous")
    );
    // The mirror: a masters doubt with its file, beside a step doubt without.
    let step_unavailable_beside = [
        (DoubtKind::Masters, doubt()),
        (DoubtKind::BatchStep, MastersRecord::DoubtRecordUnavailable),
    ];
    assert_eq!(
        select_doubt(None, &step_unavailable_beside),
        Err("ack_doubt_ambiguous")
    );
    // Both held only by the check record: no name could be reviewed, so the
    // first is chosen and refused as unavailable, not as ambiguous.
    let both_unavailable = [
        (DoubtKind::Masters, MastersRecord::DoubtRecordUnavailable),
        (DoubtKind::BatchStep, MastersRecord::DoubtRecordUnavailable),
    ];
    assert_eq!(
        select_doubt(None, &both_unavailable),
        Ok(DoubtKind::Masters)
    );
    let step_unavailable = [
        (DoubtKind::Masters, NoDoubt),
        (DoubtKind::BatchStep, MastersRecord::DoubtRecordUnavailable),
    ];
    assert_eq!(
        select_doubt(None, &step_unavailable),
        Ok(DoubtKind::BatchStep)
    );
    // A single post can hold no step doubt.
    let single = [(DoubtKind::Masters, doubt())];
    assert_eq!(select_doubt(None, &single), Ok(DoubtKind::Masters));
    assert_eq!(
        select_doubt(Some(DoubtKind::BatchStep), &single),
        Err("ack_no_observed_doubt")
    );
    // None observed: the kind whose record says why is the one refused.
    let pending = [
        (DoubtKind::Masters, NoDoubt),
        (DoubtKind::BatchStep, Pending),
    ];
    assert_eq!(select_doubt(None, &pending), Ok(DoubtKind::BatchStep));
    let none = [
        (DoubtKind::Masters, NoDoubt),
        (DoubtKind::BatchStep, NoDoubt),
    ];
    assert_eq!(select_doubt(None, &none), Ok(DoubtKind::Masters));
}

/// A batch review binds every voucher in batch order, and is refused unless
/// every one reads back verified: a partial read is reconciled first.
#[test]
fn a_batch_review_binds_every_voucher_and_refuses_a_partial_read() {
    let imports = tempfile::tempdir().unwrap();
    let line = posted_batch(3);
    let rows = batch_rows(&line);
    fs::write(
        super::batch_step_doubt_path(imports.path(), BATCH),
        STEP_DOUBT,
    )
    .unwrap();
    let shown = admit_review(
        imports.path(),
        &line,
        &verified(3),
        &rows,
        DoubtKind::BatchStep,
    )
    .unwrap();
    assert_eq!(shown.doubt_raw, STEP_DOUBT);
    assert_eq!(
        shown
            .vouchers
            .iter()
            .map(|voucher| (voucher.bridge_txn_id.as_str(), voucher.alter_id))
            .collect::<Vec<_>>(),
        [("T1", 10), ("T2", 11), ("T3", 12)]
    );
    let refused = |payload: &Value, rows: &[ReadVoucher], kind| {
        admit_review(imports.path(), &line, payload, rows, kind).err()
    };
    assert_eq!(
        refused(&verified(2), &rows, DoubtKind::BatchStep).as_deref(),
        Some("ack_readback_not_matched")
    );
    assert_eq!(
        refused(&verified(3), &rows[..2], DoubtKind::BatchStep).as_deref(),
        Some("ack_readback_not_matched")
    );
    // No masters doubt is recorded, so a masters review has nothing to cover.
    assert_eq!(
        refused(&verified(3), &rows, DoubtKind::Masters).as_deref(),
        Some("ack_no_observed_doubt")
    );
}

/// The batch dialog shows the doubt and the vouchers as read, in totals, and
/// is refused rather than cut when it does not fit.
#[test]
fn the_batch_review_summarizes_the_doubt_and_the_vouchers_as_read() {
    let line = posted_batch(3);
    let rows = batch_rows(&line);
    let rows = rows.iter().collect::<Vec<_>>();
    let step: Value = serde_json::from_slice(STEP_DOUBT).unwrap();
    let preview = batch_review_preview(&line, DoubtKind::BatchStep, "Books", &step, &rows).unwrap();
    for shown in [
        "Record that you reviewed 3 vouchers in \"Books\"",
        "its voucher mark moved by 4 (from 10 to 14); Tally reported creating 3.",
        "Not shown here: narrations, voucher numbers and types.",
        "Reviewing these vouchers covers no other voucher in this company.",
        "Dates: 20260907 to 20260907  ALTERIDs: 10 to 12",
        "Dr 3  Cr 0  3 entries  \"Ledger 0\"",
        &format!("Batch: {BATCH}"),
        "I reviewed these 3 vouchers in Tally.",
        "reconciliation_required",
    ] {
        assert!(preview.contains(shown), "{shown}: {preview}");
    }
    let masters: Value = serde_json::from_slice(MASTERS_DOUBT).unwrap();
    let preview =
        batch_review_preview(&line, DoubtKind::Masters, "Books", &masters, &rows).unwrap();
    assert!(
        preview.contains("these ledgers no longer resolve"),
        "{preview}"
    );
    assert!(preview.contains("  \"Cash\""), "{preview}");
    // Too many ledgers for one dialog.
    let wide = (0..40)
        .map(|index| {
            let mut row = row_json(1, "wide");
            row["amounts"][0]["ledger"] = json!(format!("Wide ledger {index}"));
            serde_json::from_value::<ReadVoucher>(row).unwrap()
        })
        .collect::<Vec<_>>();
    let wide = wide.iter().collect::<Vec<_>>();
    assert_eq!(
        batch_review_preview(&line, DoubtKind::BatchStep, "Books", &step, &wide)
            .err()
            .as_deref(),
        Some("ack_review_too_large")
    );
}

/// A step doubt is shown in its own words for each way it arises: a mark
/// never read, a mark that went backwards, and an answer that did not parse
/// are never shown as a count.
#[test]
fn each_kind_of_step_doubt_is_shown_in_its_own_words() {
    let line = posted_batch(2);
    let rows = batch_rows(&line);
    let rows = rows.iter().collect::<Vec<_>>();
    for (step, said, never) in [
        (
            Value::Null,
            "changed this company's vouchers:\nBridge could not read its voucher mark after posting.\n",
            "Tally reported",
        ),
        (
            json!({"before":14,"after":10,"step":null,"reported_created":2}),
            "its voucher mark went backwards (from 14 to 10); Tally reported creating 2.",
            "moved by",
        ),
        (
            json!({"before":10,"after":13,"step":3,"reported_created":null}),
            "its voucher mark moved by 3 (from 10 to 13); Tally's answer could not be read.",
            "reported creating",
        ),
    ] {
        let doubt = json!({"state":"unmatched","target_voucher_step":step});
        let preview =
            batch_review_preview(&line, DoubtKind::BatchStep, "Books", &doubt, &rows).unwrap();
        assert!(preview.contains(said), "{said}: {preview}");
        assert!(!preview.contains(never), "{never}: {preview}");
    }
}

/// A batch's reviews are reported per doubt. A review covers only the doubt it
/// names, and turns stale, naming the voucher, when any one of its vouchers
/// changes.
#[test]
fn a_batch_review_covers_only_its_own_doubt_and_names_a_changed_voucher() {
    let imports = tempfile::tempdir().unwrap();
    let line = posted_batch(2);
    let rows = batch_rows(&line);
    fs::write(masters_doubt_path(imports.path(), BATCH), MASTERS_DOUBT).unwrap();
    fs::write(
        super::batch_step_doubt_path(imports.path(), BATCH),
        STEP_DOUBT,
    )
    .unwrap();
    let shown = admit_review(
        imports.path(),
        &line,
        &verified(2),
        &rows,
        DoubtKind::Masters,
    )
    .unwrap();
    let record = BatchAckRecord {
        version: BATCH_RECORD_VERSION,
        batch_id: BATCH.into(),
        company_guid: line.company_guid.clone(),
        doubt: "masters".into(),
        doubt_sha256: sha256_hex(MASTERS_DOUBT),
        vouchers: shown.vouchers,
        voucher_fingerprint_fields: FINGERPRINT_FIELDS.into(),
        shown: json!([]),
        reviewed_at: "2026-09-26T00:00:00.000Z".into(),
        local_account_label: None,
    };
    fs::write(
        DoubtKind::Masters.ack_path(imports.path(), BATCH),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let review = operator_review(imports.path(), &line, &rows).unwrap();
    assert_eq!(review["masters"]["state"], "current", "{review}");
    assert_eq!(review["batch_step"]["state"], "absent", "{review}");
    // A masters review does not cover the step doubt, even filed under it.
    fs::copy(
        DoubtKind::Masters.ack_path(imports.path(), BATCH),
        DoubtKind::BatchStep.ack_path(imports.path(), BATCH),
    )
    .unwrap();
    let review = operator_review(imports.path(), &line, &rows).unwrap();
    assert_eq!(review["batch_step"]["state"], "stale", "{review}");
    assert_eq!(review["batch_step"]["covers_doubt"], false, "{review}");
    // Even bound to the step doubt's own bytes, a record that names another
    // doubt does not cover it.
    let mut misnamed = serde_json::from_slice::<BatchAckRecord>(
        &fs::read(DoubtKind::Masters.ack_path(imports.path(), BATCH)).unwrap(),
    )
    .unwrap();
    misnamed.doubt_sha256 = sha256_hex(STEP_DOUBT);
    fs::write(
        DoubtKind::BatchStep.ack_path(imports.path(), BATCH),
        serde_json::to_vec(&misnamed).unwrap(),
    )
    .unwrap();
    let review = operator_review(imports.path(), &line, &rows).unwrap();
    assert_eq!(review["batch_step"]["covers_doubt"], false, "{review}");
    misnamed.doubt = "batch_step".into();
    fs::write(
        DoubtKind::BatchStep.ack_path(imports.path(), BATCH),
        serde_json::to_vec(&misnamed).unwrap(),
    )
    .unwrap();
    let review = operator_review(imports.path(), &line, &rows).unwrap();
    assert_eq!(review["batch_step"]["state"], "current", "{review}");
    // A record for the same vouchers in another order covers nothing.
    misnamed.vouchers.reverse();
    fs::write(
        DoubtKind::BatchStep.ack_path(imports.path(), BATCH),
        serde_json::to_vec(&misnamed).unwrap(),
    )
    .unwrap();
    let review = operator_review(imports.path(), &line, &rows).unwrap();
    assert_eq!(review["batch_step"]["covers_doubt"], false, "{review}");
    misnamed.vouchers.reverse();
    fs::write(
        DoubtKind::BatchStep.ack_path(imports.path(), BATCH),
        serde_json::to_vec(&misnamed).unwrap(),
    )
    .unwrap();
    // One voucher altered: the masters review is stale and names it.
    let mut edited = rows.clone();
    edited[1].alter_id = Some(99);
    let review = operator_review(imports.path(), &line, &edited).unwrap();
    assert_eq!(review["masters"]["state"], "stale", "{review}");
    assert_eq!(
        review["masters"]["changed_vouchers"],
        json!(["T2"]),
        "{review}"
    );
    // A review whose doubt file has gone is stale, not silently absent.
    fs::remove_file(super::batch_step_doubt_path(imports.path(), BATCH)).unwrap();
    let review = operator_review(imports.path(), &line, &rows).unwrap();
    assert_eq!(review["batch_step"]["state"], "stale", "{review}");
    assert_eq!(review["batch_step"]["covers_doubt"], false, "{review}");
    assert_eq!(review["masters"]["state"], "current", "{review}");
}

/// A kind whose verdict is not yet recorded reads `pending`, not `null`: the
/// verdict counts it as doubt, so the review reports it rather than hide it.
#[test]
fn a_batch_kind_whose_verdict_is_pending_reads_pending() {
    let imports = tempfile::tempdir().unwrap();
    let line = posted_batch(2);
    let rows = batch_rows(&line);
    let check = |masters: &str, step: &str| {
        fs::write(
            masters_check_path(imports.path(), BATCH),
            serde_json::to_vec(&json!({"state":masters,"batch_step":{"state":step}})).unwrap(),
        )
        .unwrap();
        operator_review(imports.path(), &line, &rows)
    };
    let review = check("unchanged", MASTERS_CHECK_PENDING).unwrap();
    assert_eq!(review["batch_step"], json!({"state":"pending"}), "{review}");
    assert_eq!(review["masters"], Value::Null, "{review}");
    let review = check(MASTERS_CHECK_PENDING, MASTERS_CHECK_PENDING).unwrap();
    assert_eq!(review["masters"], json!({"state":"pending"}), "{review}");
    // Control: both recorded, no doubt observed, nothing to report.
    assert_eq!(check("unchanged", "matched"), None);
}

/// A doubt the check record holds whose own file is absent reads
/// `doubt_record_unavailable`, not `null` (#722): the verdict counts it as
/// doubt, and no review can bind to it. For each kind, and for one voucher.
#[test]
fn a_doubt_whose_own_file_was_not_written_reads_doubt_record_unavailable() {
    let imports = tempfile::tempdir().unwrap();
    let unavailable = json!({"state":"doubt_record_unavailable"});
    let check = |line: &ImportLedgerLine, masters: &str, step: &str| {
        fs::write(
            masters_check_path(imports.path(), BATCH),
            serde_json::to_vec(&json!({"state":masters,"batch_step":{"state":step}})).unwrap(),
        )
        .unwrap();
        operator_review(imports.path(), line, &batch_rows(line))
    };
    let batch = posted_batch(2);
    let review = check(&batch, "posted_under_changed_masters", "matched").unwrap();
    assert_eq!(review["masters"], unavailable, "{review}");
    assert_eq!(review["batch_step"], Value::Null, "{review}");
    let review = check(&batch, "unchanged", "unmatched").unwrap();
    assert_eq!(review["batch_step"], unavailable, "{review}");
    assert_eq!(review["masters"], Value::Null, "{review}");
    let single = posted_batch(1);
    assert_eq!(
        check(&single, "posted_under_changed_masters", "matched"),
        Some(unavailable)
    );
    // A review already recorded outranks the absent file: it reads stale,
    // as a review whose doubt can no longer be read does, for each kind.
    for (kind, masters, step) in [
        (
            DoubtKind::Masters,
            "posted_under_changed_masters",
            "matched",
        ),
        (DoubtKind::BatchStep, "unchanged", "unmatched"),
    ] {
        fs::write(kind.ack_path(imports.path(), BATCH), b"{}").unwrap();
        let review = check(&batch, masters, step).unwrap();
        assert_eq!(review[kind.name()]["state"], "stale", "{review}");
        fs::remove_file(kind.ack_path(imports.path(), BATCH)).unwrap();
    }
    fs::write(masters_ack_path(imports.path(), BATCH), b"{}").unwrap();
    let review = check(&single, "posted_under_changed_masters", "matched").unwrap();
    assert_eq!(review["state"], "stale", "{review}");
    fs::remove_file(masters_ack_path(imports.path(), BATCH)).unwrap();
    // Control: with each doubt's own file written, the review reads it.
    fs::write(
        super::masters_doubt_path(imports.path(), BATCH),
        MASTERS_DOUBT,
    )
    .unwrap();
    let review = check(&batch, "posted_under_changed_masters", "matched").unwrap();
    assert_eq!(review["masters"]["state"], "absent", "{review}");
    assert_eq!(
        check(&single, "posted_under_changed_masters", "matched").unwrap()["state"],
        "absent"
    );
    fs::write(
        super::batch_step_doubt_path(imports.path(), BATCH),
        STEP_DOUBT,
    )
    .unwrap();
    let review = check(&batch, "unchanged", "unmatched").unwrap();
    assert_eq!(review["batch_step"]["state"], "absent", "{review}");
}

/// A debit total is the negated sum of what Tally holds, never each line's
/// magnitude: a debit line with an unexpected sign still shows as it is.
#[test]
fn a_debit_total_is_the_negated_sum_not_each_lines_magnitude() {
    let line = posted_batch(2);
    let mut rows = batch_rows(&line);
    for (row, amount) in rows.iter_mut().zip(["-5.00", "2.00"]) {
        row.entries[0].amount = amount.into();
        row.entries[0].ledger = "Odd".into();
    }
    let rows = rows.iter().collect::<Vec<_>>();
    let step: Value = serde_json::from_slice(STEP_DOUBT).unwrap();
    let preview = batch_review_preview(&line, DoubtKind::BatchStep, "Books", &step, &rows).unwrap();
    assert!(
        preview.contains("Dr 3  Cr 0  2 entries  \"Odd\""),
        "{preview}"
    );
}
