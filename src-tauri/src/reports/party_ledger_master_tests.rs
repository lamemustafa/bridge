use std::collections::BTreeMap;
use std::io::Cursor;

use bridge_tally_protocol::{
    native_outstandings::{
        parse_native_group_snapshot_with_evidence, parse_native_ledger_snapshot,
    },
    parse_native_party_ledger_master_records_with_evidence, PartyLedgerMasterFieldObservation,
    PartyLedgerMasterFields,
};

use super::*;
use crate::tally::OutstandingsCurrencyAssertion;
use zip::ZipArchive;

const MASTER_FIELDS_LAB_COMPANY_GUID: &str = "56359347-3976-4d01-b44e-56fa0f6a422c";
const MASTER_FIELDS_LAB_LEDGERS: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/ledgers_native_master_fields_lab.utf8.xml"
);
const MASTER_FIELDS_LAB_BALANCES: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/ledger_snapshot_master_fields_lab.utf8.xml"
);
const MASTER_FIELDS_LAB_GROUPS: &str = include_str!(
    "../../crates/bridge-tally-protocol/tests/fixtures/native/group_snapshot_master_fields_lab.utf8.xml"
);

fn master_response_with_computed_company_guid() -> String {
    // This is the captured master body. It predates the response-bound
    // compute added to the request, whose exact `List of Ledgers` shape
    // is already captured by the standard identity profile. The test
    // adds only that collection-level response context; it does not
    // invent, alter, or populate any master field.
    MASTER_FIELDS_LAB_LEDGERS.replace(
        "</GUID>",
        "</GUID><BRIDGECOMPANYGUID>56359347-3976-4d01-b44e-56fa0f6a422c</BRIDGECOMPANYGUID>",
    )
}

fn group_response_with_computed_company_guid() -> String {
    // This is the captured Group body. It predates the response-bound
    // compute added to the request. The test adds only the selected
    // collection context; it does not invent or alter Group master data.
    MASTER_FIELDS_LAB_GROUPS.replace(
        "</GUID>",
        "</GUID><BRIDGECOMPANYGUID>56359347-3976-4d01-b44e-56fa0f6a422c</BRIDGECOMPANYGUID>",
    )
}

fn source() -> PartyLedgerMasterSource {
    PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20260401").unwrap(),
        to: TallyDate::parse("20260731").unwrap(),
        rows: vec![PartyLedgerMasterRow {
            name: "Customer".to_string(),
            parent: PartyLedgerMasterFieldObservation::Returned("Sundry Debtors".to_string()),
            party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
            fields: PartyLedgerMasterFields::default(),
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
    }
}

#[test]
fn rejects_duplicate_source_identities_before_rendering() {
    let mut duplicate_guid = source();
    duplicate_guid.rows.push(PartyLedgerMasterRow {
        guid: "LEDGER-GUID".to_string(),
        master_id: "8".to_string(),
        ..duplicate_guid.rows[0].clone()
    });
    assert!(matches!(
        build_party_ledger_master_workbook(duplicate_guid),
        Err(PartyLedgerMasterError::DuplicateGuid)
    ));

    let mut duplicate_master_id = source();
    duplicate_master_id.rows.push(PartyLedgerMasterRow {
        guid: "other-guid".to_string(),
        ..duplicate_master_id.rows[0].clone()
    });
    assert!(matches!(
        build_party_ledger_master_workbook(duplicate_master_id),
        Err(PartyLedgerMasterError::DuplicateMasterId)
    ));
}

#[test]
fn captured_master_fields_lab_drives_the_party_export_and_schedule_iii_view() {
    let master = parse_native_party_ledger_master_records_with_evidence(
        &master_response_with_computed_company_guid(),
        MASTER_FIELDS_LAB_COMPANY_GUID,
    )
    .expect("captured ledger-master response parses");
    let mut balances = BTreeMap::new();
    for balance in parse_native_ledger_snapshot(MASTER_FIELDS_LAB_BALANCES)
        .expect("captured balance response parses")
    {
        let key = (balance.name.clone(), balance.parent.clone());
        assert!(
            balances.insert(key, balance).is_none(),
            "captured balance response has one row per display key"
        );
    }

    let mut rows = Vec::new();
    for source in master.records {
        let key = (
            source.record.ledger.name.clone(),
            source
                .record
                .ledger
                .parent
                .nonempty_returned_text()
                .map(str::to_owned),
        );
        let balance = balances
            .remove(&key)
            .expect("every captured master has a corresponding balance");
        assert_eq!(
            source.record.ledger.opening_balance.as_deref(),
            Some(balance.opening_balance.as_str()),
            "captured master and balance openings agree"
        );
        rows.push(PartyLedgerMasterRow {
            name: source.record.ledger.name,
            parent: source.record.ledger.parent,
            party_gstin: source.record.ledger.party_gstin,
            fields: source.record.fields,
            guid: source.identities.guid.expect("captured GUID"),
            master_id: source.identities.master_id.expect("captured MASTERID"),
            alter_id: source.alter_id.expect("captured ALTERID"),
            opening_balance: balance.opening_balance,
            closing_balance: balance.closing_balance,
        });
    }
    assert!(
        balances.is_empty(),
        "the captured balance response has no unmatched ledger"
    );

    let groups = parse_native_group_snapshot_with_evidence(
        &group_response_with_computed_company_guid(),
        MASTER_FIELDS_LAB_COMPANY_GUID,
    )
    .expect("captured group response parses")
    .into_iter()
    .map(|entry| entry.record)
    .collect();
    let source = PartyLedgerMasterSource {
        company: "BRIDGE MASTER FIELDS LAB".to_string(),
        company_guid: MASTER_FIELDS_LAB_COMPANY_GUID.to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20250401").unwrap(),
        to: TallyDate::parse("20260331").unwrap(),
        rows,
        request_sha256: "0".repeat(64),
        master_response_sha256: "def766e42d0e36b4b73d7a176fa0ad08d1a7467e301000650fb5ba9a2ae06f29"
            .to_string(),
        balance_response_sha256: "7e7af264d7251713d5179c6b6614f47329d860e941587ec9a5245597bf059f77"
            .to_string(),
        group_response_sha256: "a9839b578ed776b707597e5dad93d155ab443b93c03d5ecb0b70bfd7ac207b1b"
            .to_string(),
        master_response_bytes: MASTER_FIELDS_LAB_LEDGERS.len(),
        balance_response_bytes: MASTER_FIELDS_LAB_BALANCES.len(),
        group_response_bytes: MASTER_FIELDS_LAB_GROUPS.len(),
        groups,
    };
    let workbook = build_party_ledger_master_workbook(source).expect("captured source admits");
    let xlsx = super::super::party_ledger_master_xlsx::render_party_ledger_master_xlsx(&workbook)
        .expect("captured source renders a workbook");
    assert!(
        xlsx.starts_with(b"PK"),
        "captured source rendered an XLSX archive"
    );
    let mut archive = ZipArchive::new(Cursor::new(xlsx)).expect("valid XLSX archive");
    let mut xlsx_text = String::new();
    for name in [
        "xl/worksheets/sheet1.xml",
        "xl/worksheets/sheet2.xml",
        "xl/sharedStrings.xml",
    ] {
        std::io::Read::read_to_string(
            &mut archive.by_name(name).expect("XLSX part"),
            &mut xlsx_text,
        )
        .expect("read XLSX part");
    }
    assert!(xlsx_text.contains("ZZZZZ0002Z"));
    assert!(xlsx_text.contains("INR"));
    let source = workbook.source();
    assert_eq!(source.rows.len(), 17);
    assert!(source.rows.iter().any(|row| {
        row.name == "BRIDGE MFLAB DEBTOR CREDIT BALANCE"
            && row.opening_balance.as_str() == "1250.00"
            && row
                .closing_balance
                .as_ref()
                .is_some_and(|balance| balance.as_str() == "1250.00")
    }));
    assert!(source.rows.iter().any(|row| {
        row.name == "BRIDGE MFLAB CREDITOR DEBIT BALANCE"
            && row.opening_balance.as_str() == "-1250.00"
            && row
                .closing_balance
                .as_ref()
                .is_some_and(|balance| balance.as_str() == "-1250.00")
    }));
    assert!(source.rows.iter().any(|row| {
        row.name == "BRIDGE MFLAB DEBTOR BETA"
            && row.fields.income_tax_number
                == PartyLedgerMasterFieldObservation::Returned("ZZZZZ0002Z".to_string())
            && row.fields.name_on_pan
                == PartyLedgerMasterFieldObservation::Returned(
                    "BRIDGE MFLAB DEBTOR BETA".to_string(),
                )
            && row.fields.pin_code == PartyLedgerMasterFieldObservation::Returned(String::new())
            && row.fields.state == PartyLedgerMasterFieldObservation::NotObserved
    }));

    let schedule = super::super::schedule_iii::build_schedule_iii_view(source)
        .expect("captured Schedule III derivation succeeds");
    assert!(schedule.difference.is_zero());
    assert!(schedule
        .lines
        .iter()
        .all(|line| line.label.ends_with("group subtotal")));
    assert!(schedule.exclusions.iter().any(|entry| {
        source.rows[entry.row_index].name == "BRIDGE MFLAB DEBTOR CREDIT BALANCE"
            && entry.reason.contains("opposite polarity")
    }));
    assert!(schedule.exclusions.iter().any(|entry| {
        source.rows[entry.row_index].name == "BRIDGE MFLAB CREDITOR DEBIT BALANCE"
            && entry.reason.contains("opposite polarity")
    }));
}
