use super::*;

fn profiles<'a>(
    company: &'a ValidatedCompanyName,
    range: &'a ValidatedDateRange,
    canary_ledger: &'a ValidatedCanaryLedgerName,
    identity_query_sha256: &'a ValidatedIdentityQuerySha256,
) -> [ReadOnlyProfile<'a>; 9] {
    [
        ReadOnlyProfile::CompanyListV1,
        ReadOnlyProfile::CompanyListV2,
        ReadOnlyProfile::CompanyBookExtentV2 { company },
        ReadOnlyProfile::StandardLedgerIdentityV1 { company },
        ReadOnlyProfile::StandardLedgerCatalogV1 { company },
        ReadOnlyProfile::LedgersV1 { company },
        ReadOnlyProfile::LedgerCanaryReadbackV1 {
            company,
            ledger_name: canary_ledger,
            identity_query_sha256,
        },
        ReadOnlyProfile::VouchersV2 { company, range },
        ReadOnlyProfile::VouchersV3 { company, range },
    ]
}

#[test]
fn validated_inputs_reject_arbitrary_or_invalid_values() {
    for company in ["", "  ", "line\nbreak", "\u{0}"] {
        assert_eq!(
            ValidatedCompanyName::new(company),
            Err(ReadProfileValidationError::CompanyInvalid)
        );
    }
    assert_eq!(
        ValidatedCompanyName::new("x".repeat(256)),
        Err(ReadProfileValidationError::CompanyInvalid)
    );
    for (from, to, expected) in [
        (
            "2026-01-01",
            "20260102",
            ReadProfileValidationError::DateInvalid,
        ),
        (
            "20260229",
            "20260301",
            ReadProfileValidationError::DateInvalid,
        ),
        (
            "20260402",
            "20260401",
            ReadProfileValidationError::DateRangeInvalid,
        ),
    ] {
        assert_eq!(ValidatedDateRange::new(from, to), Err(expected));
    }
    assert!(ValidatedDateRange::new("20240229", "20240229").is_ok());
    for ledger_name in [
        "",
        "Cash",
        "BRIDGE-CANARY-",
        "canary\nledger",
        "canary\"ledger",
        "canary:$ledger",
    ] {
        assert_eq!(
            ValidatedCanaryLedgerName::new(ledger_name),
            Err(ReadProfileValidationError::CanaryLedgerInvalid)
        );
    }
    assert_eq!(
        ValidatedCanaryLedgerName::new("x".repeat(129)),
        Err(ReadProfileValidationError::CanaryLedgerInvalid)
    );
    assert!(ValidatedCanaryLedgerName::new("BRIDGE-CANARY-LEDGER-001").is_ok());
    assert_eq!(
        ValidatedIdentityQuerySha256::new("a".repeat(63)),
        Err(ReadProfileValidationError::IdentityQueryInvalid)
    );
    assert_eq!(
        ValidatedIdentityQuerySha256::new("g".repeat(64)),
        Err(ReadProfileValidationError::IdentityQueryInvalid)
    );
}

#[test]
fn closed_profiles_emit_exports_only_and_escape_dynamic_values() {
    let company = ValidatedCompanyName::new("BRIDGE & <SYNTHETIC> \"BOOK\"").unwrap();
    let range = ValidatedDateRange::new("20260401", "20260430").unwrap();
    let canary_ledger = ValidatedCanaryLedgerName::new(TEMPLATE_CANARY_LEDGER).unwrap();
    let identity_query_sha256 =
        ValidatedIdentityQuerySha256::new(TEMPLATE_IDENTITY_QUERY_SHA256).unwrap();
    for profile in profiles(&company, &range, &canary_ledger, &identity_query_sha256) {
        let request = profile.render();
        let upper = request.to_ascii_uppercase();
        assert!(upper.contains("<TALLYREQUEST>EXPORT</TALLYREQUEST>"));
        for forbidden in ["IMPORT", "CREATE", "ALTER", "DELETE"] {
            assert!(!upper.contains(&format!("<TALLYREQUEST>{forbidden}")));
        }
    }
    let ledger = ReadOnlyProfile::LedgersV1 { company: &company }.render();
    assert!(ledger.contains("BRIDGE &amp; &lt;SYNTHETIC&gt; &quot;BOOK&quot;"));
    assert!(!ledger.contains("BRIDGE & <SYNTHETIC>"));

    let canary = ReadOnlyProfile::LedgerCanaryReadbackV1 {
        company: &company,
        ledger_name: &canary_ledger,
        identity_query_sha256: &identity_query_sha256,
    }
    .render();
    assert!(canary.contains(BRIDGE_LEDGER_WRITE_READBACK_SCHEMA));
    assert!(canary.contains("<FILTERS>BRIDGE Ledger Exact Canary Name V1</FILTERS>"));
    assert!(canary.contains(
        "<SYSTEM TYPE=\"Formulae\" NAME=\"BRIDGE Ledger Exact Canary Name V1\">$Name = \"BRIDGE-CANARY-LEDGER-001\"</SYSTEM>"
    ));
    assert!(canary.contains("$Name = \"BRIDGE-CANARY-LEDGER-001\""));
    assert!(canary.contains(
        "<XMLATTR>\"QUERYIDENTITYSETSHA256\" : \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"</XMLATTR>"
    ));

    let injection =
        ValidatedCompanyName::new("X</SVCURRENTCOMPANY><TALLYREQUEST>IMPORT</TALLYREQUEST>")
            .unwrap();
    let escaped = ReadOnlyProfile::LedgersV1 {
        company: &injection,
    }
    .render();
    assert_eq!(escaped.matches("<TALLYREQUEST>").count(), 1);
    assert!(!escaped.contains("<TALLYREQUEST>IMPORT"));
    assert!(escaped.contains("&lt;/SVCURRENTCOMPANY&gt;"));

    let bootstrap = ReadOnlyProfile::StandardLedgerIdentityV1 {
        company: &injection,
    }
    .render();
    assert_eq!(bootstrap.matches("<TALLYREQUEST>").count(), 1);
    assert!(!bootstrap.contains("<TALLYREQUEST>IMPORT"));
    assert!(bootstrap.contains("&lt;/SVCURRENTCOMPANY&gt;"));

    let extent_request = ReadOnlyProfile::CompanyBookExtentV1 {
        company: &injection,
    }
    .render();
    assert_eq!(extent_request.matches("<TALLYREQUEST>").count(), 1);
    assert!(!extent_request.contains("<TALLYREQUEST>IMPORT"));
    assert!(extent_request.contains("&lt;/SVCURRENTCOMPANY&gt;"));

    let extent_v2_request = ReadOnlyProfile::CompanyBookExtentV2 {
        company: &injection,
    }
    .render();
    assert_eq!(extent_v2_request.matches("<TALLYREQUEST>").count(), 1);
    assert!(!extent_v2_request.contains("<TALLYREQUEST>IMPORT"));
    assert!(extent_v2_request.contains("&lt;/SVCURRENTCOMPANY&gt;"));

    #[cfg(feature = "voucher-scan")]
    {
        let pinned = PinnedCompany::verified(injection.clone(), "synthetic-guid".to_string())
            .expect("verified test identity");
        let window = crate::outstandings::DateWindow::parse(
            crate::outstandings::DateBoundaryProfile::EducationRestricted,
            "20260401",
            "20260402",
        )
        .unwrap()
        .narrow_partitions()
        .unwrap()
        .remove(0);
        for request in [
            ReadOnlyProfile::VoucherOutstandingsV1 {
                company: &pinned,
                window: &window,
                alter_id_range: AlterIdRange::new(0, 1).unwrap(),
            }
            .render(),
            ReadOnlyProfile::VoucherEmptyPartitionWitnessV1 {
                company: &pinned,
                window: &window,
            }
            .render(),
        ] {
            assert_eq!(request.matches("<TALLYREQUEST>").count(), 1);
            assert!(!request.contains("<TALLYREQUEST>IMPORT"));
            assert!(request.contains("&lt;/SVCURRENTCOMPANY&gt;"));
        }
        let witness = ReadOnlyProfile::VoucherEmptyPartitionWitnessV1 {
            company: &pinned,
            window: &window,
        };
        assert_eq!(
            witness.id(),
            ReadOnlyProfileId::VoucherEmptyPartitionWitnessV1
        );
        assert!(witness.template_sha256().len() == 64);
    }
}

/// `CompanyListV2` is a native Tally `Company` collection, not the
/// embedded-TDL custom report `CompanyListV1` builds: no `REPORT`/`FORM`/
/// `PART`/`LINE`/`FIELD` stack and no computed TDL function call (a
/// report stack, and no dynamic `$Field:Argument` invocation). Its three
/// fixed `$$LicenseInfo` arguments contain no spaces. It also carries no
/// `SVCURRENTCOMPANY`, so discovery is never scoped to one company -- the
/// request must be able to return every company Tally has loaded.
#[test]
fn company_list_v2_is_a_native_collection_scoped_to_no_single_company() {
    let request = ReadOnlyProfile::CompanyListV2.render();
    assert!(request.contains("<TYPE>Collection</TYPE>"));
    assert!(request.contains("<TYPE>Company</TYPE>"));
    for field in ["NAME", "GUID", "PRODUCTNAME"] {
        assert!(request.contains(&format!("<NATIVEMETHOD>{field}</NATIVEMETHOD>")));
    }
    for function in ["IsEducationalMode", "IsSilver", "IsGold"] {
        assert!(request.contains(&format!("$$LicenseInfo:{function}")));
    }
    for report_stack_tag in ["<REPORT", "<FORM ", "<PART ", "<LINE ", "<FIELD "] {
        assert!(
            !request.contains(report_stack_tag),
            "unexpected report stack tag {report_stack_tag}"
        );
    }
    assert!(!request.contains("<SVCURRENTCOMPANY"));
}

#[test]
fn profile_ids_and_template_hashes_are_stable() {
    #[cfg_attr(not(feature = "voucher-scan"), allow(unused_mut))]
    let mut expected: Vec<(ReadOnlyProfileId, &str)> = vec![
        (
            ReadOnlyProfileId::CompanyListV1,
            "8905c041a75929f157be704ca9dd076a37995dc7936e0ad71c9361406a8fb694",
        ),
        (
            ReadOnlyProfileId::CompanyListV2,
            "9df2a53f085dac2636e9435462b612c1487ec6f903677815036c9f39163f7dd8",
        ),
        (
            ReadOnlyProfileId::CompanyBookExtentV1,
            "f46420fd96ee567069d1bf70c7895e76c75482b5370fc52e97aea64b8950d3a5",
        ),
        (
            ReadOnlyProfileId::CompanyBookExtentV2,
            "28ffc66bfaee2172ac5de37ab358951ae2439ec2f2fc6b5aeb220624d8d87fdc",
        ),
        (
            ReadOnlyProfileId::StandardLedgerIdentityV1,
            "3f38d58a88c8bb99180290b2fb17a3057d6149f78d81e08ba3d9bfc2d595dd95",
        ),
        (
            ReadOnlyProfileId::StandardLedgerCatalogV1,
            "3f38d58a88c8bb99180290b2fb17a3057d6149f78d81e08ba3d9bfc2d595dd95",
        ),
        (
            ReadOnlyProfileId::LedgersV1,
            "a4a29d043d8f0c11c5f358043cb510554e8307f93a5c45676d2f073ad68f87fd",
        ),
        (
            ReadOnlyProfileId::LedgerCanaryReadbackV1,
            "6659ce0840da754a7cc3bf5272aa2b13c4b1ec2e9f9099555835276d3a478b76",
        ),
        (
            ReadOnlyProfileId::VouchersV2,
            "81cc3da69ab58cd857603342898a2b3142ade321d5fea38de76c425612ccf6df",
        ),
        (
            ReadOnlyProfileId::VouchersV3,
            "8dbe02d0645ff6b055ea8f6fb63d27af90dc41e81ee42b39ee7d2b407ab4aa3b",
        ),
    ];
    #[cfg(feature = "voucher-scan")]
    expected.extend([
        (
            ReadOnlyProfileId::LedgerOpeningCoverageV1,
            "ae607d1d0ae4347f30e03ae3f7ee6c9a48b936be1cf71bb63ea1966727871efb",
        ),
        (
            ReadOnlyProfileId::VoucherOutstandingsV1,
            "165b40352eda40491a189d58ac81777b307bb709c65afb1f3555f19f0df0edd9",
        ),
        (
            ReadOnlyProfileId::VoucherEmptyPartitionWitnessV1,
            "7e9d3befbb8ef32503e0fe960c78aedcbf8a8075c056a8de1ad40846e6d109bc",
        ),
    ]);
    let observed = expected
        .iter()
        .map(|(profile, _)| (profile.as_str(), profile.template_sha256()))
        .collect::<Vec<_>>();
    let sealed = expected
        .iter()
        .map(|(profile, digest)| (profile.as_str(), (*digest).to_string()))
        .collect::<Vec<_>>();
    assert!(observed.iter().all(|(_, digest)| digest.len() == 64));
    assert_eq!(observed, sealed);
}

#[test]
fn compatibility_renderers_preserve_validated_profile_bytes() {
    let company = ValidatedCompanyName::new("BRIDGE SYNTHETIC BOOK").unwrap();
    let range = ValidatedDateRange::new("20260401", "20260430").unwrap();
    assert_eq!(
        compatibility::company_list_request(),
        ReadOnlyProfile::CompanyListV1.render()
    );
    assert_eq!(
        compatibility::standard_ledger_identity_request(company.as_str()),
        ReadOnlyProfile::StandardLedgerIdentityV1 { company: &company }.render()
    );
    assert_eq!(
        compatibility::standard_ledger_catalog_request(company.as_str()),
        ReadOnlyProfile::StandardLedgerCatalogV1 { company: &company }.render()
    );
    assert_eq!(
        compatibility::ledgers_request(company.as_str()),
        ReadOnlyProfile::LedgersV1 { company: &company }.render()
    );
    assert_eq!(
        compatibility::vouchers_request(
            company.as_str(),
            range.from_yyyymmdd(),
            range.to_yyyymmdd(),
        ),
        ReadOnlyProfile::VouchersV2 {
            company: &company,
            range: &range,
        }
        .render()
    );
    assert_eq!(
        compatibility::selected_vouchers_request(
            company.as_str(),
            range.from_yyyymmdd(),
            range.to_yyyymmdd(),
        ),
        ReadOnlyProfile::VouchersV3 {
            company: &company,
            range: &range,
        }
        .render()
    );
}
