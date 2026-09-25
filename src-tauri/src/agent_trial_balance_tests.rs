use super::page_boundary;

#[test]
fn maximum_offset_returns_an_empty_page_without_overflowing() {
    assert_eq!(page_boundary(usize::MAX, 0, 1), (false, None));
    assert_eq!(page_boundary(0, 2, 3), (true, Some(2)));
    assert_eq!(page_boundary(2, 1, 3), (false, None));
}

// -- #630: one read per logical trial-balance listing -------------------------

mod listing {
    use super::super::*;
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    const GUID: &str = "eebb9a9f-1679-4468-9e8f-814c729674cb";

    fn decode(bytes: &[u8]) -> String {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    fn companies() -> String {
        decode(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-release-companies.utf16le.xml"
        ))
    }

    /// The captured extents, with only this company's voucher mark changed
    /// when `vouchers` differs from the captured 14.
    fn extents(vouchers: u64) -> String {
        let extent = include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents-with-number.utf8.xml"
        );
        let at = extent.find(GUID).unwrap();
        let start = extent[..at].rfind("<COMPANY ").unwrap();
        let end = at + extent[at..].find("</COMPANY>").unwrap();
        let from = "<ALTVCHID TYPE=\"Number\"> 14</ALTVCHID>";
        assert_eq!(extent[start..end].matches(from).count(), 1);
        format!(
            "{}{}{}",
            &extent[..start],
            extent[start..end].replace(
                from,
                &format!("<ALTVCHID TYPE=\"Number\"> {vouchers}</ALTVCHID>")
            ),
            &extent[end..]
        )
    }

    fn xml(text: String) -> ScenarioPlan {
        ScenarioPlan::new(Fixture::SyntheticXml(text))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    }

    fn status() -> ScenarioPlan {
        ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
    }

    fn pair(plans: &mut Vec<ScenarioPlan>, response: ScenarioPlan) {
        plans.extend([response.clone(), status(), response, status()]);
    }

    fn identity_plans() -> Vec<ScenarioPlan> {
        let mut plans = Vec::new();
        pair(&mut plans, xml(companies()));
        plans
    }

    /// A whole first-page call: identity, then the runtime read's brackets,
    /// currency and report, as `runtime_trial_balance_tests` replays them.
    fn first_page_plans(vouchers: u64) -> Vec<ScenarioPlan> {
        let companies = xml(companies());
        let currency = decode(include_bytes!(
            "../crates/bridge-tally-protocol/tests/fixtures/currency_inr_modern_live.utf16le.xml"
        ));
        let report = include_str!(
            "../crates/bridge-tally-protocol/tests/fixtures/native/trial_balance_known_lab.xml"
        )
        .to_string();
        let mut plans = identity_plans();
        plans.extend([status(), companies.clone(), companies.clone()]);
        pair(&mut plans, xml(extents(vouchers)));
        pair(&mut plans, xml(currency));
        pair(&mut plans, xml(report));
        pair(&mut plans, xml(extents(vouchers)));
        plans.extend([companies.clone(), status(), companies]);
        plans
    }

    /// A continuation page: identity, then the bracketed, paired extent read.
    fn continuation_plans(vouchers: u64) -> Vec<ScenarioPlan> {
        let companies = xml(companies());
        let mut plans = identity_plans();
        plans.push(companies.clone());
        pair(&mut plans, xml(extents(vouchers)));
        plans.push(companies);
        plans
    }

    struct OneServer {
        simulator: SequenceSimulator,
        server: Server,
        _directory: tempfile::TempDir,
    }

    impl OneServer {
        fn spawn(plans: Vec<ScenarioPlan>) -> Self {
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
                writes_enabled: false,
            });
            Self {
                simulator,
                server,
                _directory: directory,
            }
        }

        async fn call(&self, args: Value) -> Value {
            self.server.call_tool("trial_balance", args).await
        }

        fn requests(self) -> usize {
            self.simulator.finish().unwrap().len()
        }
    }

    fn result(response: &Value) -> &Value {
        assert_ne!(response["isError"], true, "{response}");
        &response["structuredContent"]["result"]
    }

    fn page(from: &str, offset: usize, snapshot_id: Option<&str>) -> Value {
        let mut args =
            json!({"company_guid":GUID,"from":from,"to":"2026-09-02","limit":3,"offset":offset});
        if let Some(id) = snapshot_id {
            args["snapshot_id"] = json!(id);
        }
        args
    }

    /// Page 2 costs the identity read and one extent read, and continues the
    /// same report: its rows follow page 1's, and the frame is the same.
    #[tokio::test]
    async fn a_continuation_page_continues_the_first_pages_report() {
        let mut plans = first_page_plans(14);
        plans.extend(continuation_plans(14));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(page("2026-04-01", 0, None)).await;
        let id = result(&first)["snapshot"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let second = one.call(page("2026-04-01", 3, Some(&id))).await;
        assert_eq!(result(&second)["snapshot"]["reused"], true);
        assert_eq!(result(&second)["snapshot"]["voucher_alter_id"], 14);
        assert_eq!(result(&second)["totals"], result(&first)["totals"]);
        assert_eq!(result(&second)["read_at"], result(&first)["read_at"]);
        assert_eq!(one.requests(), total);

        let whole = OneServer::spawn(first_page_plans(14));
        let all = whole
            .call(json!({"company_guid":GUID,"from":"2026-04-01","to":"2026-09-02"}))
            .await;
        let all = result(&all)["ledgers"].as_array().unwrap().clone();
        assert!(all.len() > 3, "the capture has more rows than one page");
        assert_eq!(
            result(&first)["ledgers"].as_array().unwrap().as_slice(),
            &all[..3]
        );
        assert_eq!(
            result(&second)["ledgers"].as_array().unwrap().as_slice(),
            &all[3..all.len().min(6)]
        );
    }

    /// A voucher posted since page 1 moves ALTVCHID: named, the listing is
    /// refused with nothing read after the extent check; unnamed, the page
    /// reads a fresh report.
    #[tokio::test]
    async fn a_moved_voucher_mark_is_refused_by_id_or_read_fresh() {
        let mut plans = first_page_plans(14);
        plans.extend(continuation_plans(15));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(page("2026-04-01", 0, None)).await;
        let id = result(&first)["snapshot"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let refused = one.call(page("2026-04-01", 3, Some(&id))).await;
        assert_eq!(refused["isError"], true, "{refused}");
        let error = &refused["structuredContent"]["result"]["error"];
        assert_eq!(error["code"], "listing_snapshot_changed");
        assert_eq!(error["cause"], "book_changed_since_first_page");
        assert_eq!(one.requests(), total);

        let mut plans = first_page_plans(14);
        plans.extend(continuation_plans(15));
        plans.extend(
            first_page_plans(15)
                .into_iter()
                .skip(identity_plans().len()),
        );
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let _ = one.call(page("2026-04-01", 0, None)).await;
        let fresh = one.call(page("2026-04-01", 3, None)).await;
        assert_eq!(result(&fresh)["snapshot"]["reused"], false);
        assert_eq!(result(&fresh)["snapshot"]["voucher_alter_id"], 15);
        assert_eq!(one.requests(), total);
    }

    /// Another period is another listing, and a write through this server
    /// drops the company's snapshots: neither continues from the held report.
    #[tokio::test]
    async fn another_period_or_a_write_does_not_continue_the_held_report() {
        let mut plans = first_page_plans(14);
        plans.extend(continuation_plans(14));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(page("2026-04-01", 0, None)).await;
        let id = result(&first)["snapshot"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let other = one.call(page("2026-05-01", 3, Some(&id))).await;
        assert_eq!(
            other["structuredContent"]["result"]["error"]["cause"], "snapshot_not_held",
            "{other}"
        );
        assert_eq!(one.requests(), total);

        let mut plans = first_page_plans(14);
        plans.extend(continuation_plans(14));
        let total = plans.len();
        let one = OneServer::spawn(plans);
        let first = one.call(page("2026-04-01", 0, None)).await;
        let id = result(&first)["snapshot"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        one.server.drop_listing_snapshots(GUID);
        let dropped = one.call(page("2026-04-01", 3, Some(&id))).await;
        assert_eq!(
            dropped["structuredContent"]["result"]["error"]["cause"], "snapshot_not_held",
            "{dropped}"
        );
        assert_eq!(one.requests(), total);
    }
}
