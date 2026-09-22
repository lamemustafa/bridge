use super::*;
use bridge_tally_protocol::native_trial_balance::{
    parse_native_trial_balance, NativeTrialBalance, NativeTrialBalanceAmount, NativeTrialBalanceRow,
};

fn empty_parent_row(parent: PartyLedgerMasterFieldObservation) -> NativeTrialBalanceRow {
    NativeTrialBalanceRow {
        name: String::new(),
        guid: String::new(),
        parent,
        opening: NativeTrialBalanceAmount::PresentEmpty,
        debit: NativeTrialBalanceAmount::PresentEmpty,
        credit: NativeTrialBalanceAmount::PresentEmpty,
        closing: NativeTrialBalanceAmount::PresentEmpty,
        currency_name: None,
    }
}

#[test]
fn captured_opening_difference_is_retained_without_a_balancing_row() {
    let xml = include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_opening_year.xml"
    );
    let report = parse_native_trial_balance(xml, "915d42f8-42ae-4b03-8291-55f596e3a2ea").unwrap();
    let totals = observed_totals(&report).unwrap();
    assert_eq!(report.rows.len(), 8);
    assert!(totals
        .opening
        .sum
        .numeric_eq(&ExactDecimal::parse("-49833.50").unwrap()));
    assert_eq!(totals.opening.empty_count, 0);
    assert!(totals.debit.empty_count > 0);
    assert!(totals
        .debit
        .sum
        .checked_add(&totals.credit.sum)
        .unwrap()
        .numeric_eq(&ExactDecimal::zero()));
}

#[test]
fn parent_query_keeps_exact_text_and_distinguishes_empty_from_not_observed() {
    let mut report = parse_native_trial_balance(
        include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
        ),
        "eebb9a9f-1679-4468-9e8f-814c729674cb",
    )
    .unwrap();
    let sundry_debtors = query_observed_parent(
        &report,
        &PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".into()),
    )
    .unwrap();
    assert_eq!(sundry_debtors.source_row_count, 6);
    assert_eq!(sundry_debtors.selected_rows.len(), 3);
    assert!(sundry_debtors
        .totals
        .opening
        .sum
        .numeric_eq(&ExactDecimal::parse("-4400.00").unwrap()));
    assert_eq!(sundry_debtors.totals.opening.empty_count, 0);
    assert!(sundry_debtors
        .totals
        .debit
        .sum
        .numeric_eq(&ExactDecimal::parse("-4777.00").unwrap()));
    assert_eq!(sundry_debtors.totals.debit.empty_count, 2);
    assert!(sundry_debtors
        .totals
        .credit
        .sum
        .numeric_eq(&ExactDecimal::parse("5700.00").unwrap()));
    assert_eq!(sundry_debtors.totals.credit.empty_count, 1);
    assert!(sundry_debtors
        .totals
        .closing
        .sum
        .numeric_eq(&ExactDecimal::parse("-3477.00").unwrap()));
    assert_eq!(sundry_debtors.totals.closing.empty_count, 1);

    let unobserved_guid = report.rows[0].guid.clone();
    let empty_guid = report.rows[1].guid.clone();
    report.rows[0].parent = PartyLedgerMasterFieldObservation::NotObserved;
    report.rows[1].parent = PartyLedgerMasterFieldObservation::Returned(String::new());

    let unobserved =
        query_observed_parent(&report, &PartyLedgerMasterFieldObservation::NotObserved).unwrap();
    assert_eq!(unobserved.selected_rows.len(), 1);
    assert_eq!(unobserved.selected_rows[0].guid, unobserved_guid);

    let empty = query_observed_parent(
        &report,
        &PartyLedgerMasterFieldObservation::Returned(String::new()),
    )
    .unwrap();
    assert_eq!(empty.selected_rows.len(), 1);
    assert_eq!(empty.selected_rows[0].guid, empty_guid);
    assert_ne!(empty.parent, unobserved.parent);

    let marked_parent = report
        .rows
        .iter()
        .find(|row| row.name == "Profit & Loss A/c")
        .unwrap()
        .parent
        .clone();
    assert!(matches!(
        &marked_parent,
        PartyLedgerMasterFieldObservation::Returned(value) if value.starts_with('\u{fffd}')
    ));
    let marked = query_observed_parent(&report, &marked_parent).unwrap();
    assert_eq!(marked.selected_rows.len(), 1);
    assert_eq!(marked.selected_rows[0].parent, marked_parent);
    assert_ne!(
        marked.parent,
        PartyLedgerMasterFieldObservation::Returned("Primary".into())
    );

    assert_eq!(
        query_observed_parent(
            &report,
            &PartyLedgerMasterFieldObservation::Returned("sundry debtors".into()),
        )
        .unwrap_err(),
        TrialBalanceParentQueryError::ParentNotInCapture,
    );
}

#[test]
fn captured_parent_options_rank_without_normalizing_observed_parent_text() {
    let mut report = parse_native_trial_balance(
        include_str!(
            "../../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
        ),
        "eebb9a9f-1679-4468-9e8f-814c729674cb",
    )
    .unwrap();
    let captured = list_observed_capture_parents(&report, "sundry debtors".into()).unwrap();
    assert_eq!(captured.source_row_count, 6);
    assert!(!captured.has_more);
    assert_eq!(captured.options.len(), 1);
    assert_eq!(
        captured.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".into())
    );
    assert_eq!(captured.options[0].row_count, 3);

    report.rows[0].parent = PartyLedgerMasterFieldObservation::Returned("alpha".into());
    report.rows[1].parent = PartyLedgerMasterFieldObservation::Returned("alpha".into());
    report.rows[2].parent = PartyLedgerMasterFieldObservation::Returned("ALPHA".into());
    report.rows[3].parent =
        PartyLedgerMasterFieldObservation::Returned("prefix alpha suffix".into());
    report.rows[4].parent = PartyLedgerMasterFieldObservation::NotObserved;
    report.rows[5].parent = PartyLedgerMasterFieldObservation::Returned(String::new());

    let ranked = list_observed_capture_parents(&report, "alpha".into()).unwrap();
    assert_eq!(ranked.options.len(), 3);
    assert_eq!(ranked.options[0].row_count, 2);
    assert_eq!(
        ranked.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("alpha".into())
    );
    assert_eq!(
        ranked.options[1].parent,
        PartyLedgerMasterFieldObservation::Returned("ALPHA".into())
    );
    assert_eq!(
        ranked.options[2].parent,
        PartyLedgerMasterFieldObservation::Returned("prefix alpha suffix".into())
    );

    let blank = list_observed_capture_parents(&report, String::new()).unwrap();
    assert_eq!(
        blank.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("alpha".into())
    );
    assert!(blank
        .options
        .iter()
        .any(|option| option.parent == PartyLedgerMasterFieldObservation::NotObserved));
    let group = list_observed_capture_parents(&report, "Group: alpha".into()).unwrap();
    assert_eq!(group.options.len(), 2);
    assert_eq!(
        group.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("alpha".into())
    );
    assert_eq!(
        group.options[1].parent,
        PartyLedgerMasterFieldObservation::Returned("ALPHA".into())
    );

    report.rows[2].parent =
        PartyLedgerMasterFieldObservation::Returned(RETURNED_EMPTY_PARENT_LABEL.into());
    report.rows[3].parent =
        PartyLedgerMasterFieldObservation::Returned(NOT_RETURNED_PARENT_LABEL.into());
    let missing = list_observed_capture_parents(&report, NOT_RETURNED_PARENT_LABEL.into()).unwrap();
    assert_eq!(missing.options.len(), 2);
    assert_eq!(
        missing.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned(NOT_RETURNED_PARENT_LABEL.into())
    );
    assert_eq!(
        missing.options[1].parent,
        PartyLedgerMasterFieldObservation::NotObserved
    );
    let empty = list_observed_capture_parents(&report, RETURNED_EMPTY_PARENT_LABEL.into()).unwrap();
    assert_eq!(empty.options.len(), 2);
    assert_eq!(
        empty.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned(RETURNED_EMPTY_PARENT_LABEL.into())
    );
    assert_eq!(
        empty.options[1].parent,
        PartyLedgerMasterFieldObservation::Returned(String::new())
    );
    let literal_group = list_observed_capture_parents(
        &report,
        format!("{GROUP_LABEL_PREFIX}{NOT_RETURNED_PARENT_LABEL}"),
    )
    .unwrap();
    assert_eq!(literal_group.options.len(), 1);
    assert_eq!(
        literal_group.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned(NOT_RETURNED_PARENT_LABEL.into())
    );

    report.rows[4].parent = PartyLedgerMasterFieldObservation::Returned("\u{fffd}Primary".into());
    let marker = list_observed_capture_parents(&report, "\u{fffd}Primary".into()).unwrap();
    assert_eq!(
        marker.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("\u{fffd}Primary".into())
    );
}

#[test]
fn blank_parent_search_keeps_interleaved_field_states_in_source_order() {
    let report = NativeTrialBalance {
        rows: vec![
            empty_parent_row(PartyLedgerMasterFieldObservation::NotObserved),
            empty_parent_row(PartyLedgerMasterFieldObservation::Returned("first".into())),
            empty_parent_row(PartyLedgerMasterFieldObservation::Returned(String::new())),
            empty_parent_row(PartyLedgerMasterFieldObservation::Returned("second".into())),
        ],
    };

    let listed = list_observed_capture_parents(&report, String::new()).unwrap();
    assert_eq!(
        listed
            .options
            .into_iter()
            .map(|option| option.parent)
            .collect::<Vec<_>>(),
        vec![
            PartyLedgerMasterFieldObservation::NotObserved,
            PartyLedgerMasterFieldObservation::Returned("first".into()),
            PartyLedgerMasterFieldObservation::Returned(String::new()),
            PartyLedgerMasterFieldObservation::Returned("second".into()),
        ]
    );
}

#[test]
fn parent_option_scan_keeps_only_101_borrowed_candidates_for_200k_rows() {
    const SYNTHETIC_ROWS: usize = 200_000;
    let report = NativeTrialBalance {
        rows: (0..SYNTHETIC_ROWS)
            .map(|index| NativeTrialBalanceRow {
                name: String::new(),
                guid: String::new(),
                parent: PartyLedgerMasterFieldObservation::Returned(format!(
                    "Synthetic parent {index:06}"
                )),
                opening: NativeTrialBalanceAmount::PresentEmpty,
                debit: NativeTrialBalanceAmount::PresentEmpty,
                credit: NativeTrialBalanceAmount::PresentEmpty,
                closing: NativeTrialBalanceAmount::PresentEmpty,
                currency_name: None,
            })
            .collect(),
    };
    let search = ParentSearch::new(String::new()).unwrap();
    let scan = scan_observed_capture_parents(&report, &search);
    assert_eq!(scan.max_retained_candidates, MAX_RETAINED_PARENT_CANDIDATES);
    assert!(
        scan.max_retained_parent_text_bytes
            <= MAX_RETAINED_PARENT_CANDIDATES * "Synthetic parent 000000".len()
    );
    let display_len = GROUP_LABEL_PREFIX.len() + "Synthetic parent 000000".len();
    assert!(scan.max_temporary_text_bytes <= "Synthetic parent 000000".len() + (2 * display_len));

    let options = scan.finish(report.rows.len());
    assert_eq!(options.source_row_count, SYNTHETIC_ROWS);
    assert_eq!(options.options.len(), MAX_PARENT_OPTIONS);
    assert!(options.has_more);
    assert_eq!(options.options[0].row_count, 1);
    assert_eq!(
        options.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("Synthetic parent 000000".into())
    );
}

#[test]
fn late_exact_parent_replaces_a_bounded_substring_candidate_and_counts_duplicates() {
    let mut rows = (0..MAX_RETAINED_PARENT_CANDIDATES)
        .map(|index| NativeTrialBalanceRow {
            name: String::new(),
            guid: String::new(),
            parent: PartyLedgerMasterFieldObservation::Returned(format!(
                "candidate needle {index:03}"
            )),
            opening: NativeTrialBalanceAmount::PresentEmpty,
            debit: NativeTrialBalanceAmount::PresentEmpty,
            credit: NativeTrialBalanceAmount::PresentEmpty,
            closing: NativeTrialBalanceAmount::PresentEmpty,
            currency_name: None,
        })
        .collect::<Vec<_>>();
    rows.extend((0..2).map(|_| NativeTrialBalanceRow {
        name: String::new(),
        guid: String::new(),
        parent: PartyLedgerMasterFieldObservation::Returned("needle".into()),
        opening: NativeTrialBalanceAmount::PresentEmpty,
        debit: NativeTrialBalanceAmount::PresentEmpty,
        credit: NativeTrialBalanceAmount::PresentEmpty,
        closing: NativeTrialBalanceAmount::PresentEmpty,
        currency_name: None,
    }));
    let report = NativeTrialBalance { rows };

    let listed = list_observed_capture_parents(&report, "needle".into()).unwrap();
    assert_eq!(listed.options.len(), MAX_PARENT_OPTIONS);
    assert!(listed.has_more);
    assert_eq!(
        listed.options[0].parent,
        PartyLedgerMasterFieldObservation::Returned("needle".into())
    );
    assert_eq!(listed.options[0].row_count, 2);
}

#[test]
fn parent_option_search_accepts_supported_display_and_control_text_but_refuses_oversize() {
    let unicode_parent = "\u{0130}".repeat(2_048);
    let report = NativeTrialBalance {
        rows: vec![
            empty_parent_row(PartyLedgerMasterFieldObservation::Returned(
                unicode_parent.clone(),
            )),
            empty_parent_row(PartyLedgerMasterFieldObservation::Returned(
                "parent\0".into(),
            )),
        ],
    };
    let supported_display = format!("{GROUP_LABEL_PREFIX}{unicode_parent}");
    assert_eq!(supported_display.len(), MAX_PARENT_DISPLAY_BYTES);
    assert_eq!(
        list_observed_capture_parents(&report, supported_display)
            .unwrap()
            .options[0]
            .parent,
        PartyLedgerMasterFieldObservation::Returned(unicode_parent)
    );
    assert_eq!(
        list_observed_capture_parents(&report, "parent\0".into())
            .unwrap()
            .options[0]
            .parent,
        PartyLedgerMasterFieldObservation::Returned("parent\0".into())
    );
    assert_eq!(
        list_observed_capture_parents(&report, "x".repeat(MAX_PARENT_DISPLAY_BYTES + 1))
            .unwrap_err(),
        TrialBalanceCaptureParentOptionsError::SearchInvalid
    );
}
