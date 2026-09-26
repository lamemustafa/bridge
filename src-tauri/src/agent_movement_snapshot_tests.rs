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
    // The movement read proves the book keeps one Currency master (#716).
    let currency = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
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
            currency.clone(),
            status.clone(),
            currency.clone(),
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
            // Each opening read now includes its paired currency read (#716).
            if change == "incomplete" { 38 } else { 66 }
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
    // The movement read proves the book keeps one Currency master (#716).
    let currency = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
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
            currency.clone(),
            status.clone(),
            currency.clone(),
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

// -- bridge#716: a named ledger's movement names no currency ----------------

async fn movement_call(plans: Vec<ScenarioPlan>, guid: &str, ledger: &str) -> (Value, usize) {
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
            json!({"company_guid": guid, "from":"20260915", "to":"20260915", "ledger": ledger}),
        )
        .await;
    (response, simulator.finish().unwrap().len())
}

/// Identity, then the movement's first read up to and including its paired
/// currency read, which the refusal ends on.
fn opening_through_currency(extent: ScenarioPlan, currency: ScenarioPlan) -> Vec<ScenarioPlan> {
    let company = captured(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
    ));
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime));
    vec![
        company.clone(),
        status.clone(),
        company.clone(),
        status.clone(),
        status.clone(),
        company.clone(),
        company,
        extent.clone(),
        status.clone(),
        extent,
        status.clone(),
        currency.clone(),
        status.clone(),
        currency,
        status,
    ]
}

/// A movement on the captured several-currency book is refused after its
/// currency read, before any ledger or voucher request: an opening and a
/// movement name no currency, so a dollar ledger's figures would carry
/// nothing to say they are not rupees (#716). Named here is a dollar ledger
/// of that book; a rupee ledger is refused the same way.
#[tokio::test]
async fn a_movement_on_a_several_currency_book_is_refused_before_any_ledger() {
    let plans = opening_through_currency(
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/company_extents_forex_live.utf16le.xml"
        )),
        captured(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_multi_live.utf16le.xml"
        )),
    );
    let total = plans.len();
    let (response, requests) = movement_call(
        plans,
        "b14e9b2d-8a63-4779-804d-25d59eb787eb",
        "FX USD Debtor 01",
    )
    .await;
    assert_eq!(requests, total, "no ledger or voucher request was sent");
    let error = &response["structuredContent"]["result"]["error"];
    assert_eq!(response["isError"], true, "{response}");
    assert_eq!(error["code"], "ledger_movement_read_failed");
    assert_eq!(error["cause"], "company_several_currency_masters");
    let remediation = error["remediation"].as_str().unwrap();
    assert!(remediation.contains("ledger_movement"), "{error}");
}

/// A movement whose currency collection holds no master is refused after
/// it, before any ledger or voucher request. DERIVED from the captured
/// single-master response with its one `CURRENCY` element removed (#716).
#[tokio::test]
async fn a_movement_with_no_currency_master_is_refused_before_any_ledger() {
    let single = String::from_utf16(
        &include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        )
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>(),
    )
    .unwrap();
    let start = single.find("<CURRENCY ").unwrap();
    let end = start + single[start..].find("</CURRENCY>").unwrap() + "</CURRENCY>".len();
    let mut none = single.clone();
    none.replace_range(start..end, "");
    assert!(!none.contains("<CURRENCY "), "no master left");
    let plans = opening_through_currency(
        captured_utf8(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )),
        captured_utf8(&none),
    );
    let total = plans.len();
    let (response, requests) =
        movement_call(plans, "61c6de69-1748-461c-ad3f-162cb949df9f", "WR2 Sales").await;
    assert_eq!(requests, total, "no ledger or voucher request was sent");
    let error = &response["structuredContent"]["result"]["error"];
    assert_eq!(response["isError"], true, "{response}");
    assert_eq!(error["cause"], "company_currency_probe_failed");
}

/// A movement on a book whose one Currency master is not INR is refused after
/// its currency read, before any ledger or voucher request (#716). DERIVED
/// from the captured single-master response with its `MAILINGNAME` changed
/// from `INR`; no non-INR book has been captured.
#[tokio::test]
async fn a_movement_on_a_non_inr_book_is_refused_before_any_ledger() {
    let single = String::from_utf16(
        &include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        )
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>(),
    )
    .unwrap();
    let inr = "<MAILINGNAME TYPE=\"String\">INR</MAILINGNAME>";
    assert_eq!(single.matches(inr).count(), 1);
    let foreign = single.replace(inr, "<MAILINGNAME TYPE=\"String\">UAE Dirham</MAILINGNAME>");
    let plans = opening_through_currency(
        captured_utf8(include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        )),
        captured_utf8(&foreign),
    );
    let total = plans.len();
    let (response, requests) =
        movement_call(plans, "61c6de69-1748-461c-ad3f-162cb949df9f", "WR2 Sales").await;
    assert_eq!(requests, total, "no ledger or voucher request was sent");
    let error = &response["structuredContent"]["result"]["error"];
    assert_eq!(response["isError"], true, "{response}");
    assert_eq!(error["code"], "ledger_movement_read_failed");
    assert_eq!(error["cause"], "company_base_currency_not_inr");
}
