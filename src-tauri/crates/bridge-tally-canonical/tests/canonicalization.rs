use bridge_tally_canonical::{build_core_window, validate_selected_voucher_window};
use bridge_tally_core::{
    CanonicalPackWindow, CanonicalText, CapabilityPackId, CompanyRef, LedgerEntryPolarity,
    ObservedSourceIdentities, PackBatch, PackSchemaVersion, ReadWindow, RequestContext,
    SourceIdentity, SourceIdentityKind, TallyError,
};
use bridge_tally_protocol::{
    parse_group_source_records_with_evidence, parse_ledger_source_records_with_evidence,
    parse_voucher_source_records_with_evidence, parse_voucher_type_source_records_with_evidence,
    ParsedExport, ParsedSourceRecord, TallyLedger, TallyNamedMaster, TallyVoucher,
    BRIDGE_GROUP_EXPORT_SCHEMA, BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_VOUCHER_EXPORT_SCHEMA,
    BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA,
};

fn context() -> RequestContext {
    RequestContext {
        run_id: "synthetic-run".to_string(),
        company: CompanyRef {
            identity: SourceIdentity {
                bridge_source_lineage: "synthetic-lineage".to_string(),
                company_guid: "synthetic-company-guid".to_string(),
                observed_fingerprint: "synthetic-observation".to_string(),
            },
            display_name: "BRIDGE SYNTHETIC BOOK".to_string(),
        },
        pack: CapabilityPackId::CoreAccounting,
        schema_version: PackSchemaVersion { major: 1, minor: 0 },
        window: ReadWindow {
            from_yyyymmdd: "20260701".to_string(),
            to_yyyymmdd: "20260731".to_string(),
        },
        query_profile: CanonicalText::parse("core_accounting_v1").unwrap(),
        filters_sha256: CanonicalText::parse("0".repeat(64)).unwrap(),
    }
}

fn groups() -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
    parse_group_source_records_with_evidence(&format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="GROUP" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="1"/><GROUP NAME="Assets" GUID="group-guid" MASTERID="1" ALTERID="5"><PARENT>Primary</PARENT></GROUP></BODY></ENVELOPE>"#,
        BRIDGE_GROUP_EXPORT_SCHEMA
    ))
    .unwrap()
}

fn ledgers_and_vouchers(
    cash_name: &str,
    entry_ledger_name: &str,
) -> (
    ParsedExport<ParsedSourceRecord<TallyLedger>>,
    ParsedExport<ParsedSourceRecord<TallyVoucher>>,
) {
    let ledgers = parse_ledger_source_records_with_evidence(&format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="2"/><LEDGER NAME="{}" GUID="ledger-cash" REMOTEID="cash-remote" MASTERID="2" ALTERID="6"><PARENT>Assets</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER><LEDGER NAME="Sales" GUID="ledger-sales" MASTERID="3" ALTERID="7"><PARENT>Assets</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></BODY></ENVELOPE>"#,
        BRIDGE_LEDGER_EXPORT_SCHEMA, cash_name
    ))
    .unwrap();
    let vouchers = parse_voucher_source_records_with_evidence(&format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHER" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="1"/><VOUCHER GUID="voucher-guid" REMOTEID="voucher-remote" MASTERID="9" ALTERID="10"><DATE>20260714</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><VOUCHERNUMBER>SYN-1</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><LEDGERENTRYCOUNT>2</LEDGERENTRYCOUNT><LEDGERENTRIES><LEDGERENTRY><ENTRYINDEX>1</ENTRYINDEX><LEDGERNAME>{}</LEDGERNAME><AMOUNT>-100.00</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE></LEDGERENTRY><LEDGERENTRY><ENTRYINDEX>2</ENTRYINDEX><LEDGERNAME>Sales</LEDGERNAME><AMOUNT>100.00</AMOUNT><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE></LEDGERENTRY></LEDGERENTRIES></VOUCHER></BODY></ENVELOPE>"#,
        BRIDGE_VOUCHER_EXPORT_SCHEMA, entry_ledger_name
    ))
    .unwrap();
    (ledgers, vouchers)
}

fn voucher_types() -> ParsedExport<ParsedSourceRecord<TallyNamedMaster>> {
    parse_voucher_type_source_records_with_evidence(&format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHERTYPE" NAME="BRIDGE SYNTHETIC BOOK" GUID="synthetic-company-guid" RECORDCOUNT="1"/><VOUCHERTYPE NAME="Receipt" GUID="voucher-type-guid" MASTERID="8" ALTERID="9"><PARENT>Receipt</PARENT></VOUCHERTYPE></BODY></ENVELOPE>"#,
        BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA
    ))
    .unwrap()
}

fn valid_window() -> CanonicalPackWindow {
    let (ledgers, vouchers) = ledgers_and_vouchers("Cash", "Cash");
    build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers).unwrap()
}

#[test]
fn canonicalizes_all_core_records_with_exact_reference_and_provenance_binding() {
    let window = valid_window();
    window.validate_record_evidence_binding().unwrap();
    let PackBatch::CoreAccounting(batch) = &window.batch else {
        panic!("wrong pack")
    };
    assert_eq!(
        (
            batch.groups.len(),
            batch.ledgers.len(),
            batch.voucher_types.len()
        ),
        (1, 2, 1)
    );
    assert_eq!((batch.vouchers.len(), batch.ledger_entries.len()), (1, 2));
    assert_eq!(
        batch.ledgers[0].parent_source_id.as_deref(),
        Some("group-guid")
    );
    assert_eq!(batch.groups[0].parent_source_id, None);
    assert_eq!(
        batch.vouchers[0].voucher_type_source_id,
        "voucher-type-guid"
    );
    assert_eq!(batch.ledger_entries[0].ledger_source_id, "ledger-cash");
    assert_eq!(batch.ledger_entries[0].voucher_source_id, "voucher-guid");
    assert_eq!(batch.ledger_entries[0].polarity, LedgerEntryPolarity::Debit);
    assert_eq!(
        batch.ledger_entries[1].polarity,
        LedgerEntryPolarity::Credit
    );
    assert!(batch.ledger_entries[0]
        .source_id
        .starts_with("bridge-derived:ledger-entry:v1:"));
    assert_eq!(window.source_counts.as_ref().unwrap().len(), 5);
    assert_eq!(window.record_evidence.as_ref().unwrap().len(), 7);

    let voucher_evidence = window
        .record_evidence
        .as_ref()
        .unwrap()
        .iter()
        .find(|evidence| evidence.object_type.as_str() == "voucher")
        .unwrap();
    assert_eq!(voucher_evidence.identity_kind, SourceIdentityKind::Guid);
    assert_eq!(
        voucher_evidence
            .observed_identities
            .remote_id
            .as_ref()
            .unwrap()
            .as_str(),
        "voucher-remote"
    );
    assert_eq!(
        voucher_evidence
            .observed_identities
            .master_id
            .as_ref()
            .unwrap()
            .as_str(),
        "9"
    );
}

#[test]
fn derived_entry_ids_are_deterministic_but_never_claim_native_identity() {
    fn entry_ids(window: &CanonicalPackWindow) -> Vec<String> {
        let PackBatch::CoreAccounting(batch) = &window.batch else {
            panic!("wrong pack")
        };
        batch
            .ledger_entries
            .iter()
            .map(|entry| entry.source_id.clone())
            .collect()
    }
    let first = valid_window();
    let second = valid_window();
    assert_eq!(entry_ids(&first), entry_ids(&second));
    let entry_evidence = first
        .record_evidence
        .as_ref()
        .unwrap()
        .iter()
        .filter(|evidence| evidence.object_type.as_str() == "ledger_entry")
        .collect::<Vec<_>>();
    assert_eq!(entry_evidence.len(), 2);
    assert!(entry_evidence.iter().all(|evidence| {
        evidence.identity_kind == SourceIdentityKind::Fallback
            && evidence.observed_identities == ObservedSourceIdentities::default()
    }));
}

#[test]
fn unresolved_mutable_name_reference_fails_closed() {
    let (ledgers, vouchers) = ledgers_and_vouchers("Cash", "Missing Ledger");
    let error =
        build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers).unwrap_err();
    assert!(matches!(
        error,
        TallyError::InvalidData { code }
            if code == "voucher_ledger_reference_missing"
    ));
}

#[test]
fn duplicate_mutable_names_fail_closed_even_when_native_ids_differ() {
    let (ledgers, vouchers) = ledgers_and_vouchers("Sales", "Sales");
    let error =
        build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers).unwrap_err();
    assert!(matches!(
        error,
        TallyError::InvalidData { code } if code == "ledger_name_duplicate"
    ));
}

#[test]
fn invalid_or_out_of_window_voucher_dates_fail_before_canonical_state_exists() {
    for (date, expected_code) in [
        ("20260230", "voucher_date_invalid"),
        ("20260630", "voucher_date_outside_requested_window"),
        ("20260801", "voucher_date_outside_requested_window"),
    ] {
        let (ledgers, mut vouchers) = ledgers_and_vouchers("Cash", "Cash");
        vouchers.records[0].record.date = Some(date.to_string());
        let error = build_core_window(&context(), groups(), ledgers, voucher_types(), vouchers)
            .unwrap_err();
        assert!(matches!(
            error,
            TallyError::InvalidData { code } if code == expected_code
        ));
    }
}

#[test]
fn invalid_requested_window_fails_before_source_rows_are_canonicalized() {
    for (from, to) in [("20260230", "20260731"), ("20260801", "20260731")] {
        let mut request = context();
        request.window.from_yyyymmdd = from.to_string();
        request.window.to_yyyymmdd = to.to_string();
        let (ledgers, vouchers) = ledgers_and_vouchers("Cash", "Cash");
        let error =
            build_core_window(&request, groups(), ledgers, voucher_types(), vouchers).unwrap_err();
        assert!(matches!(
            error,
            TallyError::InvalidData { code } if code == "requested_window_invalid"
        ));
    }
}

#[test]
fn selected_voucher_qualification_rejects_noncanonical_records_and_entries() {
    let (_, vouchers) = ledgers_and_vouchers("Cash", "Cash");
    validate_selected_voucher_window("20260701", "20260731", &vouchers).unwrap();

    let mut invalid_amount = vouchers.clone();
    invalid_amount.records[0].record.ledger_entries[0].amount = "not-an-amount".to_string();
    assert!(validate_selected_voucher_window("20260701", "20260731", &invalid_amount).is_err());

    let mut invalid_name = vouchers.clone();
    invalid_name.records[0].record.ledger_entries[0].ledger_name = " x ".to_string();
    assert!(validate_selected_voucher_window("20260701", "20260731", &invalid_name).is_err());

    let mut invalid_alter_id = vouchers;
    invalid_alter_id.records[0].alter_id = Some("contains whitespace".to_string());
    assert!(validate_selected_voucher_window("20260701", "20260731", &invalid_alter_id).is_err());
}
