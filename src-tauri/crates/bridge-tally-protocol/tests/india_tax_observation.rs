#![cfg(feature = "india-tax-observation-parser")]

use bridge_tally_protocol::india_tax_observation::{
    parse_unbound_india_tax_observation, IndiaTaxCountAuthority, IndiaTaxObservationBinding,
    IndiaTaxObservationError, IndiaTaxObservationLimits, ObservedTaxOwnerKind,
};

const COMPANY: &str = "bridge-synthetic-company-guid";
const GSTIN: &str = "27ABCDE1234F1Z5";

fn envelope(
    registrations: &str,
    voucher_taxes: &str,
    registration_count: u64,
    tax_count: u64,
) -> String {
    format!(
        r#"<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><INDIATAXCONTEXT SCHEMA="bridge.tally.india-tax-observed-raw/1" PROFILE="bridge.india-tax-observed-raw-xml/1" OBJECTTYPE="INDIATAXOBSERVATION" COMPANYGUID="{COMPANY}" FROMDATE="20260401" TODATE="20260430" TAXREGISTRATIONCOUNT="{registration_count}" VOUCHERTAXCOUNT="{tax_count}"/>{registrations}{voucher_taxes}</BODY></ENVELOPE>"#,
    )
}

fn company_registration() -> String {
    format!(
        r#"<TAXREGISTRATION OWNERKIND="COMPANY" OWNERGUID="{COMPANY}" OWNERALTERID="7"><REGISTRATIONTYPE>Regular</REGISTRATIONTYPE><GSTIN>{GSTIN}</GSTIN></TAXREGISTRATION>"#,
    )
}

fn ledger_registration() -> String {
    r#"<TAXREGISTRATION OWNERKIND="LEDGER" OWNERREMOTEID="bridge-synthetic-ledger-1" OWNERMASTERID="11"><REGISTRATIONTYPE>Composition</REGISTRATIONTYPE><GSTIN>29ABCDE1234F1Z5</GSTIN></TAXREGISTRATION>"#.to_owned()
}

fn voucher_tax(ordinal: u64, component: &str, amount: &str) -> String {
    format!(
        r#"<VOUCHERTAX VOUCHERGUID="bridge-synthetic-voucher-1" VOUCHERALTERID="9" TAXROWORDINAL="{ordinal}"><PLACEOFSUPPLY>27-Maharashtra</PLACEOFSUPPLY><ASSESSABLEVALUE>1000.00</ASSESSABLEVALUE><TAXCOMPONENT>{component}</TAXCOMPONENT><TAXRATE>9.00</TAXRATE><TAXAMOUNT>{amount}</TAXAMOUNT></VOUCHERTAX>"#,
    )
}

#[test]
fn parses_exact_unbound_observations_without_promoting_authority() {
    let xml = envelope(
        &(company_registration() + &ledger_registration()),
        &(voucher_tax(1, "CGST", "90.00") + &voucher_tax(2, "SGST", "90.00")),
        2,
        2,
    );
    let parsed = parse_unbound_india_tax_observation(xml.as_bytes(), Default::default())
        .expect("parse exact synthetic observation");

    assert_eq!(parsed.tax_registrations().len(), 2);
    assert_eq!(parsed.voucher_taxes().len(), 2);
    assert_eq!(
        parsed.tax_registrations()[0].owner_kind(),
        ObservedTaxOwnerKind::Company
    );
    assert_eq!(parsed.tax_registrations()[0].gstin(), GSTIN);
    assert_eq!(parsed.voucher_taxes()[1].tax_component(), "SGST");
    assert_eq!(parsed.voucher_taxes()[1].tax_amount(), "90.00");
    assert_ne!(
        parsed.voucher_taxes()[0].raw_fragment_sha256(),
        parsed.voucher_taxes()[1].raw_fragment_sha256()
    );
    assert_eq!(
        parsed.evidence().binding(),
        IndiaTaxObservationBinding::UnboundNoRequestArtifact
    );
    assert_eq!(
        parsed.evidence().count_authority(),
        IndiaTaxCountAuthority::ResponseInternalOnly
    );
    assert!(!parsed.canonicalization_eligible());
}

#[test]
fn zero_rows_is_only_an_internally_consistent_unbound_response() {
    let parsed =
        parse_unbound_india_tax_observation(envelope("", "", 0, 0).as_bytes(), Default::default())
            .expect("parse zero-row response");
    assert!(parsed.tax_registrations().is_empty());
    assert!(parsed.voucher_taxes().is_empty());
    assert_eq!(parsed.evidence().claimed_registration_count(), 0);
    assert!(!parsed.canonicalization_eligible());
}

#[test]
fn rejects_count_mismatch_and_rows_out_of_order() {
    let mismatch = envelope(&company_registration(), "", 0, 0);
    assert_eq!(
        parse_unbound_india_tax_observation(mismatch, Default::default()).unwrap_err(),
        IndiaTaxObservationError::CountMismatch
    );

    let wrong_order = envelope(
        &company_registration(),
        &voucher_tax(1, "IGST", "180.00"),
        1,
        1,
    )
    .replace(
        &(company_registration() + &voucher_tax(1, "IGST", "180.00")),
        &(voucher_tax(1, "IGST", "180.00") + &company_registration()),
    );
    assert_eq!(
        parse_unbound_india_tax_observation(wrong_order, Default::default()).unwrap_err(),
        IndiaTaxObservationError::WrongGrammar
    );
}

#[test]
fn rejects_nested_unknown_and_duplicate_case_variant_attributes() {
    let nested = envelope(
        &format!("<WRAPPER>{}</WRAPPER>", company_registration()),
        "",
        1,
        0,
    );
    assert_eq!(
        parse_unbound_india_tax_observation(nested, Default::default()).unwrap_err(),
        IndiaTaxObservationError::WrongGrammar
    );

    let duplicate = envelope(&company_registration(), "", 1, 0).replace(
        "OWNERKIND=\"COMPANY\"",
        "OWNERKIND=\"COMPANY\" ownerkind=\"COMPANY\"",
    );
    assert_eq!(
        parse_unbound_india_tax_observation(duplicate, Default::default()).unwrap_err(),
        IndiaTaxObservationError::DuplicateField
    );

    let unknown = envelope(&company_registration(), "", 1, 0).replace(
        " OWNERALTERID=\"7\"",
        " OWNERALTERID=\"7\" SECRET=\"sentinel\"",
    );
    assert_eq!(
        parse_unbound_india_tax_observation(unknown, Default::default()).unwrap_err(),
        IndiaTaxObservationError::WrongGrammar
    );
}

#[test]
fn rejects_ambiguous_company_owner_and_missing_voucher_identity() {
    let ambiguous = envelope(
        &company_registration().replace(
            " OWNERALTERID=\"7\"",
            " OWNERREMOTEID=\"not-company-authority\" OWNERALTERID=\"7\"",
        ),
        "",
        1,
        0,
    );
    assert_eq!(
        parse_unbound_india_tax_observation(ambiguous, Default::default()).unwrap_err(),
        IndiaTaxObservationError::MissingIdentity
    );

    let missing = envelope(
        "",
        &voucher_tax(1, "IGST", "180.00")
            .replace(" VOUCHERGUID=\"bridge-synthetic-voucher-1\"", ""),
        0,
        1,
    );
    assert_eq!(
        parse_unbound_india_tax_observation(missing, Default::default()).unwrap_err(),
        IndiaTaxObservationError::MissingIdentity
    );
}

#[test]
fn rejects_duplicate_observation_keys_and_invalid_numeric_lexemes() {
    let duplicated = company_registration() + &company_registration();
    assert_eq!(
        parse_unbound_india_tax_observation(envelope(&duplicated, "", 2, 0), Default::default())
            .unwrap_err(),
        IndiaTaxObservationError::DuplicateObservation
    );

    for invalid in ["1e2", "1,000.00", "NaN", "+10.00"] {
        let xml = envelope("", &voucher_tax(1, "IGST", invalid), 0, 1);
        assert_eq!(
            parse_unbound_india_tax_observation(xml, Default::default()).unwrap_err(),
            IndiaTaxObservationError::InvalidValue
        );
    }
    let negative_rate = envelope(
        "",
        &voucher_tax(1, "IGST", "180.00").replace("<TAXRATE>9.00", "<TAXRATE>-9.00"),
        0,
        1,
    );
    assert_eq!(
        parse_unbound_india_tax_observation(negative_rate, Default::default()).unwrap_err(),
        IndiaTaxObservationError::InvalidValue
    );
}

#[test]
fn enforces_response_field_and_record_limits_without_partial_results() {
    let xml = envelope(&company_registration(), "", 1, 0);
    let mut limits = IndiaTaxObservationLimits {
        max_encoded_bytes: xml.len() - 1,
        ..Default::default()
    };
    assert_eq!(
        parse_unbound_india_tax_observation(&xml, limits).unwrap_err(),
        IndiaTaxObservationError::ResponseTooLarge
    );

    limits = IndiaTaxObservationLimits {
        max_field_bytes: 8,
        ..Default::default()
    };
    assert_eq!(
        parse_unbound_india_tax_observation(&xml, limits).unwrap_err(),
        IndiaTaxObservationError::ResourceLimitExceeded
    );

    limits = IndiaTaxObservationLimits {
        max_records: 0,
        ..Default::default()
    };
    assert_eq!(
        parse_unbound_india_tax_observation(&xml, limits).unwrap_err(),
        IndiaTaxObservationError::ResourceLimitExceeded
    );

    assert!(
        parse_unbound_india_tax_observation(&xml[..xml.len() - 12], Default::default()).is_err()
    );
}

#[test]
fn debug_display_and_errors_do_not_echo_sensitive_sentinels() {
    let company_sentinel = "company-secret-guid";
    let identity_sentinel = "voucher-secret-identity";
    let gstin_sentinel = "27ABCDE1234F1Z5";
    let amount_sentinel = "987654321.99";
    let xml = envelope(
        &company_registration(),
        &voucher_tax(1, "IGST", amount_sentinel),
        1,
        1,
    )
    .replace(COMPANY, company_sentinel)
    .replace("bridge-synthetic-voucher-1", identity_sentinel);
    let parsed =
        parse_unbound_india_tax_observation(xml, Default::default()).expect("parse sentinels");
    let debug = format!("{parsed:?}");
    for sentinel in [
        company_sentinel,
        identity_sentinel,
        gstin_sentinel,
        amount_sentinel,
    ] {
        assert!(!debug.contains(sentinel));
    }

    let malformed = format!("<ENVELOPE><SECRET>{identity_sentinel}</SECRET>");
    let error = parse_unbound_india_tax_observation(malformed, Default::default()).unwrap_err();
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains(identity_sentinel));
}
