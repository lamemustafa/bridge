use bridge_tally_core::structured_import::{
    plan_payment_receipt_json, CompanyLedgerCatalog, DispatchAuthority, DryRunState,
    ImportLedgerMappingInput, ImportLedgerMappings, PostingSide, SettlementLedgerRole,
    StructuredImportError, VoucherKind,
};
use bridge_tally_core::{
    source_count_scope_fingerprint, CanonicalPackWindow, CanonicalText, CapabilityPackId,
    CompanyRef, CoreAccountingBatch, GroupRecord, LedgerRecord, ObservedSourceIdentities,
    PackBatch, ReadWindow, RequestContext, SourceCountScope, SourceCountScopeDescriptor,
    SourceIdentity, SourceIdentityKind, SourceRecordEvidence, SourceRecordId,
    SourceReportedCountEvidence, CORE_ACCOUNTING_SCHEMA_VERSION,
};

const PARTY_GUID: &str = "synthetic-company-guid-party";
const BANK_GUID: &str = "synthetic-company-guid-bank";
const OTHER_BANK_GUID: &str = "synthetic-company-guid-bank-two";

fn request_context() -> RequestContext {
    RequestContext {
        run_id: "synthetic-run-1".to_string(),
        company: CompanyRef {
            identity: SourceIdentity {
                bridge_source_lineage: "synthetic-lineage".to_string(),
                company_guid: "synthetic-company-guid".to_string(),
                observed_fingerprint: "a".repeat(64),
            },
            display_name: "Synthetic Company".to_string(),
        },
        pack: CapabilityPackId::CoreAccounting,
        schema_version: CORE_ACCOUNTING_SCHEMA_VERSION,
        window: ReadWindow {
            from_yyyymmdd: "20260401".to_string(),
            to_yyyymmdd: "20260430".to_string(),
        },
        query_profile: CanonicalText::parse("synthetic_core_profile").unwrap(),
        filters_sha256: CanonicalText::parse("b".repeat(64)).unwrap(),
    }
}

fn evidence(object_type: &str, source_id: &str) -> SourceRecordEvidence {
    let source_id = SourceRecordId::parse(source_id).unwrap();
    SourceRecordEvidence {
        object_type: CanonicalText::parse(object_type).unwrap(),
        source_id: source_id.clone(),
        identity_kind: SourceIdentityKind::Guid,
        observed_identities: ObservedSourceIdentities {
            guid: Some(source_id),
            ..ObservedSourceIdentities::default()
        },
        raw_source_sha256: bridge_tally_core::RawSourceSha256::parse("c".repeat(64)).unwrap(),
        alter_id: None,
    }
}

fn core_window(party_name: &str) -> (RequestContext, CanonicalPackWindow) {
    let context = request_context();
    let core = CoreAccountingBatch {
        groups: vec![
            GroupRecord {
                source_id: "group-bank".to_string(),
                name: "Bank Accounts".to_string(),
                parent_source_id: None,
            },
            GroupRecord {
                source_id: "group-party".to_string(),
                name: "Sundry Debtors".to_string(),
                parent_source_id: None,
            },
        ],
        ledgers: vec![
            LedgerRecord {
                source_id: PARTY_GUID.to_string(),
                name: party_name.to_string(),
                parent_source_id: Some("group-party".to_string()),
                opening_balance: None,
            },
            LedgerRecord {
                source_id: BANK_GUID.to_string(),
                name: "Synthetic Bank".to_string(),
                parent_source_id: Some("group-bank".to_string()),
                opening_balance: None,
            },
            LedgerRecord {
                source_id: OTHER_BANK_GUID.to_string(),
                name: "Synthetic Bank Two".to_string(),
                parent_source_id: Some("group-bank".to_string()),
                opening_balance: None,
            },
        ],
        ..CoreAccountingBatch::default()
    };
    let source_counts = [
        ("group", core.groups.len() as u64),
        ("ledger", core.ledgers.len() as u64),
    ]
    .into_iter()
    .map(|(object_type, source_reported_count)| {
        let descriptor = SourceCountScopeDescriptor {
            source_identity: context.company.identity.clone(),
            pack: context.pack,
            pack_schema_version: context.schema_version,
            object_type: CanonicalText::parse(object_type).unwrap(),
            query_profile: context.query_profile.clone(),
            filters_sha256: context.filters_sha256.clone(),
            window: None,
        };
        SourceReportedCountEvidence {
            object_type: descriptor.object_type.clone(),
            query_profile: descriptor.query_profile.clone(),
            source_scope_fingerprint: source_count_scope_fingerprint(
                &descriptor,
                SourceCountScope::Complete,
            )
            .unwrap(),
            source_count_scope: SourceCountScope::Complete,
            source_reported_count,
        }
    })
    .collect();
    let record_evidence = core
        .groups
        .iter()
        .map(|group| evidence("group", &group.source_id))
        .chain(
            core.ledgers
                .iter()
                .map(|ledger| evidence("ledger", &ledger.source_id)),
        )
        .collect();
    (
        context,
        CanonicalPackWindow {
            batch: PackBatch::CoreAccounting(core),
            source_counts: Some(source_counts),
            record_evidence: Some(record_evidence),
        },
    )
}

fn catalog() -> CompanyLedgerCatalog {
    let (context, window) = core_window("Synthetic Party");
    CompanyLedgerCatalog::from_core_window(&context, &window, BANK_GUID).unwrap()
}

fn mappings(catalog: &CompanyLedgerCatalog) -> ImportLedgerMappings {
    ImportLedgerMappings::bind(
        catalog,
        vec![ImportLedgerMappingInput {
            source_ledger_key: "party-a".to_string(),
            ledger_guid: PARTY_GUID.to_string(),
            expected_exact_name: "Synthetic Party".to_string(),
        }],
    )
    .unwrap()
}

fn document(amount: &str, voucher_kind: &str, date: &str) -> Vec<u8> {
    format!(
        r#"{{"contract_version":1,"rows":[{{"source_row_id":"row-1","voucher_kind":"{voucher_kind}","date":"{date}","amount":"{amount}","counterparty_ledger_key":"party-a","narration":"Synthetic settlement"}}]}}"#
    )
    .into_bytes()
}

#[test]
fn plans_balanced_payment_without_dispatch_authority() {
    let catalog = catalog();
    let mappings = mappings(&catalog);
    let json = document("100.25", "payment", "20260401");
    let first = plan_payment_receipt_json(&json, &catalog, &mappings).unwrap();
    let second = plan_payment_receipt_json(&json, &catalog, &mappings).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.manifest().dry_run_state(), DryRunState::NotDispatched);
    assert_eq!(
        first.manifest().dispatch_authority(),
        DispatchAuthority::Absent
    );
    assert_eq!(
        first.manifest().unresolved_dispatch_preconditions().len(),
        7
    );
    assert_eq!(first.manifest().vouchers().len(), 1);
    assert_eq!(catalog.settlement_role(), SettlementLedgerRole::Bank);
    let voucher = &first.manifest().vouchers()[0];
    assert_eq!(voucher.voucher_kind(), VoucherKind::Payment);
    assert!(voucher.debits_equal_credits());
    assert_eq!(voucher.postings()[0].side(), PostingSide::Debit);
    assert_eq!(voucher.postings()[1].side(), PostingSide::Credit);
    assert_eq!(voucher.postings()[0].amount().as_str(), "100.25");
    assert_eq!(
        voucher.postings()[0].ledger().exact_name(),
        "Synthetic Party"
    );
    assert_eq!(voucher.postings()[1].ledger().ledger_guid(), BANK_GUID);
}

#[test]
fn receipt_reverses_explicit_posting_sides() {
    let catalog = catalog();
    let plan = plan_payment_receipt_json(
        &document("42", "receipt", "20260402"),
        &catalog,
        &mappings(&catalog),
    )
    .unwrap();
    let postings = plan.manifest().vouchers()[0].postings();
    assert_eq!(postings[0].side(), PostingSide::Credit);
    assert_eq!(postings[1].side(), PostingSide::Debit);
}

#[test]
fn rejects_json_numbers_exponents_commas_zero_and_negative_money() {
    let catalog = catalog();
    let mappings = mappings(&catalog);
    let number = String::from_utf8(document("100", "payment", "20260401"))
        .unwrap()
        .replace("\"amount\":\"100\"", "\"amount\":100");
    assert_eq!(
        plan_payment_receipt_json(number.as_bytes(), &catalog, &mappings),
        Err(StructuredImportError::InvalidJson)
    );
    for amount in ["1e3", "1,000", "0", "-0.00", "-1"] {
        let result = plan_payment_receipt_json(
            &document(amount, "payment", "20260401"),
            &catalog,
            &mappings,
        );
        let expected = if matches!(amount, "0" | "-0.00" | "-1") {
            StructuredImportError::NonPositiveAmount { ordinal: 0 }
        } else {
            StructuredImportError::InvalidJson
        };
        assert_eq!(result, Err(expected), "amount={amount}");
    }
}

#[test]
fn refuses_unknown_stale_and_settlement_ledger_mappings() {
    let catalog = catalog();
    let mappings = mappings(&catalog);
    let unknown = String::from_utf8(document("10", "payment", "20260401"))
        .unwrap()
        .replace("party-a", "unknown");
    assert_eq!(
        plan_payment_receipt_json(unknown.as_bytes(), &catalog, &mappings),
        Err(StructuredImportError::UnknownLedgerMapping { ordinal: 0 })
    );

    let (context, window) = core_window("Renamed Synthetic Party");
    let changed = CompanyLedgerCatalog::from_core_window(&context, &window, BANK_GUID).unwrap();
    assert_eq!(
        plan_payment_receipt_json(&document("10", "payment", "20260401"), &changed, &mappings),
        Err(StructuredImportError::StaleLedgerMapping)
    );

    let (mut refreshed_context, refreshed_window) = core_window("Synthetic Party");
    refreshed_context.run_id = "synthetic-run-2".to_string();
    let refreshed =
        CompanyLedgerCatalog::from_core_window(&refreshed_context, &refreshed_window, BANK_GUID)
            .unwrap();
    let refreshed_plan = plan_payment_receipt_json(
        &document("10", "payment", "20260401"),
        &refreshed,
        &mappings,
    )
    .unwrap();
    assert_eq!(refreshed_plan.manifest().source_run_id(), "synthetic-run-2");

    assert_eq!(
        ImportLedgerMappings::bind(
            &catalog,
            vec![ImportLedgerMappingInput {
                source_ledger_key: "wrong-role".to_string(),
                ledger_guid: OTHER_BANK_GUID.to_string(),
                expected_exact_name: "Synthetic Bank Two".to_string(),
            }]
        ),
        Err(StructuredImportError::InvalidLedgerMapping)
    );
}

#[test]
fn refuses_incomplete_evidence_invalid_settlement_role_and_out_of_window_dates() {
    let (context, mut window) = core_window("Synthetic Party");
    window.record_evidence = None;
    assert_eq!(
        CompanyLedgerCatalog::from_core_window(&context, &window, BANK_GUID),
        Err(StructuredImportError::InvalidSourceEvidence)
    );

    let (context, mut window) = core_window("Synthetic Party");
    window
        .source_counts
        .as_mut()
        .unwrap()
        .retain(|count| count.object_type.as_str() == "ledger");
    assert_eq!(
        CompanyLedgerCatalog::from_core_window(&context, &window, BANK_GUID),
        Err(StructuredImportError::InvalidSourceEvidence)
    );

    let (context, window) = core_window("Synthetic Party");
    assert_eq!(
        CompanyLedgerCatalog::from_core_window(&context, &window, PARTY_GUID),
        Err(StructuredImportError::InvalidSettlementLedger)
    );

    let catalog = catalog();
    assert_eq!(
        plan_payment_receipt_json(
            &document("10", "payment", "20260501"),
            &catalog,
            &mappings(&catalog)
        ),
        Err(StructuredImportError::VoucherDateOutsideAllowedWindow { ordinal: 0 })
    );
}

#[test]
fn rejects_duplicate_rows_unknown_fields_and_per_row_settlement_keys() {
    let catalog = catalog();
    let mappings = mappings(&catalog);
    let one = String::from_utf8(document("10", "payment", "20260401")).unwrap();
    let duplicate = one.replace("]}", &format!(",{}]}}", &one[30..one.len() - 2]));
    assert_eq!(
        plan_payment_receipt_json(duplicate.as_bytes(), &catalog, &mappings),
        Err(StructuredImportError::DuplicateRowIdentity { ordinal: 1 })
    );
    for invalid in [
        one.replace("\"rows\":", "\"unexpected\":true,\"rows\":"),
        one.replace(
            "\"counterparty_ledger_key\":",
            "\"cash_or_bank_ledger_key\":\"bank-a\",\"counterparty_ledger_key\":",
        ),
    ] {
        assert_eq!(
            plan_payment_receipt_json(invalid.as_bytes(), &catalog, &mappings),
            Err(StructuredImportError::InvalidJson)
        );
    }
}
