use super::*;
#[test]
fn import_name_scope_must_select_one_observed_company() {
    // Local identity-admission test. These are not claimed Tally responses.
    let original = bridge_tally_protocol::TallyCompany {
        name: "Synthetic Book".into(),
        guid: Some("00000000-0000-4000-8000-000000000001".into()),
        company_number: Some("1".into()),
        books_from: Some("20260401".into()),
    };
    let identity = super::super::VerifiedCompanyIdentity::from_observed_companies(
        original.name.clone(),
        original.guid.clone().unwrap(),
        "1".into(),
        "20260401".into(),
        std::slice::from_ref(&original),
    )
    .unwrap();
    assert!(require_unique_company_scope(std::slice::from_ref(&original), &identity).is_ok());
    for name in ["Synthetic Book", " synthetic book "] {
        let mut other = original.clone();
        other.name = name.into();
        other.guid = Some("00000000-0000-4000-8000-000000000002".into());
        assert_eq!(
            require_unique_company_scope(&[original.clone(), other], &identity),
            Err(AmbiguousImportCompany)
        );
    }
}

/// The queue's Education recheck covers every voucher's date, not only the
/// first, and an empty list approves nothing.
#[test]
fn every_voucher_date_must_pass_the_education_boundary() {
    let date = |value: &str| TallyDate::parse(value.to_string()).unwrap();
    let education = DateBoundaryProfile::EducationRestricted;
    assert!(every_date_accepted(education, &[date("20250401")]));
    assert!(every_date_accepted(
        education,
        &[date("20250401"), date("20250502"), date("20250531")]
    ));
    // The second voucher's date fails, so the batch fails.
    assert!(!every_date_accepted(
        education,
        &[date("20250401"), date("20250415")]
    ));
    assert!(!every_date_accepted(
        education,
        &[date("20250415"), date("20250401")]
    ));
    assert!(every_date_accepted(
        DateBoundaryProfile::ModeAgnostic,
        &[date("20250401"), date("20250415")]
    ));
    assert!(!every_date_accepted(DateBoundaryProfile::ModeAgnostic, &[]));
}
