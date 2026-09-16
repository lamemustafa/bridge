use super::*;

fn id(value: &str) -> SourceRecordId {
    SourceRecordId::parse(value).expect("valid synthetic source id")
}

fn decimal(value: &str) -> ExactDecimal {
    ExactDecimal::parse(value).expect("valid exact decimal")
}

fn text(value: &str) -> CanonicalText {
    CanonicalText::parse(value).expect("valid canonical text")
}

fn non_negative(value: &str) -> NonNegativeExactDecimal {
    NonNegativeExactDecimal::parse(value).expect("valid non-negative exact decimal")
}

#[test]
fn canonical_pack_models_round_trip_with_exact_values_and_references() {
    let tax = IndiaTaxBatch {
        tax_registrations: vec![TaxRegistrationRecord {
            source_id: id("tax-registration:1"),
            owner_kind: TaxRegistrationOwnerKind::Ledger,
            owner_source_id: id("ledger:customer"),
            registration_type: text("regular"),
            gstin: Gstin::parse("27ABCDE1234F1Z5").unwrap(),
        }],
        voucher_taxes: vec![VoucherTaxRecord {
            source_id: id("voucher-tax:1"),
            voucher_source_id: id("voucher:1"),
            place_of_supply: text("27"),
            assessable_value: decimal("1000.00"),
            tax_component: text("igst"),
            tax_rate: non_negative("18.00"),
            tax_amount: decimal("180.00"),
        }],
    };
    tax.validate().expect("valid tax batch");
    let encoded = serde_json::to_string(&tax).expect("serialize tax batch");
    let decoded: IndiaTaxBatch = serde_json::from_str(&encoded).expect("deserialize tax batch");
    assert_eq!(decoded, tax);
    assert_eq!(decoded.voucher_taxes[0].tax_amount.as_str(), "180.00");

    let bills = BillsAndPaymentsBatch {
        parties: vec![PartyOutstandingFacts {
            source_identity: SourceIdentity {
                bridge_source_lineage: "bridge-source:test".to_string(),
                company_guid: "company-guid:test".to_string(),
                observed_fingerprint: "b".repeat(64),
            },
            party_ledger_source_id: id("ledger:customer"),
            report_as_of_yyyymmdd: TallyDate::parse("20260228").unwrap(),
            direction: OutstandingDirection::Receivable,
            bill_wise_state: BillWiseState::EnabledObserved,
            allocation_coverage: BillsCoverageState::ObservedCompleteScope,
            outstanding_coverage: BillsCoverageState::ObservedCompleteScope,
            fetch_bracket: FetchBracketState::StableObserved,
            query_profile: text("bills-confidence-v1"),
            source_scope_fingerprint: text(&"a".repeat(64)),
            source_reported_allocation_count: 1,
            source_reported_outstanding_count: 0,
            allocations: vec![BillAllocationRecord {
                source_id: id("bill-allocation:1"),
                identity_basis: DerivedIdentityBasis::ParentOrdinal,
                origin: BillAllocationOrigin::Voucher {
                    voucher_source_id: id("voucher:1"),
                    party_entry_source_id: id("entry:1"),
                },
                reference: BillReference {
                    kind: BillReferenceKind::NewReference,
                    name: Some(text("INV-0001")),
                    raw_kind: None,
                },
                bill_date_yyyymmdd: Some(TallyDate::parse("20260201").unwrap()),
                effective_date_yyyymmdd: None,
                due_date_yyyymmdd: Some(TallyDate::parse("20260228").unwrap()),
                due_date_evidence: BillDueDateEvidence::Explicit,
                amount: decimal("-1180.00"),
                observed_polarity: Some(crate::LedgerEntryPolarity::Debit),
                currency_basis: CurrencyBasis::CompanyBase {
                    currency: text("company-base"),
                },
            }],
            outstanding: Vec::new(),
        }],
    };
    bills.validate().expect("valid bills batch");
    assert_eq!(
        serde_json::from_value::<BillsAndPaymentsBatch>(
            serde_json::to_value(&bills).expect("serialize bills batch")
        )
        .expect("deserialize bills batch"),
        bills
    );

    let mut invalid = bills.clone();
    invalid.parties[0].allocations[0].reference = BillReference {
        kind: BillReferenceKind::OnAccount,
        name: Some(text("invented-link")),
        raw_kind: None,
    };
    assert!(invalid.validate().is_err());

    let mut invalid = bills.clone();
    invalid.parties[0].allocations[0].due_date_evidence = BillDueDateEvidence::Unavailable;
    assert!(invalid.validate().is_err());

    let mut invalid = bills.clone();
    invalid.parties[0].allocation_coverage = BillsCoverageState::ObservedPartial;
    invalid.parties[0].source_reported_allocation_count = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = bills.clone();
    invalid.parties[0].source_scope_fingerprint = text("not-a-sha256");
    assert!(invalid.validate().is_err());

    let mut invalid = bills.clone();
    invalid.parties[0].allocations[0].identity_basis =
        DerivedIdentityBasis::MutableReferenceOrdinal;
    assert!(invalid.validate().is_err());

    let inventory = InventoryBatch {
        stock_items: vec![StockItemRecord {
            source_id: id("stock-item:1"),
            name: text("Synthetic Item"),
            base_unit: text("nos"),
        }],
        godowns: vec![GodownRecord {
            source_id: id("godown:1"),
            name: text("Synthetic Location"),
        }],
        inventory_entries: vec![InventoryEntryRecord {
            source_id: id("inventory-entry:1"),
            voucher_source_id: id("voucher:1"),
            stock_item_source_id: id("stock-item:1"),
            godown_source_id: id("godown:1"),
            quantity: decimal("2.000"),
            rate: decimal("500.00"),
            amount: decimal("1000.00"),
        }],
    };
    inventory.validate().expect("valid inventory batch");
    assert_eq!(
        serde_json::from_value::<InventoryBatch>(
            serde_json::to_value(&inventory).expect("serialize inventory batch")
        )
        .expect("deserialize inventory batch"),
        inventory
    );
}

#[test]
fn derived_bill_ids_bind_parent_scope_and_ordinal_not_mutable_values() {
    let company = text(&"c".repeat(64));
    let party = id("ledger:party");
    let parent = id("voucher:1");
    let first = derive_bill_allocation_source_id(&company, &party, &parent, 1).unwrap();
    let same = derive_bill_allocation_source_id(&company, &party, &parent, 1).unwrap();
    let next = derive_bill_allocation_source_id(&company, &party, &parent, 2).unwrap();
    assert_eq!(first, same, "amount and due date are not identity inputs");
    assert_ne!(first, next);
    assert!(derive_bill_allocation_source_id(&company, &party, &parent, 0).is_err());

    let scope = text(&"a".repeat(64));
    assert_ne!(
        derive_bill_outstanding_source_id(&scope, 1).unwrap(),
        derive_bill_outstanding_source_id(&scope, 2).unwrap()
    );
}

#[test]
fn missing_fields_unknown_fields_and_invalid_exact_decimals_fail_deserialization() {
    assert!(serde_json::from_str::<InventoryEntryRecord>(
        r#"{
                "source_id":"entry:1",
                "voucher_source_id":"voucher:1",
                "stock_item_source_id":"item:1",
                "godown_source_id":"godown:1",
                "quantity":"1",
                "rate":"10.00"
            }"#
    )
    .is_err());
    assert!(serde_json::from_str::<InventoryBatch>(
        r#"{"stock_items":[],"godowns":[],"inventory_entries":[],"schema_version":1}"#
    )
    .is_err());
    assert!(serde_json::from_str::<VoucherTaxRecord>(
        r#"{
                "source_id":"tax:1",
                "voucher_source_id":"voucher:1",
                "place_of_supply":"27",
                "assessable_value":"100.00",
                "tax_component":"igst",
                "tax_rate":"NaN",
                "tax_amount":"18.00"
            }"#
    )
    .is_err());
    assert!(serde_json::from_str::<VoucherTaxRecord>(
        r#"{
                "source_id":"tax:1",
                "voucher_source_id":"voucher:1",
                "place_of_supply":"27",
                "assessable_value":"100.00",
                "tax_component":"igst",
                "tax_rate":"-18.00",
                "tax_amount":"18.00"
            }"#
    )
    .is_err());
}

#[test]
fn typed_ids_and_dates_reject_ambiguous_values_at_construction_and_deserialization() {
    for value in ["", " leading", "trailing ", "line\nbreak"] {
        assert!(SourceRecordId::parse(value).is_err(), "accepted {value:?}");
    }
    for value in ["20260229", "20261301", "20260001", "00000101", "2026-01-01"] {
        assert!(TallyDate::parse(value).is_err(), "accepted {value}");
    }
    assert!(TallyDate::parse("20240229").is_ok());
    assert!(serde_json::from_str::<TallyDate>(r#""20260229""#).is_err());
    assert_eq!(
        TallyDate::parse("20240228")
            .unwrap()
            .next_day()
            .unwrap()
            .as_str(),
        "20240229"
    );
    assert_eq!(
        TallyDate::parse("20241231")
            .unwrap()
            .next_day()
            .unwrap()
            .as_str(),
        "20250101"
    );
    assert!(TallyDate::parse("99991231").unwrap().next_day().is_err());
    for value in ["", " leading", "trailing ", "line\nbreak"] {
        assert!(CanonicalText::parse(value).is_err(), "accepted {value:?}");
    }
    for value in ["", "27abcde1234f1z5", "27ABCDE1234F1Z"] {
        assert!(Gstin::parse(value).is_err(), "accepted {value:?}");
    }
}

#[test]
fn semantic_validation_rejects_duplicate_ids_and_unpopulated_required_text() {
    let duplicate = StockItemRecord {
        source_id: id("item:1"),
        name: text("Item"),
        base_unit: text("nos"),
    };
    let inventory = InventoryBatch {
        stock_items: vec![duplicate.clone(), duplicate],
        godowns: Vec::new(),
        inventory_entries: Vec::new(),
    };
    assert!(matches!(
        inventory.validate(),
        Err(TallyError::InvalidData { code }) if code == "duplicate_stock_item_source_id"
    ));

    assert!(serde_json::from_str::<VoucherTaxRecord>(
        r#"{
                "source_id":"tax:1",
                "voucher_source_id":"voucher:1",
                "place_of_supply":"",
                "assessable_value":"100.00",
                "tax_component":"igst",
                "tax_rate":"18.00",
                "tax_amount":"18.00"
            }"#
    )
    .is_err());
}

#[test]
fn source_counts_are_optional_explicit_evidence_never_derived_from_records() {
    let batch = InventoryBatch {
        stock_items: vec![StockItemRecord {
            source_id: id("item:1"),
            name: text("Synthetic Item"),
            base_unit: text("nos"),
        }],
        godowns: Vec::new(),
        inventory_entries: Vec::new(),
    };
    let absent = PackWindow::without_source_count_evidence(batch.clone());
    assert_eq!(absent.source_counts, None);
    absent
        .validate_source_count_evidence()
        .expect("absent source evidence is honest");

    let observed = PackWindow {
        batch,
        source_counts: Some(vec![SourceReportedCountEvidence {
            object_type: text("stock_item"),
            query_profile: text("inventory-v1"),
            source_scope_fingerprint: text("scope-sha256:synthetic"),
            source_count_scope: SourceCountScope::Complete,
            // Deliberately differs from the parsed Vec length. The model
            // stores the source claim without inventing reconciliation.
            source_reported_count: 37,
        }]),
        record_evidence: None,
    };
    observed
        .validate_source_count_evidence()
        .expect("valid explicit evidence");
    assert_eq!(
        observed.source_counts.as_ref().unwrap()[0].source_reported_count,
        37
    );
}

#[test]
fn source_count_fingerprint_binds_exact_versioned_scope() {
    let descriptor = SourceCountScopeDescriptor {
        source_identity: SourceIdentity {
            bridge_source_lineage: "lineage:synthetic".to_string(),
            company_guid: "company:synthetic".to_string(),
            observed_fingerprint: "fingerprint:synthetic".to_string(),
        },
        pack: CapabilityPackId::Inventory,
        pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
        object_type: text("stock_item"),
        query_profile: text("inventory-v1"),
        filters_sha256: text(&"a".repeat(64)),
        window: None,
    };
    let fingerprint = source_count_scope_fingerprint(&descriptor, SourceCountScope::Complete)
        .expect("valid complete scope fingerprint");
    assert_eq!(fingerprint.as_str().len(), 64);
    assert_eq!(
        fingerprint,
        source_count_scope_fingerprint(&descriptor, SourceCountScope::Complete).unwrap()
    );

    let evidence = SourceReportedCountEvidence {
        object_type: descriptor.object_type.clone(),
        query_profile: descriptor.query_profile.clone(),
        source_scope_fingerprint: fingerprint,
        source_count_scope: SourceCountScope::Complete,
        source_reported_count: 0,
    };
    assert!(evidence.matches_scope_descriptor(&descriptor).unwrap());

    let mut drifted = descriptor.clone();
    drifted.query_profile = text("inventory-v2");
    assert!(!evidence.matches_scope_descriptor(&drifted).unwrap());
    assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Window).is_err());
}

#[test]
fn window_count_fingerprint_requires_exact_valid_window() {
    let mut descriptor = SourceCountScopeDescriptor {
        source_identity: SourceIdentity {
            bridge_source_lineage: "lineage:synthetic".to_string(),
            company_guid: "company:synthetic".to_string(),
            observed_fingerprint: "fingerprint:synthetic".to_string(),
        },
        pack: CapabilityPackId::CoreAccounting,
        pack_schema_version: PackSchemaVersion { major: 1, minor: 0 },
        object_type: text("voucher"),
        query_profile: text("voucher-v1"),
        filters_sha256: text(&"b".repeat(64)),
        window: Some(ReadWindow {
            from_yyyymmdd: "20260101".to_string(),
            to_yyyymmdd: "20260131".to_string(),
        }),
    };
    assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Window).is_ok());
    assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Complete).is_err());

    descriptor.window.as_mut().unwrap().from_yyyymmdd = "20260201".to_string();
    assert!(source_count_scope_fingerprint(&descriptor, SourceCountScope::Window).is_err());
}

#[test]
fn empty_or_duplicate_source_count_evidence_fails_closed() {
    let empty = PackWindow {
        batch: IndiaTaxBatch::default(),
        source_counts: Some(Vec::new()),
        record_evidence: None,
    };
    assert!(matches!(
        empty.validate_source_count_evidence(),
        Err(TallyError::InvalidData { code }) if code == "source_count_evidence_empty"
    ));

    let count = SourceReportedCountEvidence {
        object_type: text("voucher_tax"),
        query_profile: text("tax-v1"),
        source_scope_fingerprint: text("scope-sha256:synthetic"),
        source_count_scope: SourceCountScope::Complete,
        source_reported_count: 2,
    };
    let duplicate = PackWindow {
        batch: IndiaTaxBatch::default(),
        source_counts: Some(vec![count.clone(), count]),
        record_evidence: None,
    };
    assert!(matches!(
        duplicate.validate_source_count_evidence(),
        Err(TallyError::InvalidData { code }) if code == "source_count_evidence_duplicate_scope"
    ));
}
