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

fn captured_utf8(body: &str) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(body.to_owned()))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}

#[tokio::test]
async fn movement_refuses_voucher_changes_even_when_period_openings_match() {
    let company = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ));
    let extent = captured_utf8(include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    ));
    let ledger = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-period-opening.utf16le.xml"
    ));
    let voucher = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    ));
    // A small synthetic mark for this company, in the shape the import tests
    // already replay: three vouchers cannot exceed the window budget.
    let high_water = captured_utf8(
        "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>61c6de69-1748-461c-ad3f-162cb949df9f</GUID><ALTVCHID>3</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>",
    );
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
        // The pre-flight high-water read (protocol reference §11c) precedes the
        // first window read only; the closing read replays its ranges.
        plans.extend(voucher_read(high_water.clone()));
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
            writes_enabled: false,
            batch_post_enabled: false,
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
            if change == "incomplete" { 34 } else { 58 }
        );
    }
}

/// `count` copies of the captured response's first voucher, AlterIDs `first..`,
/// each with its own GUID, `REMOTEID`, AlterID and master ID, on `date`.
fn many_vouchers(original: &str, ids: std::ops::RangeInclusive<u64>, date: &str) -> String {
    let start = original.find("<VOUCHER ").unwrap();
    let end = start + original[start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
    let last = original.rfind("</VOUCHER>").unwrap() + "</VOUCHER>".len();
    let template = &original[start..end];
    let guid = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let body = ids
        .map(|id| {
            template
                .replace(&format!("{guid}-00000001"), &format!("{guid}-{id:08x}"))
                .replace(
                    "<ALTERID TYPE=\"Number\"> 1</ALTERID>",
                    &format!("<ALTERID TYPE=\"Number\"> {id}</ALTERID>"),
                )
                .replace(
                    "<MASTERID TYPE=\"Number\"> 1</MASTERID>",
                    &format!("<MASTERID TYPE=\"Number\"> {id}</MASTERID>"),
                )
                .replace(
                    "<DATE TYPE=\"Date\">20260801</DATE>",
                    &format!("<DATE TYPE=\"Date\">{date}</DATE>"),
                )
        })
        .collect::<String>();
    format!("{}{body}{}", &original[..start], &original[last..])
}

#[tokio::test]
async fn a_divided_movement_refuses_a_posting_above_the_first_reads_ceiling() {
    // #520 P1, end to end. Day one holds 200 vouchers, more than one read at the
    // movement default carries, so the window (that one day) is read in AlterID
    // spans of it, `(0,170]` and `(170,200]`. Between the first read and
    // its replay a voucher is posted on day one and takes AlterID 201, above
    // the replayed spans' ceiling: the replayed responses are byte-identical to
    // the first read's and the snapshots match. Before the fix the replay read
    // no mark and the movement was returned with the posting in neither read.
    let company = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ));
    let extent = captured_utf8(include_str!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
    ));
    let ledger = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-period-opening.utf16le.xml"
    ));
    let voucher = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    ));
    let marks = |vouchers: u64| {
        captured_utf8(&format!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>61c6de69-1748-461c-ad3f-162cb949df9f</GUID><ALTVCHID>{vouchers}</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"
        ))
    };
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
    let original = voucher.fixture.body().into_owned();
    let with = |xml: String| {
        let mut plan = voucher.clone();
        plan.fixture = Fixture::SyntheticXml(xml);
        plan
    };
    let census = with(many_vouchers(&original, 1..=200, "20260801"));
    let first_span = with(many_vouchers(&original, 1..=170, "20260801"));
    let second_span = with(many_vouchers(&original, 171..=200, "20260801"));
    for (replay_mark, expect_ok) in [(200, true), (201, false)] {
        let mut plans = vec![
            company.clone(),
            status.clone(),
            company.clone(),
            status.clone(),
        ];
        plans.extend(opening());
        plans.extend(voucher_read(marks(200)));
        plans.extend(voucher_read(census.clone()));
        let parts = [first_span.clone(), second_span.clone()];
        for part in &parts {
            plans.extend(voucher_read(part.clone()));
        }
        plans.extend(voucher_read(marks(200)));
        plans.extend(opening());
        for part in &parts {
            plans.extend(voucher_read(part.clone()));
        }
        plans.extend(voucher_read(marks(replay_mark)));
        let simulator = SequenceSimulator::spawn(plans).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 16_000_000,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
            batch_post_enabled: false,
        });
        let response = server
            .call_tool(
                "ledger_movement",
                json!({
                    "company_guid":"61c6de69-1748-461c-ad3f-162cb949df9f",
                    "from":"20260801", "to":"20260801", "ledger":"WR2 Sales"
                }),
            )
            .await;
        if expect_ok {
            assert_eq!(response["isError"], false, "{response}");
            assert_eq!(
                response["structuredContent"]["result"]["voucher_rows_observed"],
                200
            );
        } else {
            assert_eq!(response["isError"], true, "{response}");
            assert_eq!(
                response["structuredContent"]["result"]["error"]["code"],
                "voucher_window_changed_during_read",
                "{response}"
            );
            assert_eq!(
                response["structuredContent"]["evidence"]["state"],
                "partial"
            );
        }
        simulator.finish().unwrap();
    }
}
