//! Admission tests for locally generated batches, not authored Tally responses.
use super::*;
use bridge_tally_transport::TallyEndpointConfig;

fn batch() -> (ImportLedgerLine, TallyEndpointConfig) {
    let endpoint = TallyEndpointConfig {
        host: "127.0.0.1".into(),
        port: 9001,
    };
    let mut line: ImportLedgerLine = serde_json::from_value(json!({
        "batch_id":"bridge-00000000-0000-4000-8000-000000000001", "identity_scheme":"batch_v1",
        "company_guid":"00000000-0000-4000-8000-000000000002",
        "endpoint_origin":super::super::super::canonical_loopback_origin(&endpoint).unwrap(),
        "company":{"name":"Synthetic Accounts","guid":"00000000-0000-4000-8000-000000000002","company_number":"100001","books_from":"20260401"},
        "txn_ids":["journal-test"],"date_from":"20260901","date_to":"20260901",
        "sha256":"", "built_at":"2026-09-07T00:00:00Z", "status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"journal-test","date":"20260901","voucher_type":"Journal",
            "narration":"Synthetic test only","reference":"REF-1","entries":[
                {"ledger":"Expense","amount":"12.50","side":"Dr"},
                {"ledger":"Cash","amount":"12.50","side":"Cr"}]}]
    })).unwrap();
    line.sha256 = sha256_hex(
        render_import_xml("Synthetic Accounts", &line.vouchers, &line.batch_id).as_bytes(),
    );
    (line, endpoint)
}

#[test]
fn native_preview_contains_all_accounting_inputs_and_pinned_destination() {
    let (line, endpoint) = batch();
    let (_, preview) = admit_saved_journal(&line, &endpoint).unwrap();
    for field in [
        "Synthetic Accounts",
        "9001",
        "20260901",
        "Expense",
        "Cash",
        "12.50",
        "Dr",
        "Cr",
        "REF-1",
        "Synthetic test only",
        &line.batch_id,
    ] {
        assert!(preview.contains(field), "missing {field}");
    }
}

#[test]
fn old_or_changed_batches_cannot_be_posted() {
    let (mut line, mut endpoint) = batch();
    endpoint.port = 9002;
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_post_endpoint_mismatch"
    );
    endpoint.port = 9001;
    line.endpoint_origin = None;
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_post_endpoint_mismatch"
    );
    let (mut line, endpoint) = batch();
    line.vouchers[0].narration = Some("Changed after build".into());
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_batch_changed"
    );
}

#[test]
fn multiple_or_unqualified_vouchers_require_manual_workflow() {
    let (mut line, endpoint) = batch();
    line.vouchers.push(line.vouchers[0].clone());
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_post_requires_one_journal"
    );
    line.vouchers.pop();
    line.vouchers[0].voucher_type = VoucherType::Payment;
    assert_eq!(
        admit_saved_journal(&line, &endpoint).unwrap_err(),
        "import_post_requires_one_journal"
    );
}

fn refresh_batch_sha256(line: &mut ImportLedgerLine) {
    line.sha256 = sha256_hex(
        render_import_xml(
            &line.company.as_ref().unwrap().name,
            &line.vouchers,
            &line.batch_id,
        )
        .as_bytes(),
    );
}

#[test]
fn native_preview_refuses_line_and_paragraph_separators_in_every_operator_text_field() {
    for separator in ['\u{2028}', '\u{2029}'] {
        for field in ["narration", "reference", "ledger"] {
            let (mut line, endpoint) = batch();
            let value = format!("safe{separator}hidden");
            match field {
                "narration" => line.vouchers[0].narration = Some(value),
                "reference" => line.vouchers[0].reference = Some(value),
                "ledger" => line.vouchers[0].entries[0].ledger = value,
                _ => unreachable!(),
            }
            refresh_batch_sha256(&mut line);
            assert_eq!(
                admit_saved_journal(&line, &endpoint).unwrap_err(),
                "import_review_layout_text",
                "{field} {separator:?}"
            );
        }
    }
}

#[test]
fn native_preview_refuses_directional_marks_in_every_operator_text_field() {
    for mark in [
        '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
        '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        for field in ["narration", "reference", "ledger"] {
            let (mut line, endpoint) = batch();
            let value = format!("safe{mark}hidden");
            match field {
                "narration" => line.vouchers[0].narration = Some(value),
                "reference" => line.vouchers[0].reference = Some(value),
                "ledger" => line.vouchers[0].entries[0].ledger = value,
                _ => unreachable!(),
            }
            refresh_batch_sha256(&mut line);
            assert_eq!(
                admit_saved_journal(&line, &endpoint).unwrap_err(),
                "import_review_directional_text",
                "{field} {mark:?}"
            );
        }
    }
}

#[test]
fn absence_is_required_before_a_first_attempt() {
    for payload in [
        json!({}),
        json!({"result":{"counts":{"posted_verified":1},"vouchers":[{}]}}),
        json!({"result":{"counts":{"not_found":1},"vouchers":[]}}),
    ] {
        assert_eq!(
            require_absent(&payload).unwrap_err(),
            "import_preexisting_identity"
        );
    }
    assert!(require_absent(&json!({"result":{"counts":{"not_found":1},"vouchers":[{}]}})).is_ok());
}
