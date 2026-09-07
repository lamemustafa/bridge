//! Replay observed source shapes, then inject a concurrent voucher edit.
use super::*;
use tally_protocol_simulator::{
    Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

fn captured(bytes: &[u8]) -> ScenarioPlan {
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    ScenarioPlan::new(Fixture::SyntheticXml(String::from_utf16(&words).unwrap()))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}

#[tokio::test]
async fn movement_refuses_voucher_changes_even_when_period_openings_match() {
    let company = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ));
    let extent = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml"
    ));
    let ledger = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-period-opening.utf16le.xml"
    ));
    let voucher = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    ));
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
    let opening = || {
        vec![
            status.clone(),
            company.clone(),
            company.clone(),
            extent.clone(),
            status.clone(),
            extent.clone(),
            status.clone(),
            ledger.clone(),
            status.clone(),
            ledger.clone(),
            status.clone(),
            extent.clone(),
            status.clone(),
            extent.clone(),
            status.clone(),
            company.clone(),
            status.clone(),
            company.clone(),
        ]
    };
    let voucher_read = |body: ScenarioPlan| {
        vec![
            company.clone(),
            body.clone(),
            status.clone(),
            body,
            status.clone(),
            company.clone(),
        ]
    };
    let original = voucher.fixture.body();
    let start = original.find("<VOUCHER ").unwrap();
    let end = start + original[start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
    let mut reduced = original.to_string();
    reduced.replace_range(start..end, "");
    for change in ["stable", "edit", "posting", "deletion", "incomplete"] {
        let changed = change != "stable";
        let mut before = voucher.clone();
        let mut after = voucher.clone();
        // Inject concurrent changes into captured rows in memory. The source
        // captures stay byte-exact; no live concurrency experiment is claimed.
        match change {
            "edit" => {
                let altered = original.replace("-101.01", "-201.01").replace(
                    "<AMOUNT TYPE=\"Amount\">101.01</AMOUNT>",
                    "<AMOUNT TYPE=\"Amount\">201.01</AMOUNT>",
                );
                assert_ne!(altered, original);
                after.fixture = Fixture::SyntheticXml(altered);
            }
            "posting" => before.fixture = Fixture::SyntheticXml(reduced.clone()),
            "deletion" => after.fixture = Fixture::SyntheticXml(reduced.clone()),
            "incomplete" => {
                // Omit one balancing side in both paired responses. Keep the
                // selected WR2 Sales entry, so selection cannot hide refusal.
                let mut omitted = original.to_string();
                let start = omitted.find("<ALLLEDGERENTRIES.LIST>").unwrap();
                let end = start
                    + omitted[start..].find("</ALLLEDGERENTRIES.LIST>").unwrap()
                    + "</ALLLEDGERENTRIES.LIST>".len();
                omitted.replace_range(start..end, "");
                before.fixture = Fixture::SyntheticXml(omitted);
            }
            _ => {}
        }
        let mut plans = vec![
            company.clone(),
            status.clone(),
            company.clone(),
            status.clone(),
        ];
        plans.extend(opening());
        plans.extend(voucher_read(before));
        if change != "incomplete" {
            plans.extend(opening());
            plans.extend(voucher_read(after));
        }
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let response = server
            .call_tool(
                "ledger_movement",
                json!({
                    "company_guid":"61c6de69-1748-461c-ad3f-162cb949df9f",
                    "from":"20260801", "to":"20260802", "ledger":"WR2 Sales"
                }),
            )
            .await;
        if changed {
            assert_eq!(response["isError"], true, "{response}");
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"],
                if change == "incomplete" {
                    "voucher_entries_unbalanced"
                } else {
                    "voucher_snapshot_drifted"
                },
                "{response}"
            );
            assert_eq!(
                response["structuredContent"]["evidence"]["state"],
                "partial"
            );
            assert!(
                response["structuredContent"]["evidence"]["bytes"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
        } else {
            assert_eq!(response["isError"], false, "{response}");
            assert_eq!(
                response["structuredContent"]["evidence"]["state"],
                "complete"
            );
            assert_eq!(
                response["structuredContent"]["result"]["voucher_rows_observed"],
                3
            );
        }
        let observations = simulator.finish().unwrap();
        assert_eq!(
            observations.len(),
            if change == "incomplete" { 28 } else { 52 }
        );
    }
}
