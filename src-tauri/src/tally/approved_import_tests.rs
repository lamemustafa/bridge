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
