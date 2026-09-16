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
