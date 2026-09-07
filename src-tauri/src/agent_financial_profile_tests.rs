//! MCP refusals replay captured sources; only profile observations are changed.
use super::*;
use tally_protocol_simulator::{
    Fixture, ProductStatus, ScenarioPlan, SequenceSimulator, WireEncoding,
};

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

fn captured(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn xml(body: String) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(body)).with_encoding(WireEncoding::Utf16Le)
}

fn status() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
}

fn pair(plans: &mut Vec<ScenarioPlan>, source: ScenarioPlan) {
    plans.extend([source.clone(), status(), source, status()]);
}

fn join(left: &str, right: &str) -> String {
    sha256_hex(format!("{left}:{right}").as_bytes())
}

#[tokio::test]
async fn monetary_tools_refuse_unobserved_mode_or_unsupported_product_with_completed_wire_evidence() {
    let companies = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ));
    // Release and licence tier are retained as observations, rather than
    // admissions. Keep this negative coverage on conditions the read boundary
    // cannot admit: an unsupported product, or no observed licence mode.
    // These mutate captured metadata only; they are not positive evidence for
    // another product, Gold, or Education behavior.
    let unsupported_product = companies.replace(
        "<PRODUCTNAME TYPE=\"String\">TallyPrime</PRODUCTNAME>",
        "<PRODUCTNAME TYPE=\"String\">UnsupportedTallyProduct</PRODUCTNAME>",
    );
    assert_ne!(unsupported_product, companies);
    let extents = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml"
    ));
    let currency = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
    ));
    let unobserved_mode = companies.replace(
        "<SILVER TYPE=\"Logical\">Yes</SILVER>",
        "<SILVER TYPE=\"Logical\">No</SILVER>",
    );
    assert_ne!(unobserved_mode, companies);
    for unqualified in [unsupported_product, unobserved_mode] {
        for (tool, args, reads_currency) in [
            (
                "ledger_masters",
                json!({"company_guid":GUID,"fields":"basic"}),
                false,
            ),
            (
                "ledger_masters",
                json!({"company_guid":GUID,"fields":"compliance"}),
                true,
            ),
            (
                "outstandings",
                json!({"company_guid":GUID,"as_of":"20260801"}),
                true,
            ),
            (
                "ledger_movement",
                json!({"company_guid":GUID,"from":"20260801","to":"20260802"}),
                false,
            ),
        ] {
            let mut plans = Vec::new();
            pair(&mut plans, xml(companies.clone()));
            let currency_index = if reads_currency {
                plans.push(xml(companies.clone()));
                pair(&mut plans, xml(extents.clone()));
                let index = plans.len();
                pair(&mut plans, xml(currency.clone()));
                pair(&mut plans, xml(extents.clone()));
                plans.push(xml(companies.clone()));
                Some(index)
            } else {
                None
            };
            let profile_index = plans.len();
            plans.extend([status(), xml(unqualified.clone())]);
            let responses = plans
                .iter()
                .map(ScenarioPlan::response_bytes)
                .collect::<Vec<_>>();
            let simulator = SequenceSimulator::spawn(plans).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let server = Server::new(Settings {
                endpoint: TallyEndpointConfig {
                    host: "127.0.0.1".into(),
                    port: simulator.address().port(),
                },
                data_dir: directory.path().into(),
                max_rows: 500,
                max_bytes: 200_000,
                redaction: Redaction::None,
                import_enabled: false,
            });
            let response = server.call_tool(tool, args.clone()).await;
            assert_eq!(response["isError"], true, "{tool} {args}");
            let content = &response["structuredContent"];
            assert_eq!(
                content["result"]["error"]["code"], "financial_read_profile_unqualified",
                "{tool} {args}"
            );
            assert_eq!(content["result"].as_object().unwrap().len(), 1);
            assert_eq!(content["evidence"]["state"], "partial");
            let text: Value =
                serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(&text, content);
            let requests = simulator.finish().unwrap();
            // No balance, party-master, bills or voucher export may follow this probe.
            assert_eq!(requests.len(), profile_index + 2, "{tool} {args}");
            let company_request = &requests[0].request_body_sha256;
            let company_response = sha256_hex(&responses[0]);
            let profile_request = join(
                &requests[profile_index].request_body_sha256,
                &requests[profile_index + 1].request_body_sha256,
            );
            let profile_response = join(
                &sha256_hex(&responses[profile_index]),
                &sha256_hex(&responses[profile_index + 1]),
            );
            let mut bytes = responses[0].len() * 2
                + responses[profile_index].len()
                + responses[profile_index + 1].len();
            let (expected_request, expected_response) = if let Some(index) = currency_index {
                bytes += responses[index].len() * 2;
                let currency_request = &requests[index].request_body_sha256;
                let currency_response = sha256_hex(&responses[index]);
                if tool == "ledger_masters" {
                    (
                        join(company_request, &join(currency_request, &profile_request)),
                        join(
                            &company_response,
                            &join(&currency_response, &profile_response),
                        ),
                    )
                } else {
                    (
                        join(&join(company_request, currency_request), &profile_request),
                        join(
                            &join(&company_response, &currency_response),
                            &profile_response,
                        ),
                    )
                }
            } else {
                (
                    join(company_request, &profile_request),
                    join(&company_response, &profile_response),
                )
            };
            assert_eq!(
                content["evidence"]["request_sha256"], expected_request,
                "{tool} {args}"
            );
            assert_eq!(
                content["evidence"]["response_sha256"], expected_response,
                "{tool} {args}"
            );
            assert_eq!(content["evidence"]["bytes"], bytes, "{tool} {args}");
        }
    }
}
