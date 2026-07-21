use bridge_tally_protocol::{
    decode_xml_bytes, decode_xml_bytes_limited, export_status, parse_companies,
    parse_companies_for_interactive_discovery, parse_companies_with_evidence,
    parse_group_source_records_with_evidence, parse_import_result,
    parse_ledger_source_records_with_evidence, parse_ledgers, parse_ledgers_with_evidence,
    parse_selected_voucher_source_records_with_evidence,
    parse_voucher_source_records_with_evidence, parse_voucher_type_source_records_with_evidence,
    parse_vouchers, parse_vouchers_with_evidence, validate_exact_selected_export_structure,
    verify_company_context, verify_selected_voucher_window_context, ParsedSourceIdentityKind,
    TallyExportStatus, BRIDGE_GROUP_EXPORT_SCHEMA, BRIDGE_LEDGER_EXPORT_SCHEMA,
    BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA, BRIDGE_VOUCHER_EXPORT_SCHEMA,
    BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA, MAX_INTERACTIVE_DISCOVERY_COMPANIES,
};
use sha2::{Digest, Sha256};
use tally_protocol_simulator::{
    generate_master_corpus, Fixture, MasterCorpusSpec, ScenarioPlan, WireEncoding,
};

#[test]
fn production_status_parser_distinguishes_all_application_states() {
    assert_eq!(
        export_status(&Fixture::ExportStatusOne.body()).unwrap(),
        TallyExportStatus::Success
    );
    assert_eq!(
        export_status(&Fixture::ExportStatusZero.body()).unwrap(),
        TallyExportStatus::Failure
    );
    let missing = export_status(&Fixture::ExportStatusMissing.body()).unwrap_err();
    assert!(missing
        .to_string()
        .contains("did not include HEADER/STATUS"));
    let invalid = export_status(&Fixture::ExportStatusInvalid.body()).unwrap_err();
    assert!(invalid.to_string().contains("invalid application STATUS"));
}

#[test]
fn export_status_rejects_duplicate_misplaced_and_active_xml_constructs() {
    for xml in [
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY/></ENVELOPE>",
        "<ENVELOPE><HEADER><VERSION>1</VERSION></HEADER><BODY><STATUS>1</STATUS></BODY></ENVELOPE>",
        "<!DOCTYPE ENVELOPE [<!ENTITY x '1'>]><ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>&x;</STATUS></HEADER><BODY/></ENVELOPE>",
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS><STATUS>0</STATUS></HEADER><BODY/></ENVELOPE>",
        "<ENVELOPE><HEADER><VERSION>1</VERSION><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY/></ENVELOPE>",
        "<ENVELOPE><BODY/><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER></ENVELOPE>",
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY/><BODY/></ENVELOPE>",
        "<ENVELOPE unsafe=\"1\"><HEADER><STATUS>1</STATUS></HEADER><BODY/></ENVELOPE>",
        "<ENVELOPE><HEADER>mixed<STATUS>1</STATUS></HEADER><BODY/></ENVELOPE>",
        "<ENVELOPE><HEADER><UNKNOWN>1</UNKNOWN><STATUS>1</STATUS></HEADER><BODY/></ENVELOPE>",
    ] {
        assert!(export_status(xml).is_err(), "must reject {xml}");
    }
}

#[test]
fn primary_rows_and_company_context_must_use_supported_export_parents() {
    let nested_ledger = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><UNEXPECTED><LEDGER NAME="BRIDGE LEDGER" GUID="ledger-guid"><PARENT>Primary</PARENT></LEDGER></UNEXPECTED></BODY></ENVELOPE>"#,
        BRIDGE_LEDGER_EXPORT_SCHEMA
    );
    assert!(parse_ledger_source_records_with_evidence(&nested_ledger).is_err());

    let nested_group = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="GROUP" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><UNEXPECTED><GROUP NAME="BRIDGE GROUP" GUID="group-guid"><PARENT>Primary</PARENT></GROUP></UNEXPECTED></BODY></ENVELOPE>"#,
        BRIDGE_GROUP_EXPORT_SCHEMA
    );
    assert!(parse_group_source_records_with_evidence(&nested_group).is_err());

    let nested_voucher = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><UNEXPECTED><VOUCHER GUID="voucher-guid"><LEDGERENTRYCOUNT>0</LEDGERENTRYCOUNT></VOUCHER></UNEXPECTED></BODY></ENVELOPE>"#,
        BRIDGE_VOUCHER_EXPORT_SCHEMA
    );
    assert!(parse_voucher_source_records_with_evidence(&nested_voucher).is_err());

    let nested_context = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><UNEXPECTED><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/></UNEXPECTED><LEDGER NAME="BRIDGE LEDGER" GUID="ledger-guid"><PARENT>Primary</PARENT></LEDGER></BODY></ENVELOPE>"#,
        BRIDGE_LEDGER_EXPORT_SCHEMA
    );
    assert!(parse_ledger_source_records_with_evidence(&nested_context).is_err());
}

#[test]
fn selected_voucher_v3_binds_exact_window_and_rejects_structural_siblings() {
    let xml = format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA>
<COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1" FROMDATE="20260701" TODATE="20260731"/>
<VOUCHER GUID="voucher-guid"><DATE>20260715</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><LEDGERENTRYCOUNT>0</LEDGERENTRYCOUNT></VOUCHER>
</DATA></BODY></ENVELOPE>"#,
        BRIDGE_SELECTED_VOUCHER_EXPORT_SCHEMA
    );
    validate_exact_selected_export_structure(&xml, "VOUCHER").unwrap();
    let parsed = parse_selected_voucher_source_records_with_evidence(&xml).unwrap();
    verify_company_context(&parsed.evidence, "COMPANY-GUID").unwrap();
    verify_selected_voucher_window_context(&parsed.evidence, "20260701", "20260731").unwrap();
    assert!(
        verify_selected_voucher_window_context(&parsed.evidence, "20260702", "20260731").is_err()
    );

    let with_sibling = xml.replace(
        "</DATA>",
        "<LEDGER NAME=\"unexpected\" GUID=\"ledger-guid\"/></DATA>",
    );
    assert!(validate_exact_selected_export_structure(&with_sibling, "VOUCHER").is_err());

    for invalid in [
        xml.replace("<DATA>", "<DATA unsafe=\"1\">"),
        xml.replace("<VOUCHER GUID", "<![CDATA[hidden]]><VOUCHER GUID"),
        xml.replace("<COMPANYCONTEXT", "<![CDATA[hidden]]><COMPANYCONTEXT"),
    ] {
        assert!(validate_exact_selected_export_structure(&invalid, "VOUCHER").is_err());
    }
}

#[test]
fn production_decoder_accepts_utf8_bom_and_utf16_equivalently() {
    let expected = Fixture::NormalExport.body().into_owned();
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf8Bom,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let wire = ScenarioPlan::new(Fixture::NormalExport)
            .with_encoding(encoding)
            .response_bytes();
        let decoded = decode_xml_bytes(&wire).expect("production decoder accepts fixture");
        assert_eq!(decoded, expected);
        assert_eq!(parse_ledgers(&decoded).unwrap().len(), 1);
    }
    assert!(decode_xml_bytes([0xFF, 0xFE, 0x00]).is_err());
    assert!(decode_xml_bytes([0x80]).is_err());
}

#[test]
fn generated_many_master_corpus_traverses_production_decoder_and_parser() {
    let spec = MasterCorpusSpec {
        total_records: 10_000,
        text_width: 8,
        seed: 29,
    };
    let mut reference_records = None;
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let (bytes, generated) = generate_master_corpus(spec, encoding).unwrap();
        let decoded = decode_xml_bytes(bytes).unwrap();
        let parsed = parse_ledger_source_records_with_evidence(&decoded).unwrap();
        assert_eq!(parsed.records.len(), spec.total_records as usize);
        verify_company_context(&parsed.evidence, "00000000-0000-4000-8000-000000000001").unwrap();
        assert_eq!(
            parsed_master_semantic_sha256(&parsed.records),
            generated.expected_semantic_sha256
        );
        match &reference_records {
            Some(expected) => assert_eq!(&parsed.records, expected),
            None => reference_records = Some(parsed.records),
        }
    }
}

fn parsed_master_semantic_sha256(
    records: &[bridge_tally_protocol::ParsedSourceRecord<bridge_tally_protocol::TallyLedger>],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"bridge.tally.synthetic-master-semantics/1\0");
    for (index, source) in records.iter().enumerate() {
        for (label, value) in [
            ("record_index", index.to_string()),
            ("name", source.record.name.clone()),
            ("guid", source.identities.guid.clone().unwrap_or_default()),
            (
                "remote_id",
                source.identities.remote_id.clone().unwrap_or_default(),
            ),
            (
                "master_id",
                source.identities.master_id.clone().unwrap_or_default(),
            ),
            ("alter_id", source.alter_id.clone().unwrap_or_default()),
            ("parent", source.record.parent.clone().unwrap_or_default()),
            (
                "opening_balance",
                source.record.opening_balance.clone().unwrap_or_default(),
            ),
        ] {
            semantic_field(&mut digest, label.as_bytes(), value.as_bytes());
        }
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn semantic_field(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u16).to_be_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

#[test]
fn production_byte_limit_is_checked_before_decoding() {
    let bytes = ScenarioPlan::new(Fixture::Oversized {
        minimum_bytes: 4096,
    })
    .response_bytes();
    let error = decode_xml_bytes_limited(&bytes, 1024).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tally response exceeded the 1024-byte limit"
    );
    assert!(decode_xml_bytes_limited(&bytes, bytes.len()).is_ok());
}

#[test]
fn malformed_and_truncated_corpus_never_returns_partial_records() {
    assert!(parse_ledgers(&Fixture::MalformedXml.body()).is_err());
    assert!(parse_ledgers(&Fixture::TruncatedXml.body()).is_err());
}

#[test]
fn ledger_parser_preserves_exact_decimal_text_and_company_evidence() {
    let parsed = parse_ledgers_with_evidence(&Fixture::NormalExport.body()).unwrap();
    assert_eq!(parsed.records.len(), 1);
    assert_eq!(parsed.records[0].name, "BRIDGE SYNTHETIC LEDGER");
    assert_eq!(
        parsed.records[0].opening_balance.as_deref(),
        Some("-1180.00")
    );
    assert_eq!(parsed.evidence.identified_record_count, 1);
    assert_eq!(
        parsed.evidence.schema.as_deref(),
        Some(BRIDGE_LEDGER_EXPORT_SCHEMA)
    );
    assert_eq!(parsed.evidence.object_type.as_deref(), Some("LEDGER"));
    assert_eq!(parsed.evidence.source_record_count, Some(1));
    let context = parsed.evidence.company_context.as_ref().unwrap();
    assert_eq!(context.name.as_deref(), Some("BRIDGE SYNTHETIC BOOK"));
    assert_eq!(
        context.guid.as_deref(),
        Some("00000000-0000-4000-8000-000000000001")
    );
    verify_company_context(&parsed.evidence, "00000000-0000-4000-8000-000000000001").unwrap();
}

#[test]
fn source_record_parsers_preserve_identity_kind_and_exact_fragment_hash() {
    let xml = Fixture::NormalExport.body();
    let ledgers = parse_ledger_source_records_with_evidence(&xml).unwrap();
    assert_eq!(ledgers.records.len(), 1);
    assert_eq!(
        ledgers.records[0].identity_kind,
        Some(ParsedSourceIdentityKind::Guid)
    );
    assert_eq!(
        ledgers.records[0].source_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000101")
    );
    assert_eq!(ledgers.records[0].raw_source_sha256.len(), 64);
    assert_eq!(
        ledgers.records[0].raw_source_sha256,
        parse_ledger_source_records_with_evidence(&xml)
            .unwrap()
            .records[0]
            .raw_source_sha256
    );

    let vouchers =
        parse_voucher_source_records_with_evidence(&Fixture::VoucherExport.body()).unwrap();
    assert_eq!(
        vouchers.records[0].identity_kind,
        Some(ParsedSourceIdentityKind::Guid)
    );
    assert_eq!(
        vouchers.records[0].source_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000201")
    );
    assert_eq!(
        vouchers.records[0].identities.remote_id.as_deref(),
        Some("bridge-synthetic-voucher-001")
    );
    assert_eq!(vouchers.records[0].raw_source_sha256.len(), 64);

    let mixed_case_guid = xml.replace(
        "00000000-0000-4000-8000-000000000101",
        "00000000-0000-4000-8000-000000000AaB",
    );
    let mixed_case = parse_ledger_source_records_with_evidence(&mixed_case_guid).unwrap();
    assert_eq!(
        mixed_case.records[0].source_id.as_deref(),
        Some("00000000-0000-4000-8000-000000000aab")
    );
    assert_eq!(
        mixed_case.records[0].identities.guid.as_deref(),
        Some("00000000-0000-4000-8000-000000000aab")
    );
}

#[test]
fn full_core_master_and_nested_entry_contracts_are_strict() {
    let group_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="GROUP" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><GROUP NAME="BRIDGE GROUP" GUID="group-guid" REMOTEID="group-remote" MASTERID="7" ALTERID="11"><PARENT>Primary</PARENT></GROUP></BODY></ENVELOPE>"#,
        BRIDGE_GROUP_EXPORT_SCHEMA
    );
    let groups = parse_group_source_records_with_evidence(&group_xml).unwrap();
    assert_eq!(groups.records[0].source_id.as_deref(), Some("group-guid"));
    assert_eq!(groups.records[0].identities.master_id.as_deref(), Some("7"));
    assert_eq!(groups.records[0].alter_id.as_deref(), Some("11"));

    let voucher_type_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHERTYPE" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><VOUCHERTYPE NAME="Receipt" MASTERID="9" ALTERID="12"><PARENT>Receipt</PARENT></VOUCHERTYPE></BODY></ENVELOPE>"#,
        BRIDGE_VOUCHER_TYPE_EXPORT_SCHEMA
    );
    let voucher_types = parse_voucher_type_source_records_with_evidence(&voucher_type_xml).unwrap();
    assert_eq!(
        voucher_types.records[0].identity_kind,
        Some(ParsedSourceIdentityKind::MasterId)
    );

    let voucher_xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="VOUCHER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><VOUCHER GUID="voucher-guid" REMOTEID="voucher-remote" MASTERID="10" ALTERID="13"><DATE>20260701</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><VOUCHERNUMBER>BRIDGE-1</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><LEDGERENTRYCOUNT>2</LEDGERENTRYCOUNT><LEDGERENTRIES><LEDGERENTRY><ENTRYINDEX>1</ENTRYINDEX><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>-100.00</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE></LEDGERENTRY><LEDGERENTRY><ENTRYINDEX>2</ENTRYINDEX><LEDGERNAME>Sales</LEDGERNAME><AMOUNT>100.00</AMOUNT><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE></LEDGERENTRY></LEDGERENTRIES></VOUCHER></BODY></ENVELOPE>"#,
        BRIDGE_VOUCHER_EXPORT_SCHEMA
    );
    let vouchers = parse_voucher_source_records_with_evidence(&voucher_xml).unwrap();
    assert_eq!(vouchers.records[0].record.ledger_entries.len(), 2);
    assert_eq!(vouchers.records[0].record.ledger_entries[0].entry_index, 1);
    assert_eq!(
        vouchers.records[0].record.ledger_entries[0].amount,
        "-100.00"
    );
    assert_eq!(
        vouchers.records[0].record.ledger_entries[0]
            .raw_source_sha256
            .len(),
        64
    );

    let bad_count = voucher_xml.replace(
        "<LEDGERENTRYCOUNT>2</LEDGERENTRYCOUNT>",
        "<LEDGERENTRYCOUNT>1</LEDGERENTRYCOUNT>",
    );
    assert!(parse_voucher_source_records_with_evidence(&bad_count).is_err());

    let missing_polarity = voucher_xml.replace("<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>", "");
    assert!(parse_voucher_source_records_with_evidence(&missing_polarity).is_err());
    let invalid_polarity = voucher_xml.replace(
        "<ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
        "<ISDEEMEDPOSITIVE>Maybe</ISDEEMEDPOSITIVE>",
    );
    assert!(parse_voucher_source_records_with_evidence(&invalid_polarity).is_err());
}

#[test]
fn wrong_company_and_duplicate_identities_are_explicit_safe_evidence() {
    let wrong = parse_ledgers_with_evidence(&Fixture::WrongCompany.body()).unwrap();
    let error = verify_company_context(&wrong.evidence, "00000000-0000-4000-8000-000000000001")
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tally response company context did not match the request"
    );
    assert!(!error.to_string().contains("00000000"));

    let duplicate = parse_ledgers_with_evidence(&Fixture::DuplicateIdentity.body()).unwrap();
    assert_eq!(duplicate.records.len(), 2);
    assert_eq!(duplicate.evidence.identified_record_count, 2);
    assert_eq!(duplicate.evidence.duplicate_identities.len(), 1);
    assert_eq!(duplicate.evidence.duplicate_identities[0].occurrences, 2);
    assert_eq!(
        duplicate.evidence.duplicate_identities[0]
            .identity_sha256
            .len(),
        64
    );
    let serialized = serde_json::to_string(&duplicate.evidence).unwrap();
    assert!(!serialized.contains("00000000-0000-4000-8000-000000000199"));
}

#[test]
fn child_and_attribute_metadata_shapes_are_strictly_equivalent() {
    let child = parse_ledgers_with_evidence(&Fixture::NormalExport.body()).unwrap();
    assert_eq!(
        child.evidence.schema.as_deref(),
        Some(BRIDGE_LEDGER_EXPORT_SCHEMA)
    );
    assert_eq!(child.evidence.source_record_count, Some(1));

    let attributes = parse_vouchers_with_evidence(&Fixture::VoucherExport.body()).unwrap();
    assert_eq!(attributes.records.len(), 1);
    assert_eq!(
        attributes.evidence.schema.as_deref(),
        Some(BRIDGE_VOUCHER_EXPORT_SCHEMA)
    );
    assert_eq!(attributes.evidence.object_type.as_deref(), Some("VOUCHER"));
    assert_eq!(attributes.evidence.source_record_count, Some(1));
    assert_eq!(
        attributes.records[0].id.as_deref(),
        Some("00000000-0000-4000-8000-000000000201")
    );
}

#[test]
fn a_proven_empty_export_is_not_confused_with_missing_evidence() {
    let empty = parse_ledgers_with_evidence(&Fixture::EmptyExport.body()).unwrap();
    assert!(empty.records.is_empty());
    assert_eq!(empty.evidence.source_record_count, Some(0));
    assert_eq!(empty.evidence.object_type.as_deref(), Some("LEDGER"));

    let missing = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY /></ENVELOPE>"#;
    let error = parse_ledgers_with_evidence(missing).unwrap_err();
    assert_eq!(error.to_string(), "Tally response omitted company context");
}

#[test]
fn count_mismatch_and_malformed_or_duplicate_metadata_fail_closed() {
    let mismatch = parse_ledgers_with_evidence(&Fixture::RecordCountMismatch.body()).unwrap_err();
    assert_eq!(
        mismatch.to_string(),
        "Tally source record count did not match parsed primary rows"
    );

    let malformed =
        parse_ledgers_with_evidence(&Fixture::MalformedExportMetadata.body()).unwrap_err();
    assert_eq!(
        malformed.to_string(),
        "Tally company context RECORDCOUNT was not a non-negative integer"
    );

    let duplicate =
        parse_ledgers_with_evidence(&Fixture::DuplicateExportMetadata.body()).unwrap_err();
    assert_eq!(
        duplicate.to_string(),
        "Tally company context contained duplicate metadata"
    );
}

#[test]
fn primary_rows_reject_case_insensitive_duplicate_identity_attributes() {
    let xml = format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="{}" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="company-guid" RECORDCOUNT="1"/><LEDGER NAME="BRIDGE SYNTHETIC LEDGER" GUID="guid-a" guid="guid-b"><PARENT>Primary</PARENT></LEDGER></BODY></ENVELOPE>"#,
        BRIDGE_LEDGER_EXPORT_SCHEMA
    );
    let error = parse_ledger_source_records_with_evidence(&xml)
        .expect_err("case-insensitive duplicate identity attributes must fail closed");
    assert!(error.to_string().contains("repeated an attribute"));
}

#[test]
fn schema_and_object_type_are_bound_to_the_public_parser() {
    let wrong_schema = Fixture::NormalExport
        .body()
        .replace(BRIDGE_LEDGER_EXPORT_SCHEMA, BRIDGE_VOUCHER_EXPORT_SCHEMA);
    let error = parse_ledgers_with_evidence(&wrong_schema).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tally response export schema did not match the parser"
    );

    let wrong_object = Fixture::NormalExport.body().replace(
        "<OBJECTTYPE>LEDGER</OBJECTTYPE>",
        "<OBJECTTYPE>VOUCHER</OBJECTTYPE>",
    );
    let error = parse_ledgers_with_evidence(&wrong_object).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tally response object type did not match the parser"
    );
}

#[test]
fn multiple_company_contexts_are_rejected_as_ambiguous() {
    let xml = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY>
      <COMPANYCONTEXT><SCHEMA>bridge.tally.ledgers/1</SCHEMA><OBJECTTYPE>LEDGER</OBJECTTYPE><NAME>BRIDGE SYNTHETIC COMPANY A</NAME><GUID>guid-a</GUID><RECORDCOUNT>0</RECORDCOUNT></COMPANYCONTEXT>
      <COMPANYCONTEXT><SCHEMA>bridge.tally.ledgers/1</SCHEMA><OBJECTTYPE>LEDGER</OBJECTTYPE><NAME>BRIDGE SYNTHETIC COMPANY B</NAME><GUID>guid-b</GUID><RECORDCOUNT>0</RECORDCOUNT></COMPANYCONTEXT>
    </BODY></ENVELOPE>"#;
    let error = parse_ledgers_with_evidence(xml).expect_err("ambiguous context must fail closed");
    assert!(error.to_string().contains("multiple company contexts"));
}

#[test]
fn duplicate_identity_evidence_is_object_scoped_and_uses_guid_fallback() {
    let cross_type = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY>
      <COMPANYCONTEXT SCHEMA="bridge.tally.ledgers/1" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="guid-company" RECORDCOUNT="1" />
      <LEDGER REMOTEID="shared-id" NAME="BRIDGE SYNTHETIC LEDGER" />
      <VOUCHER REMOTEID="shared-id" />
    </BODY></ENVELOPE>"#;
    let evidence = parse_ledgers_with_evidence(cross_type).unwrap().evidence;
    assert_eq!(evidence.identified_record_count, 2);
    assert!(evidence.duplicate_identities.is_empty());

    let duplicate_guid = r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY>
      <COMPANYCONTEXT SCHEMA="bridge.tally.ledgers/1" OBJECTTYPE="LEDGER" NAME="BRIDGE SYNTHETIC BOOK" GUID="guid-company" RECORDCOUNT="2" />
      <LEDGER GUID="synthetic-guid" NAME="BRIDGE SYNTHETIC LEDGER A" />
      <LEDGER GUID="synthetic-guid" NAME="BRIDGE SYNTHETIC LEDGER B" />
    </BODY></ENVELOPE>"#;
    let evidence = parse_ledgers_with_evidence(duplicate_guid)
        .unwrap()
        .evidence;
    assert_eq!(evidence.identified_record_count, 2);
    assert_eq!(evidence.duplicate_identities.len(), 1);
    assert_eq!(evidence.duplicate_identities[0].occurrences, 2);

    let case_variant_guid = duplicate_guid.replace(
        "<LEDGER GUID=\"synthetic-guid\" NAME=\"BRIDGE SYNTHETIC LEDGER B\"",
        "<LEDGER GUID=\"SYNTHETIC-GUID\" NAME=\"BRIDGE SYNTHETIC LEDGER B\"",
    );
    let evidence = parse_ledgers_with_evidence(&case_variant_guid)
        .unwrap()
        .evidence;
    assert_eq!(evidence.duplicate_identities.len(), 1);
    assert_eq!(evidence.duplicate_identities[0].occurrences, 2);
}

#[test]
fn production_import_parser_preserves_every_exact_counter() {
    let result = parse_import_result(&Fixture::ImportCounters.body()).unwrap();
    assert_eq!(result.created, 2);
    assert_eq!(result.altered, 3);
    assert_eq!(result.deleted, 1);
    assert_eq!(result.ignored, 0);
    assert_eq!(result.errors, 0);
    assert_eq!(result.cancelled, 0);
    assert_eq!(result.exceptions, 0);
    assert_eq!(result.line_error_count, 0);
    assert!(result.is_clean_success());

    let duplicate = parse_import_result(&Fixture::ImportDuplicate.body()).unwrap();
    assert_eq!(duplicate.ignored, 1);
    assert_eq!(duplicate.errors, 1);
    assert_eq!(duplicate.line_error_count, 1);
    assert!(!duplicate.is_clean_success());

    let partial = parse_import_result(&Fixture::ImportPartial.body()).unwrap();
    assert_eq!(partial.created, 1);
    assert_eq!(partial.altered, 1);
    assert_eq!(partial.ignored, 1);
    assert_eq!(partial.errors, 1);
    assert_eq!(partial.exceptions, 1);
    assert_eq!(partial.line_error_count, 1);
}

#[test]
fn application_failure_errors_do_not_echo_line_error_payloads() {
    let error = parse_ledgers(&Fixture::ExportStatusZero.body()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tally reported that the export request failed"
    );
    assert!(!error.to_string().contains("BRIDGE_SYNTHETIC"));
}

#[test]
fn legacy_company_and_voucher_api_shapes_are_preserved() {
    let companies = parse_companies(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000001</COMPANYGUIDFIELD></COMPANYINFO></BODY></ENVELOPE>"#,
    )
    .unwrap();
    assert_eq!(companies.len(), 1);
    assert_eq!(companies[0].name, "BRIDGE SYNTHETIC BOOK");

    let vouchers = parse_vouchers(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><COMPANYCONTEXT SCHEMA="bridge.tally.vouchers/2" OBJECTTYPE="VOUCHER" NAME="BRIDGE SYNTHETIC BOOK" GUID="guid-company" RECORDCOUNT="1" /><VOUCHER REMOTEID="bridge-synthetic-voucher-001"><DATE>20260701</DATE><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><VOUCHERNUMBER>BRIDGE-001</VOUCHERNUMBER><PARTYLEDGERNAME>BRIDGE SYNTHETIC LEDGER</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><LEDGERENTRYCOUNT>0</LEDGERENTRYCOUNT></VOUCHER></BODY></ENVELOPE>"#,
    )
    .unwrap();
    assert_eq!(vouchers.len(), 1);
    assert_eq!(
        vouchers[0].id.as_deref(),
        Some("bridge-synthetic-voucher-001")
    );
    assert_eq!(vouchers[0].date.as_deref(), Some("20260701"));
    assert_eq!(vouchers[0].cancelled, Some(false));
}

#[test]
fn direct_company_report_response_is_narrowly_admitted_without_status_header() {
    let direct = r#"<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>"#;
    let companies = parse_companies_for_interactive_discovery(direct).unwrap();
    assert_eq!(companies.len(), 1);
    assert_eq!(companies[0].name, "BRIDGE DIRECT SYNTHETIC BOOK");
    assert!(parse_companies(direct).is_err());
    assert!(parse_companies_with_evidence(direct).is_err());

    for xml in [
        "<ENVELOPE/>",
        "<ENVELOPE><BODY><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></BODY></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYNAMEFIELD>duplicate</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD><UNEXPECTED>value</UNEXPECTED></COMPANYINFO></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD><UNEXPECTED/>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD><![CDATA[BRIDGE DIRECT SYNTHETIC BOOK]]></COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK<!-- comment --></COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>&#x20;</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>&#x20;</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
    ] {
        assert!(parse_companies_for_interactive_discovery(xml).is_err(), "must reject {xml}");
    }

    assert!(parse_ledgers(
        "<ENVELOPE><COMPANYINFO><COMPANYNAMEFIELD>BRIDGE DIRECT SYNTHETIC BOOK</COMPANYNAMEFIELD><COMPANYGUIDFIELD>00000000-0000-4000-8000-000000000099</COMPANYGUIDFIELD></COMPANYINFO></ENVELOPE>",
    )
    .is_err());
}

#[test]
fn interactive_company_discovery_stops_before_materializing_an_oversized_listing() {
    let rows = (0..=MAX_INTERACTIVE_DISCOVERY_COMPANIES)
        .map(|index| format!("<COMPANYINFO><COMPANYNAMEFIELD>Synthetic {index}</COMPANYNAMEFIELD><COMPANYGUIDFIELD>guid-{index}</COMPANYGUIDFIELD></COMPANYINFO>"))
        .collect::<String>();
    let error = parse_companies_for_interactive_discovery(&format!("<ENVELOPE>{rows}</ENVELOPE>"))
        .expect_err("untrusted discovery must stop at the display ceiling");
    assert!(error.to_string().contains("listing limit exceeded"));
}

#[test]
fn incomplete_or_invalid_imports_are_rejected() {
    assert!(parse_import_result("<RESPONSE><CREATED>1</CREATED></RESPONSE>").is_err());
    assert!(parse_import_result("<RESPONSE><CREATED>1</CREATED>").is_err());
    assert!(parse_import_result("<RESPONSE><CREATED>-1</CREATED><ALTERED>0</ALTERED><IGNORED>0</IGNORED><ERRORS>0</ERRORS><EXCEPTIONS>0</EXCEPTIONS></RESPONSE>").is_err());
}
