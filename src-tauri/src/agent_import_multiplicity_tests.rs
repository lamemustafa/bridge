use super::*;

fn identical_batch() -> (ImportLedgerLine, Vec<ReadVoucher>) {
    let first = payload().vouchers.remove(0);
    let mut second = first.clone();
    second.bridge_txn_id = "txn-identical".into();
    let vouchers = vec![first, second];
    let observed = vouchers
        .iter()
        .enumerate()
        .map(|(index, voucher)| ReadVoucher {
            remote_id: Some(format!("remote-{index}")),
            guid: Some(format!("guid-{index}")),
            master_id: Some(index.to_string()),
            alter_id: Some(11 + index as u64),
            date: Some(normalized_date(&voucher.date).unwrap()),
            voucher_type: Some(voucher.voucher_type.as_str().into()),
            narration: Some(format!("[BRIDGE:{}]", voucher.bridge_txn_id)),
            voucher_number: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: voucher
                .entries
                .iter()
                .map(|entry| ReadEntry {
                    ledger: entry.ledger.clone(),
                    amount: match entry.side {
                        EntrySide::Dr => format!("-{}", entry.amount),
                        EntrySide::Cr => entry.amount.clone(),
                    },
                    is_deemed_positive: entry.side.tally_positive().into(),
                })
                .collect(),
        })
        .collect();
    let line = ImportLedgerLine {
        batch_id: "batch-identical".into(),
        company_guid: GUID.into(),
        company: None,
        txn_ids: vouchers.iter().map(|v| v.bridge_txn_id.clone()).collect(),
        date_from: "20260901".into(),
        date_to: "20260901".into(),
        sha256: "hash".into(),
        built_at: now(),
        status: "built".into(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".into(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers,
    };
    (line, observed)
}

#[test]
fn identical_expected_vouchers_verify_only_with_unique_full_attribution() {
    let (mut line, mut observed) = identical_batch();
    for reverse in [false, true] {
        if reverse {
            line.vouchers.reverse();
            observed.reverse();
        }
        let result = verify_observed_batch(&line, &observed).unwrap();
        assert_eq!(result["counts"]["posted_verified"], 2);
        assert_eq!(result["duplicates"], json!([]));
        assert_eq!(result["unrelated_duplicates_in_window"], json!([]));
        assert_eq!(verification_status(&result, 2), "posted_verified");
    }
    for extra_tag in [None, observed[0].narration.clone()] {
        let mut extra = observed[0].clone();
        extra.guid = Some("unexpected-guid".into());
        extra.master_id = Some("999".into());
        extra.remote_id = Some("unexpected-remote".into());
        let has_tag = extra_tag.is_some();
        extra.narration = extra_tag;
        if has_tag {
            assert_eq!(
                verify_observed_batch(&line, &[observed.clone(), vec![extra]].concat()),
                Err("import_verification_tag_ambiguous".into())
            );
            continue;
        }
        let result =
            verify_observed_batch(&line, &[observed.clone(), vec![extra]].concat()).unwrap();
        assert!(!result["duplicates"].as_array().unwrap().is_empty());
        assert_eq!(verification_status(&result, 2), "verification_incomplete");
    }
    let mut untagged = observed.clone();
    untagged[1].narration = None;
    let result = verify_observed_batch(&line, &untagged).unwrap();
    assert_eq!(result["counts"]["posted_verified"], 1);
    assert_eq!(verification_status(&result, 2), "verification_incomplete");
    assert!(!result["duplicates"].as_array().unwrap().is_empty());
    let mut duplicate_remote = observed.clone();
    duplicate_remote[1].remote_id = duplicate_remote[0].remote_id.clone();
    let result = verify_observed_batch(&line, &duplicate_remote).unwrap();
    assert!(result["duplicates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["kind"] == "remote_id"));
    assert_eq!(verification_status(&result, 2), "verification_incomplete");
    let mut repeated_identity = observed.clone();
    repeated_identity[1].guid = repeated_identity[0].guid.clone();
    assert_eq!(
        verify_observed_batch(&line, &repeated_identity),
        Err("import_verification_identity_invalid".into())
    );
}

#[test]
fn overlapping_expected_tags_remain_ambiguous_in_both_payload_orders() {
    let (mut line, mut observed) = identical_batch();
    observed[0].narration = Some(format!(
        "[BRIDGE:{}] [BRIDGE:{}]",
        line.vouchers[0].bridge_txn_id, line.vouchers[1].bridge_txn_id
    ));
    for source in [&observed[..1], &observed[..]] {
        for reverse in [false, true] {
            if reverse {
                line.vouchers.reverse();
            }
            assert_eq!(
                verify_observed_batch(&line, source),
                Err("import_verification_tag_ambiguous".into())
            );
        }
    }
}

#[test]
fn repeated_transaction_tags_across_pre_and_post_mark_rows_refuse_attribution() {
    let (mut line, mut observed) = identical_batch();
    line.vouchers.truncate(1);
    observed.truncate(1);
    let mut earlier = observed[0].clone();
    earlier.guid = Some("pre-mark-other-guid".into());
    earlier.master_id = Some("999".into());
    earlier.remote_id = Some("pre-mark-other-remote".into());
    earlier.alter_id = Some(1);
    earlier.entries[0].amount = "-99.99".into();
    for reverse in [false, true] {
        let mut source = vec![earlier.clone(), observed[0].clone()];
        if reverse {
            source.reverse();
        }
        assert_eq!(
            verify_observed_batch(&line, &source),
            Err("import_verification_tag_ambiguous".into())
        );
    }
}
