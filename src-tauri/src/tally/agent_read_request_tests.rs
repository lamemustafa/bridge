use super::*;

#[test]
fn only_one_complete_export_collection_header_is_admitted() {
    let request = bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::CompanyListV2.render();
    assert_eq!(
        AgentReadRequest::parse(request.clone()).unwrap().into_xml(),
        request
    );
    let catalogue =
        bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::StandardLedgerCatalogV1 {
            company: &bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
                "Synthetic Book",
            )
            .unwrap(),
        }
        .render();
    assert!(AgentReadRequest::parse(catalogue).is_ok());
    for denied in [
        request.replace(
            "<TALLYREQUEST>Export</TALLYREQUEST>",
            "<TALLYREQUEST> Import Data </TALLYREQUEST>",
        ),
        request.replace(
            "<TALLYREQUEST>Export</TALLYREQUEST>",
            "<TALLYREQUEST>Execute</TALLYREQUEST>",
        ),
        request.replace("<TYPE>Collection</TYPE>", "<TYPE>Function</TYPE>"),
        request.replace("</HEADER>", "<TALLYREQUEST>Export</TALLYREQUEST></HEADER>"),
        request.replace("</ENVELOPE>", ""),
        format!("{request}{request}"),
        format!("<!DOCTYPE ENVELOPE>{request}"),
    ] {
        assert_eq!(
            AgentReadRequest::parse(denied).unwrap_err(),
            AgentReadRequestError
        );
    }
}

#[test]
fn the_company_object_constructor_renders_the_pinned_profile_for_the_verified_name() {
    let identity = crate::tally::VerifiedCompanyIdentity::test_fixture(
        "BRIDGE & <SYNTHETIC> \"BOOK\"",
        "00000000-0000-4000-8000-000000000001",
    );
    let company = bridge_tally_protocol::xml_read_profiles::ValidatedCompanyName::new(
        "BRIDGE & <SYNTHETIC> \"BOOK\"",
    )
    .unwrap();
    let expected =
        bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::AuditCompanyObjectV1 {
            company: &company,
        };
    assert_eq!(expected.id().as_str(), "audit_company_object_v1");
    assert_eq!(
        AgentReadRequest::company_object(&identity)
            .unwrap()
            .into_xml(),
        expected.render()
    );
    let invalid = crate::tally::VerifiedCompanyIdentity::test_fixture(
        "line\nbreak",
        "00000000-0000-4000-8000-000000000001",
    );
    assert_eq!(
        AgentReadRequest::company_object(&invalid).unwrap_err(),
        AgentReadRequestError
    );
}

#[test]
fn parse_refuses_every_object_export_including_the_company_part() {
    let identity = crate::tally::VerifiedCompanyIdentity::test_fixture(
        "BRIDGE SYNTHETIC BOOK",
        "00000000-0000-4000-8000-000000000001",
    );
    let company_object = AgentReadRequest::company_object(&identity)
        .unwrap()
        .into_xml();
    assert!(company_object.contains("<TYPE>Object</TYPE>"));
    let collection =
        bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::CompanyListV2.render();
    for denied in [
        company_object.clone(),
        company_object.replace("<TYPE>Object</TYPE>", "<TYPE>object</TYPE>"),
        company_object.replace("<TYPE>Object</TYPE>", "<TYPE> Object </TYPE>"),
        company_object.replace("<SUBTYPE>Company</SUBTYPE>", "<SUBTYPE>Ledger</SUBTYPE>"),
        company_object.replace("<SUBTYPE>Company</SUBTYPE>", ""),
        company_object.replace(
            "<FETCH>GUID</FETCH>",
            "<FETCH>GUID</FETCH><FETCH>ORIGINALNAME</FETCH>",
        ),
        company_object.replace("<FETCHLIST>", "<FETCHLIST><FETCH>*</FETCH>"),
        // A Collection and an Object in one header: a second TYPE.
        company_object.replace(
            "<TYPE>Object</TYPE>",
            "<TYPE>Collection</TYPE><TYPE>Object</TYPE>",
        ),
        company_object.replace(
            "<TYPE>Object</TYPE>",
            "<TYPE>Object</TYPE><TYPE>Collection</TYPE>",
        ),
        collection.replace("<TYPE>Collection</TYPE>", "<TYPE>Object</TYPE>"),
        collection.replace("<TYPE>Collection</TYPE>", "<TYPE>OBJECT</TYPE>"),
        collection.replace("<TYPE>Collection</TYPE>", "<TYPE>\n\tObject\n</TYPE>"),
    ] {
        assert_eq!(
            AgentReadRequest::parse(denied.clone()).unwrap_err(),
            AgentReadRequestError,
            "{denied}"
        );
    }
}
