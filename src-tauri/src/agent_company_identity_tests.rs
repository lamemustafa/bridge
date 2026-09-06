//! Native company identity admission with captured-source fault injection.
use super::*;
use tally_protocol_simulator::{
    encode, Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

fn server(address: std::net::SocketAddr, path: &std::path::Path) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: address.ip().to_string(),
            port: address.port(),
        },
        data_dir: path.into(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: true,
    })
}

#[tokio::test]
async fn company_identity_requires_native_uuid_and_retains_observed_failure_evidence() {
    let bytes = include_bytes!("../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml");
    let captured = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    for observed_guid in [
        "malformed-guid".to_string(),
        GUID.to_uppercase(),
        GUID.to_string(),
    ] {
        let xml = captured.replace(GUID, &observed_guid);
        let companies = bridge_tally_protocol::parse_companies_from_collection(&xml).unwrap();
        let selected = companies
            .iter()
            .find(|company| company.guid.as_deref() == Some(observed_guid.as_str()))
            .unwrap();
        let invalid = observed_guid == "malformed-guid";
        assert_eq!(
            company_json(selected, &companies)["identity_state"],
            if invalid {
                "invalid_guid"
            } else {
                "verified_tuple"
            }
        );
        let plan =
            ScenarioPlan::new(Fixture::SyntheticXml(xml)).with_encoding(WireEncoding::Utf16Le);
        let response_bytes = encode(&plan.fixture.body(), plan.encoding);
        let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
        let simulator =
            SequenceSimulator::spawn(vec![plan.clone(), status.clone(), plan, status]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = server(simulator.address(), directory.path());
        let result = server.verified_company(&GUID.to_uppercase()).await;
        let evidence = if invalid {
            let error = result.unwrap_err();
            assert_eq!(error.code, "company_guid_invalid");
            *error.evidence.unwrap()
        } else {
            let (_, identity, evidence) = result.unwrap();
            assert_eq!(identity.company_guid(), GUID);
            assert_eq!(
                crate::agent::egress::canonical_company_guid(identity.company_guid()).as_deref(),
                Some(GUID)
            );
            evidence
        };
        let observed = simulator.finish().unwrap();
        assert_eq!(observed.len(), 4);
        assert_eq!(evidence.request_sha256, observed[0].request_body_sha256);
        assert_eq!(evidence.response_sha256, sha256_hex(&response_bytes));
        assert_eq!(evidence.bytes, response_bytes.len() * 2);
    }
}

#[tokio::test]
async fn malformed_or_non_native_guid_selectors_refuse_before_network() {
    let directory = tempfile::tempdir().unwrap();
    let server = server("127.0.0.1:9".parse().unwrap(), directory.path());
    for guid in [
        "malformed-guid".to_string(),
        GUID.replace('-', ""),
        format!("{{{GUID}}}"),
        format!("urn:uuid:{GUID}"),
    ] {
        for (tool, args) in [
            ("ledger_masters", json!({"company_guid":guid})),
            (
                "verify_import",
                json!({"company_guid":guid,"batch_id":"unread-batch"}),
            ),
            (
                "build_import_xml",
                json!({"company_guid":guid,"vouchers":[{
                    "bridge_txn_id":"uuid-admission", "date":"2026-09-01", "voucher_type":"Journal",
                    "entries":[{"ledger":"Cash","amount":"1.00","side":"Dr"},
                    {"ledger":"Sales","amount":"1.00","side":"Cr"}]
                }]}),
            ),
        ] {
            let response = server.call_tool(tool, args).await;
            assert_eq!(response["isError"], true, "{tool}");
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"], "company_guid_invalid",
                "{tool}"
            );
            assert_eq!(
                response["structuredContent"]["evidence"]["bytes"], 0,
                "{tool}"
            );
        }
    }
}
