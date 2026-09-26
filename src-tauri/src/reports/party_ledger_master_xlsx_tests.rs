use std::io::Cursor;

use bridge_tally_core::{ExactDecimal, TallyDate};
use zip::ZipArchive;

use super::*;
use crate::reports::party_ledger_master::{
    build_party_ledger_master_workbook, PartyLedgerMasterRow, PartyLedgerMasterSource,
};
use bridge_tally_protocol::{PartyLedgerMasterFieldObservation, PartyLedgerMasterFields};

fn render_with(
    workbook: &crate::reports::party_ledger_master::PartyLedgerMasterWorkbook,
    decisions: &[crate::reports::schedule_iii::Decision],
) -> Result<Vec<u8>, PartyLedgerMasterXlsxError> {
    let set = crate::reports::schedule_iii::DecisionSet::for_tests(workbook, decisions.to_vec())
        .expect("a valid synthetic decision set");
    render_party_ledger_master_xlsx(workbook, DecisionInput::Read(&set))
}

fn source_with_precision(decimal_places: u8) -> PartyLedgerMasterSource {
    PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: decimal_places,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![PartyLedgerMasterRow {
            name: "Three decimal customer".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
            fields: PartyLedgerMasterFields::default(),
            guid: "ledger-guid".to_string(),
            master_id: "7".to_string(),
            alter_id: "9".to_string(),
            opening_balance: ExactDecimal::parse("-1.234".to_string()).unwrap(),
            closing_balance: Some(ExactDecimal::parse("1.234".to_string()).unwrap()),
        }],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 100,
        balance_response_bytes: 200,
        group_response_bytes: 300,
        groups: vec![],
    }
}

#[test]
fn three_decimal_currency_renders_1234_without_a_two_decimal_format() {
    let workbook = build_party_ledger_master_workbook(source_with_precision(3)).unwrap();
    let bytes = render_with(&workbook, &[]).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    let mut styles = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("xl/styles.xml").unwrap(), &mut styles)
        .unwrap();

    assert!(sheet.contains(">1.234</v>"));
    assert!(!sheet.contains(">1.23</v>"));
    assert!(styles.contains("formatCode=\"##,##,##0.000\""));
    assert!(!styles.contains("formatCode=\"##,##,##0.00\""));
}

#[test]
fn unrenderable_currency_precision_withholds_the_workbook() {
    assert!(matches!(
        build_party_ledger_master_workbook(source_with_precision(16)),
        Err(super::super::party_ledger_master::PartyLedgerMasterError::UnrenderableCurrencyPrecision(16))
    ));
}

#[test]
fn renders_evidence_currency_and_returned_fields_in_the_workbook() {
    let workbook = build_party_ledger_master_workbook(PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![PartyLedgerMasterRow {
            name: "Customer".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::Returned("29ABCDE1234F1Z5".to_string()),
            fields: PartyLedgerMasterFields {
                email: PartyLedgerMasterFieldObservation::Returned(
                    "synthetic@example.invalid".to_string(),
                ),
                ..PartyLedgerMasterFields::default()
            },
            guid: "ledger-guid".to_string(),
            master_id: "7".to_string(),
            alter_id: "9".to_string(),
            opening_balance: ExactDecimal::parse("-100.00".to_string()).unwrap(),
            closing_balance: Some(ExactDecimal::parse("125.00".to_string()).unwrap()),
        }],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 100,
        balance_response_bytes: 200,
        group_response_bytes: 300,
        groups: vec![],
    })
    .unwrap();
    let bytes = render_with(&workbook, &[]).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in [
        "xl/worksheets/sheet1.xml",
        "xl/worksheets/sheet2.xml",
        "xl/sharedStrings.xml",
    ] {
        std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut text).unwrap();
    }
    assert!(text.contains("Master response SHA-256"));
    assert!(text.contains("Not observed fields"));
    assert!(!text.contains("Unavailable fields"));
    assert!(text.contains(
        "“Not observed” means this Tally response did not return the requested field; it does not establish whether that field is unset in this book or unavailable in this Tally build. Bridge never manufactures master data."
    ));
    assert!(text.contains("Not observed"));
    assert!(!text.contains("was unset in this book"));
    assert!(text.contains("Income Tax number (as returned)"));
    assert!(text.contains("synthetic@example.invalid"));
    assert!(text.contains("Currency"));
    assert!(text.contains("INR"));
    assert!(text.contains(
        "Read period: 20260401 to 20260731. No prior-year values were requested or inferred."
    ));
    assert!(!text.contains("One year read"));
    assert!(text.contains("EXCLUSION LIST (loud)"));
}

#[test]
fn normally_signed_sundry_debtor_renders_as_a_group_subtotal_not_trade_receivables() {
    let workbook = build_party_ledger_master_workbook(PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![PartyLedgerMasterRow {
            name: "Customer balance".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
            fields: PartyLedgerMasterFields::default(),
            guid: "ledger-guid".to_string(),
            master_id: "7".to_string(),
            alter_id: "9".to_string(),
            opening_balance: ExactDecimal::parse("-300.00".to_string()).unwrap(),
            closing_balance: Some(ExactDecimal::parse("-300.00".to_string()).unwrap()),
        }],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 100,
        balance_response_bytes: 200,
        group_response_bytes: 300,
        groups: vec![bridge_tally_protocol::TallyNamedMaster {
            name: "Sundry Debtors".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Primary".to_string()),
            reserved_name: Some("Sundry Debtors".to_string()),
        }],
    })
    .unwrap();

    let bytes = render_with(&workbook, &[]).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in [
        "xl/worksheets/sheet1.xml",
        "xl/worksheets/sheet2.xml",
        "xl/sharedStrings.xml",
    ] {
        std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut text).unwrap();
    }

    assert!(text.contains("Sundry Debtors group subtotal"));
    assert!(text.contains("-300.00"));
    assert!(!text.contains("Trade receivables"));
}

#[test]
fn gstin_not_observed_is_labeled_while_an_explicit_empty_gstin_is_not() {
    let rows = vec![
        PartyLedgerMasterRow {
            name: "GSTIN not observed".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
            fields: PartyLedgerMasterFields::default(),
            guid: "ledger-guid-1".to_string(),
            master_id: "7".to_string(),
            alter_id: "9".to_string(),
            opening_balance: ExactDecimal::parse("-100.00".to_string()).unwrap(),
            closing_balance: Some(ExactDecimal::parse("125.00".to_string()).unwrap()),
        },
        PartyLedgerMasterRow {
            name: "GSTIN returned empty".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::Returned(String::new()),
            fields: PartyLedgerMasterFields::default(),
            guid: "ledger-guid-2".to_string(),
            master_id: "8".to_string(),
            alter_id: "10".to_string(),
            opening_balance: ExactDecimal::parse("-200.00".to_string()).unwrap(),
            closing_balance: Some(ExactDecimal::parse("250.00".to_string()).unwrap()),
        },
    ];
    let workbook = build_party_ledger_master_workbook(PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows,
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 100,
        balance_response_bytes: 200,
        group_response_bytes: 300,
        groups: vec![],
    })
    .unwrap();
    let bytes = render_with(&workbook, &[]).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    let mut shared_strings = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
        &mut shared_strings,
    )
    .unwrap();
    let not_observed_index = shared_strings
        .split("<si>")
        .skip(1)
        .position(|entry| entry.contains("<t>Not observed</t>"))
        .expect("workbook contains the disclosure label");
    let not_observed_cell = format!(r#"r="C15" t="s"><v>{not_observed_index}</v>"#);
    let explicitly_empty_cell = format!(r#"r="C16" t="s"><v>{not_observed_index}</v>"#);

    assert!(
        sheet.contains(&not_observed_cell),
        "an omitted GSTIN must render the observation label"
    );
    assert!(
        !sheet.contains(&explicitly_empty_cell),
        "an explicitly returned empty GSTIN must not be mislabeled as not observed"
    );
}

fn worksheet_with_parent(parent: PartyLedgerMasterFieldObservation) -> (String, usize) {
    let workbook = build_party_ledger_master_workbook(PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![PartyLedgerMasterRow {
            name: "Parent observation".to_string(),
            parent,
            party_gstin: PartyLedgerMasterFieldObservation::Returned(String::new()),
            fields: PartyLedgerMasterFields::default(),
            guid: "ledger-guid-parent".to_string(),
            master_id: "11".to_string(),
            alter_id: "12".to_string(),
            opening_balance: ExactDecimal::parse("-100.00".to_string()).unwrap(),
            closing_balance: Some(ExactDecimal::parse("125.00".to_string()).unwrap()),
        }],
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 100,
        balance_response_bytes: 200,
        group_response_bytes: 300,
        groups: vec![],
    })
    .unwrap();
    let bytes = render_with(&workbook, &[]).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut sheet = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
        &mut sheet,
    )
    .unwrap();
    let mut shared_strings = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
        &mut shared_strings,
    )
    .unwrap();
    let not_observed_index = shared_strings
        .split("<si>")
        .skip(1)
        .position(|entry| entry.contains("<t>Not observed</t>"))
        .expect("workbook contains the disclosure label");
    (sheet, not_observed_index)
}

#[test]
fn parent_not_observed_is_labeled_in_the_group_cell() {
    let (sheet, not_observed_index) =
        worksheet_with_parent(PartyLedgerMasterFieldObservation::NotObserved);
    let group_cell = format!(r#"r="B15" t="s"><v>{not_observed_index}</v>"#);

    assert!(
        sheet.contains(&group_cell),
        "an omitted parent must render Not observed instead of a blank Group cell"
    );
}

#[test]
fn explicitly_empty_parent_renders_an_empty_group_cell() {
    let (sheet, not_observed_index) =
        worksheet_with_parent(PartyLedgerMasterFieldObservation::Returned(String::new()));
    let group_cell = format!(r#"r="B15" t="s"><v>{not_observed_index}</v>"#);

    assert!(
        !sheet.contains(&group_cell),
        "an explicitly empty parent must remain empty rather than being labeled Not observed"
    );
    let cell_has_value = sheet
        .find(r#"<c r="B15""#)
        .and_then(|start| {
            sheet[start..]
                .find("</c>")
                .map(|end| &sheet[start..start + end])
        })
        .is_some_and(|cell| cell.contains("<v>"));
    assert!(
        !cell_has_value,
        "an explicitly empty parent must not write a Group-cell value"
    );
}

#[test]
fn decided_heads_their_basis_and_a_stale_decision_reach_the_group_subtotal_sheet() {
    use crate::reports::schedule_iii::{
        Decision, DecisionId, Derivation, DerivedOutcome, FinancialYear, GroupSubtotalKind,
        LedgerGuid, ScheduleIIIHead,
    };
    let ledger = |name: &str, guid: &str, balance: &str| PartyLedgerMasterRow {
        name: name.to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
        fields: PartyLedgerMasterFields::default(),
        guid: guid.to_string(),
        master_id: guid.to_string(),
        alter_id: "9".to_string(),
        opening_balance: ExactDecimal::parse(balance.to_string()).unwrap(),
        closing_balance: Some(ExactDecimal::parse(balance.to_string()).unwrap()),
    };
    let mut source = source_with_precision(2);
    source.rows = vec![
        ledger("Customer balance", "customer-guid", "-300.00"),
        ledger("Moved party", "moved-guid", "-50.00"),
    ];
    source.groups = vec![bridge_tally_protocol::TallyNamedMaster {
        name: "Sundry Debtors".to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned("Primary".to_string()),
        reserved_name: Some("Sundry Debtors".to_string()),
    }];
    let workbook = build_party_ledger_master_workbook(source).unwrap();
    let decision = |id: u64, guid: &str, name: &str, made_against: GroupSubtotalKind| Decision {
        id: DecisionId(id),
        ledger: LedgerGuid::new(guid).unwrap(),
        ledger_name_when_made: name.to_string(),
        made_against: Derivation {
            outcome: DerivedOutcome::GroupSubtotal(made_against),
            ancestry: vec!["Sundry Debtors".to_string()],
        },
        head: ScheduleIIIHead::TradeReceivables,
        year: FinancialYear::beginning_in(2026),
    };
    let decisions = [
        decision(
            1,
            "customer-guid",
            "Customer balance",
            GroupSubtotalKind::SundryDebtors,
        ),
        decision(
            2,
            "moved-guid",
            "Moved party",
            GroupSubtotalKind::SundryCreditors,
        ),
    ];

    let bytes = render_with(&workbook, &decisions).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    for name in ["xl/worksheets/sheet2.xml", "xl/sharedStrings.xml"] {
        std::io::Read::read_to_string(&mut archive.by_name(name).unwrap(), &mut text).unwrap();
    }

    // Exact cells: only a decided line or trace row reads exactly "CA grouping
    // decision", and only a group-subtotal one "Tally group evidence".
    assert!(text.contains("<t>Trade receivables</t>"));
    assert!(text.contains("<t>CA grouping decision</t>"));
    assert!(text.contains("<t>Tally group evidence</t>"));
    assert!(text.contains("CA GROUPING DECISIONS"));
    assert!(text.contains(
        "1 applied; 1 not applied and need attention; 0 for another financial year. NOT FINAL"
    ));
    assert!(text.contains("group changed after the decision. Now: Sundry Debtors group subtotal"));
}

#[test]
fn a_decision_whose_ledger_moved_between_subgroups_reports_the_change_on_the_row_and_in_the_list() {
    use crate::reports::schedule_iii::{
        Decision, DecisionId, Derivation, DerivedOutcome, FinancialYear, GroupSubtotalKind,
        LedgerGuid, ScheduleIIIHead,
    };
    let group =
        |name: &str, parent: &str, reserved: &str| bridge_tally_protocol::TallyNamedMaster {
            name: name.to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned(parent.to_string()),
            reserved_name: Some(reserved.to_string()),
        };
    let mut source = source_with_precision(2);
    source.rows[0].name = "Advance".to_string();
    source.rows[0].parent = PartyLedgerMasterFieldObservation::Returned("Customers".to_string());
    source.rows[0].closing_balance = Some(ExactDecimal::parse("-40.00".to_string()).unwrap());
    source.groups = vec![
        group("Sundry Debtors", "Primary", "Sundry Debtors"),
        group("Staff advances", "Sundry Debtors", ""),
        group("Customers", "Sundry Debtors", ""),
    ];
    let workbook = build_party_ledger_master_workbook(source).unwrap();
    let decisions = [Decision {
        id: DecisionId(1),
        ledger: LedgerGuid::new("ledger-guid").unwrap(),
        ledger_name_when_made: "Advance".to_string(),
        made_against: Derivation {
            outcome: DerivedOutcome::GroupSubtotal(GroupSubtotalKind::SundryDebtors),
            ancestry: vec!["Staff advances".to_string(), "Sundry Debtors".to_string()],
        },
        head: ScheduleIIIHead::ShortTermLoansAndAdvances,
        year: FinancialYear::beginning_in(2026),
    }];

    let bytes = render_with(&workbook, &decisions).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut strings = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
        &mut strings,
    )
    .unwrap();

    let drift = "Ancestry changed since the decision: Sundry Debtors";
    assert_eq!(
        strings.matches(drift).count(),
        2,
        "on the row and in the list"
    );
    assert!(strings.contains("Staff advances → Sundry Debtors"));
    assert!(strings.contains("CA grouping decision. Ancestry changed since the decision"));
    assert!(strings.contains("Applied. Ancestry changed since the decision"));
    assert!(strings.contains("Nothing needs attention."));
}

#[test]
fn decisions_that_could_not_be_read_are_never_reported_as_absent() {
    use crate::reports::schedule_iii::DecisionsUnavailable;
    let workbook = build_party_ledger_master_workbook(source_with_precision(2)).unwrap();
    for (why, text) in [
        (
            DecisionsUnavailable::StoreUnavailable,
            "Could not be read: the encrypted store could not be opened or read. NOT FINAL",
        ),
        (
            DecisionsUnavailable::Unreadable,
            "Could not be read: the stored decisions could not be read by this version of ComplyEaze Bridge. NOT FINAL",
        ),
    ] {
        let bytes =
            render_party_ledger_master_xlsx(&workbook, DecisionInput::Unavailable(why)).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut strings = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/sharedStrings.xml").unwrap(),
            &mut strings,
        )
        .unwrap();
        assert!(strings.contains(text), "{why:?}");
        assert!(!strings.contains("None were applied"));
    }
}
