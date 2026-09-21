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

/// Each end of the window is checked on its own (bridge#581): Education honours
/// a boundary only on the 1st, 2nd or 31st, and a date it cannot read is not
/// one Bridge has admitted. A licensed endpoint accepts every day.
#[test]
fn an_education_window_is_admitted_only_when_both_ends_are_honoured_days() {
    use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile::{
        EducationRestricted, ModeAgnostic,
    };
    let window = |from: &str, to: &str| {
        AgentReadRequest::parse(format!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST>\
             <TYPE>Collection</TYPE><ID>Synthetic</ID></HEADER><BODY><DESC><STATICVARIABLES>\
             {from}{to}</STATICVARIABLES></DESC></BODY></ENVELOPE>"
        ))
        .unwrap()
    };
    let from = |date: &str| format!("<SVFROMDATE TYPE=\"Date\">{date}</SVFROMDATE>");
    let to = |date: &str| format!("<SVTODATE TYPE=\"Date\">{date}</SVTODATE>");
    for (request, education) in [
        (window(&from("20260401"), &to("20260430")), false),
        (window(&from("20260401"), &to("20260501")), true),
        (window(&from("20260402"), &to("20260531")), true),
        (window(&from("20260403"), &to("20260501")), false),
        (window(&from("20260405"), &to("20260405")), false),
        (window(&from("not-a-date"), &to("20260501")), false),
        (window("<SVFROMDATE/>", &to("20260501")), false),
        (window("", ""), true),
    ] {
        let xml = request.clone().into_xml();
        assert_eq!(
            request.window_accepted_by(EducationRestricted),
            education,
            "{xml}"
        );
        assert!(request.window_accepted_by(ModeAgnostic), "{xml}");
    }
}
