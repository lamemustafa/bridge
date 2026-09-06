use super::*;

#[test]
fn missing_opening_is_partial_even_at_book_start() {
    let (row, partial) = ledger_movement_row(
        LedgerMovementRow {
            name: "Ledger".into(),
            parent: None,
            opening: None,
            debit: "-12".into(),
            credit: "4".into(),
            vouchers_touching: 2,
        },
        Redaction::None,
    )
    .unwrap();
    assert!(partial);
    assert_eq!(row["state"], "partial");
    assert_eq!(row["opening"], Value::Null);
    assert_eq!(row["closing"], Value::Null);
}

#[test]
fn movement_emptiness_uses_raw_presence_before_accounting_exclusions() {
    for (cancelled, optional) in [(true, false), (false, true), (true, true)] {
        let page = parse_movement_rows(
            vec![json!({"date":"20260901", "cancelled":cancelled,
            "optional":optional,"amounts":[]})],
            "20260901",
            "20260902",
        )
        .unwrap();
        assert_eq!(page.observed_rows, 1);
        assert_eq!(page.rows.is_empty(), cancelled || optional);
    }
    let empty = parse_movement_rows(vec![], "20260901", "20260902").unwrap();
    assert_eq!(empty.observed_rows, 0);
}

#[test]
fn movement_uses_observed_period_opening_without_inferring_account_classification() {
    for (opening, closing) in [("0", "12.5"), ("-12.5", "0")] {
        let (row, partial) = ledger_movement_row(
            LedgerMovementRow {
                name: "Unclassified ledger".into(),
                parent: None,
                opening: Some(opening.into()),
                debit: "0".into(),
                credit: "12.5".into(),
                vouchers_touching: 1,
            },
            Redaction::None,
        )
        .unwrap();
        assert!(!partial);
        assert_eq!(row["opening"], opening);
        assert_eq!(row["closing"], closing);
        assert_eq!(row["reason"], Value::Null);
    }
}

#[test]
fn active_entryless_movement_voucher_is_refused() {
    let result = parse_movement_rows(
        vec![json!({"date":"20260901", "cancelled":false,
            "optional":false,"amounts":[]})],
        "20260901",
        "20260902",
    );
    assert_eq!(
        result.err(),
        Some("ledger_movement_entries_not_observed".into())
    );
}

#[test]
fn movement_snapshot_rejects_renames_even_when_the_selected_name_returns() {
    let ledger = |name: &str| TallyLedger {
        name: name.into(),
        parent: Default::default(),
        party_gstin: Default::default(),
        opening_balance: Some("0".into()),
    };
    let opening = vec![ledger("Cash"), ledger("Sales")];
    let vouchers = |name: &str| {
        vec![MovementVoucher {
            date: "20260901".into(),
            cancelled: false,
            optional: false,
            ledger_entries: vec![MovementEntry {
                ledger_name: name.into(),
                amount: "-10".into(),
                is_deemed_positive: true,
            }],
        }]
    };
    let renamed = vec![ledger("New Cash"), ledger("Sales")];
    assert_eq!(
        validate_movement_snapshot(&opening, &renamed, &vouchers("New Cash")),
        Err("ledger_snapshot_drifted".into())
    );
    assert_eq!(
        validate_movement_snapshot(&opening, &opening, &vouchers("New Cash")),
        Err("ledger_snapshot_drifted".into())
    );
    // Known entries outside the selected Cash ledger remain admissible.
    assert_eq!(
        validate_movement_snapshot(&opening, &opening, &vouchers("Sales")),
        Ok(())
    );
    let mut changed_opening = opening.clone();
    changed_opening[0].opening_balance = Some("1".into());
    assert_eq!(
        validate_movement_snapshot(&opening, &changed_opening, &vouchers("Cash")),
        Err("ledger_snapshot_drifted".into())
    );
}
