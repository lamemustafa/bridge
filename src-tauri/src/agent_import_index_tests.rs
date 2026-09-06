use super::*;

#[test]
fn captured_derived_large_verification_preserves_tag_and_fallback_multiplicity() {
    // An in-memory workload derived from a captured accounting row. Repeated
    // identities, tags and Journal type below are synthetic, not live evidence.
    let mut template = parse_import_vouchers(&boundary_tests::captured_vouchers())
        .unwrap()
        .rows
        .remove(0);
    template.voucher_type = Some("Journal".into());
    template.voucher_number = None;
    let date = template.date.as_deref().unwrap();
    let expected = ImportVoucher {
        bridge_txn_id: String::new(),
        date: format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]),
        voucher_type: VoucherType::Journal,
        narration: None,
        reference: None,
        voucher_number: None,
        entries: template
            .entries
            .iter()
            .map(|entry| ImportEntry {
                ledger: entry.ledger.clone(),
                amount: entry.amount.trim_start_matches('-').into(),
                side: if entry.is_deemed_positive == "Yes" {
                    EntrySide::Dr
                } else {
                    EntrySide::Cr
                },
            })
            .collect(),
    };
    let vouchers = (0..1000)
        .map(|index| {
            let mut row = expected.clone();
            row.bridge_txn_id = format!("scale-{index}");
            row
        })
        .collect::<Vec<_>>();
    let line = ImportLedgerLine {
        batch_id: "scale-batch".into(),
        company_guid: GUID.into(),
        company: None,
        txn_ids: vouchers
            .iter()
            .map(|row| row.bridge_txn_id.clone())
            .collect(),
        date_from: date.into(),
        date_to: date.into(),
        sha256: "synthetic".into(),
        built_at: now(),
        status: "built".into(),
        pre_import_mark: PreImportMark {
            kind: "company_high_water".into(),
            value: Some(10),
            master_value: Some(10),
        },
        vouchers,
    };
    for tagged in [false, true] {
        let observed = (0..10000)
            .map(|index| {
                let mut row = template.clone();
                row.guid = Some(format!("scale-guid-{index}"));
                row.master_id = Some(index.to_string());
                row.remote_id = None;
                row.alter_id = Some(11);
                row.narration = (tagged && index < 1000).then(|| format!("[BRIDGE:scale-{index}]"));
                row
            })
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let result = verify_observed_batch(&line, &observed).unwrap();
        eprintln!(
            "scaled verification: expected=1000 observed=10000 tagged={tagged} elapsed_ms={}",
            started.elapsed().as_millis()
        );
        assert_eq!(
            result["counts"]["posted_verified"],
            if tagged { 1000 } else { 0 }
        );
        assert_eq!(
            result["counts"]["duplicate_fingerprint"],
            if tagged { 0 } else { 1000 }
        );
        assert_eq!(result["duplicates"].as_array().unwrap().len(), 1);
        assert_eq!(
            verification_status(&result, 1000),
            "verification_incomplete"
        );
        assert!(result["duplicates"][0]["fingerprint"].is_null());
        assert!(result["duplicates"][0]["fingerprint_sha256"].is_string());
    }
}

#[test]
fn delimiter_bearing_ledger_names_do_not_create_accounting_duplicates() {
    // Pure matching fault case; these rows do not claim a live Tally capture.
    let mut first = parse_import_vouchers(&boundary_tests::captured_vouchers())
        .unwrap()
        .rows
        .remove(0);
    first.entries = vec![
        ReadEntry {
            ledger: "A".into(),
            amount: "1".into(),
            is_deemed_positive: "No".into(),
        },
        ReadEntry {
            ledger: "B".into(),
            amount: "1".into(),
            is_deemed_positive: "No".into(),
        },
    ];
    first.remote_id = None;
    let mut second = first.clone();
    second.guid = Some("different-guid".into());
    second.master_id = Some("999".into());
    second.entries = vec![ReadEntry {
        ledger: "A|1|No,B".into(),
        amount: "1".into(),
        is_deemed_positive: "No".into(),
    }];
    let rows = vec![first, second];
    let fingerprints = rows.iter().map(observed_fingerprint).collect::<Vec<_>>();
    assert_ne!(fingerprints[0], fingerprints[1]);
    assert_eq!(fingerprints[0].2.join(","), fingerprints[1].2.join(","));
    let identities = rows
        .iter()
        .map(observed_voucher_identity)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let expected = BTreeMap::from([(&fingerprints[0], 1)]);
    let (batch, unrelated) = batch_duplicate_sets(
        &rows,
        &identities,
        &fingerprints,
        &expected,
        &[None, None],
        &BTreeSet::new(),
        &BTreeSet::new(),
    );
    assert!(
        batch.is_empty(),
        "distinct entry vectors must not block a verified batch"
    );
    assert!(unrelated.is_empty());
}
