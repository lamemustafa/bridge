use super::*;

const COMPANY: &str = "de2e15f2-6d42-4715-b6e7-b7a95a68abe8";

/// A row as the parser emits it for a class-resolving read. A type's own
/// reserved name is its class name when it is a reserved type of a measured
/// class, and empty for a type the user created.
fn row(name: &str, guid_suffix: &str, class: Option<&str>) -> Value {
    let reserved = if class.is_some_and(|class| class.eq_ignore_ascii_case(name)) {
        class.unwrap()
    } else {
        ""
    };
    reserved_row(name, guid_suffix, class, reserved)
}

fn reserved_row(name: &str, guid_suffix: &str, class: Option<&str>, reserved: &str) -> Value {
    json!({
        "voucher_type": name,
        "voucher_type_guid": format!("{COMPANY}-{guid_suffix}"),
        "voucher_type_reserved_name": reserved,
        "voucher_class": class,
    })
}

/// The types of the #625 book as captured on 7.1: the reserved Purchase type
/// renamed to `PURCHASE A/C` (two vouchers) and its child `Purchase Local`
/// (one). A user type named exactly `Purchase` under Attendance exists in that
/// book but has no voucher in the window.
fn renamed_book_rows() -> Vec<Value> {
    vec![
        reserved_row("PURCHASE A/C", "0000002e", Some("Purchase"), "Purchase"),
        reserved_row("PURCHASE A/C", "0000002e", Some("Purchase"), "Purchase"),
        row("Purchase Local", "000000d4", Some("Purchase")),
    ]
}

fn names(selection: &VoucherTypeSelection) -> Vec<&str> {
    selection
        .rows
        .iter()
        .map(|row| row["voucher_type"].as_str().unwrap())
        .collect()
}

#[test]
fn a_class_request_finds_a_renamed_type_and_its_child_where_the_name_found_nothing() {
    let selection = select_voucher_rows(
        renamed_book_rows(),
        &VoucherTypeSelector::Class(ReservedVoucherClass::Purchase),
    )
    .unwrap();
    assert_eq!(
        names(&selection),
        ["PURCHASE A/C", "PURCHASE A/C", "Purchase Local"]
    );
    assert_eq!(selection.included.len(), 2);

    // The #625 false zero: the class name as a display name matches no row,
    // so it is refused and the purchase types are named, not returned as zero.
    let refusal = select_voucher_rows(
        renamed_book_rows(),
        &VoucherTypeSelector::Name("Purchase".to_string()),
    )
    .unwrap_err();
    assert_eq!(refusal.code, "voucher_type_ambiguous");
    assert_eq!(
        refusal
            .candidates
            .iter()
            .map(|kind| (kind.name.as_str(), kind.rows))
            .collect::<Vec<_>>(),
        [("PURCHASE A/C", 2), ("Purchase Local", 1)]
    );
}

#[test]
fn a_class_name_is_ambiguous_whenever_its_name_set_and_class_set_differ() {
    // Unrenamed book, reserved type only: the name and the class agree.
    let stock = || vec![row("Purchase", "0000002e", Some("Purchase"))];
    // Tally resolves a type name ignoring case, and so does the filter: a
    // lower-case name selects the type rather than a zero.
    assert_eq!(
        select_voucher_rows(stock(), &VoucherTypeSelector::Name("purchase".to_string()))
            .map(|selection| names(&selection).len())
            .map_err(|refusal| refusal.code),
        Ok(1)
    );
    assert_eq!(
        select_voucher_rows(
            renamed_book_rows(),
            &VoucherTypeSelector::Name("purchase local".to_string())
        )
        .map(|selection| names(&selection).len())
        .map_err(|refusal| refusal.code),
        Ok(1)
    );
    assert_eq!(
        select_voucher_rows(stock(), &VoucherTypeSelector::Name("Purchase".to_string()))
            .map(|selection| names(&selection).len())
            .map_err(|refusal| refusal.code),
        Ok(1)
    );
    // Unrenamed book with a child type: the name alone would be a silent part.
    let mut with_child = stock();
    with_child.push(row("Purchase Local", "000000d4", Some("Purchase")));
    assert_eq!(
        select_voucher_rows(
            with_child,
            &VoucherTypeSelector::Name("Purchase".to_string())
        )
        .map_err(|refusal| refusal.code)
        .unwrap_err(),
        "voucher_type_ambiguous"
    );
    // A type of another class carrying the class name.
    let impostor = vec![
        row("Purchase", "000000d2", None),
        reserved_row("PURCHASE A/C", "0000002e", Some("Purchase"), "Purchase"),
    ];
    let refusal = select_voucher_rows(impostor, &VoucherTypeSelector::Name("Purchase".to_string()))
        .unwrap_err();
    assert_eq!(refusal.code, "voucher_type_ambiguous");
    assert_eq!(
        refusal
            .candidates
            .iter()
            .map(|kind| kind.name.as_str())
            .collect::<Vec<_>>(),
        ["PURCHASE A/C", "Purchase"],
        "both the type of the class and the type carrying the name, in GUID order"
    );
    // Any reserved name, not only the eight classes: a renamed Stock Journal
    // is still the type reserving that name.
    let stock_journal = |extra: Option<Value>| {
        let mut rows = vec![reserved_row("STOCK JNL", "0000003a", None, "Stock Journal")];
        rows.extend(extra);
        select_voucher_rows(
            rows,
            &VoucherTypeSelector::Name("Stock Journal".to_string()),
        )
        .map(|selection| names(&selection).len())
        .map_err(|refusal| refusal.code)
    };
    assert_eq!(stock_journal(None), Err("voucher_type_ambiguous"));
    assert_eq!(
        stock_journal(Some(row("Stock Journal", "000000e1", None))),
        Err("voucher_type_ambiguous"),
        "a user type carrying the reserved name beside the renamed reserved type"
    );
    assert_eq!(
        select_voucher_rows(
            vec![reserved_row(
                "Stock Journal",
                "0000003a",
                None,
                "Stock Journal"
            )],
            &VoucherTypeSelector::Name("Stock Journal".to_string())
        )
        .map(|selection| names(&selection).len())
        .map_err(|refusal| refusal.code),
        Ok(1),
        "an unrenamed reserved type answers by its name"
    );
    // A name that is not a class name is exact, as before.
    let selection = select_voucher_rows(
        renamed_book_rows(),
        &VoucherTypeSelector::Name("Purchase Local".to_string()),
    )
    .unwrap();
    assert_eq!(names(&selection), ["Purchase Local"]);
}

#[test]
fn one_type_guid_with_two_names_or_classes_in_one_window_is_refused() {
    let mut rows = renamed_book_rows();
    rows.push(reserved_row(
        "PURCHASE RENAMED",
        "0000002e",
        Some("Purchase"),
        "Purchase",
    ));
    assert_eq!(
        select_voucher_rows(
            rows,
            &VoucherTypeSelector::Class(ReservedVoucherClass::Purchase)
        )
        .unwrap_err()
        .code,
        "voucher_type_snapshot_inconsistent"
    );
    let mut reclassed = renamed_book_rows();
    reclassed.push(row("Purchase Local", "000000d4", Some("Sales")));
    assert_eq!(
        select_voucher_rows(
            reclassed,
            &VoucherTypeSelector::Class(ReservedVoucherClass::Purchase)
        )
        .unwrap_err()
        .code,
        "voucher_type_snapshot_inconsistent",
        "one GUID with two classes"
    );
    let selection = select_voucher_rows(
        renamed_book_rows(),
        &VoucherTypeSelector::Guid(format!("{COMPANY}-000000d4")),
    )
    .unwrap();
    assert_eq!(
        names(&selection),
        ["Purchase Local"],
        "the GUID selector keeps one type"
    );
    assert_eq!(selection.window_types.len(), 2);
    let unresolved = vec![json!({"voucher_type": "Purchase"})];
    assert_eq!(
        select_voucher_rows(
            unresolved,
            &VoucherTypeSelector::Class(ReservedVoucherClass::Purchase)
        )
        .unwrap_err()
        .code,
        "voucher_type_class_missing",
        "a row the read did not resolve is never treated as another class"
    );
}

fn resolved_row(values: &[(&str, &str)]) -> Map<String, String> {
    values
        .iter()
        .map(|(tag, value)| (tag.to_string(), value.to_string()))
        .collect()
}

fn purchase_row(reserved: &str) -> Vec<(&'static str, String)> {
    let mut values = vec![
        (VOUCHER_TYPE_GUID_TAG, format!("{COMPANY}-0000002e")),
        (VOUCHER_TYPE_RESERVED_NAME_TAG, reserved.to_string()),
    ];
    for class in ReservedVoucherClass::ALL {
        let answer = if class == ReservedVoucherClass::Purchase {
            "Yes"
        } else {
            "No"
        };
        values.push((class.tag(), answer.to_string()));
    }
    values
}

#[test]
fn a_row_resolves_only_from_every_class_element_and_fails_closed_on_a_gap() {
    let full = purchase_row("");
    let as_map = |values: &[(&str, String)]| {
        resolved_row(
            &values
                .iter()
                .map(|(t, v)| (*t, v.as_str()))
                .collect::<Vec<_>>(),
        )
    };
    assert_eq!(
        resolve_row_voucher_type(&as_map(&full), COMPANY),
        Ok(Some(ResolvedVoucherType {
            guid: format!("{COMPANY}-0000002e"),
            reserved_name: String::new(),
            class: Some(ReservedVoucherClass::Purchase),
        })),
        "a child type: no reserved name of its own, classed by the function"
    );
    assert_eq!(resolve_row_voucher_type(&Map::new(), COMPANY), Ok(None));

    // An unknown function omits its element: a gap is refused, never a `No`.
    let gap = full
        .iter()
        .filter(|(tag, _)| *tag != ReservedVoucherClass::Sales.tag())
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        resolve_row_voucher_type(&as_map(&gap), COMPANY),
        Err("voucher_type_class_missing".to_string())
    );
    let mut foreign = full.clone();
    foreign[0].1 = "0f000000-0000-0000-0000-000000000000-0000002e".to_string();
    assert_eq!(
        resolve_row_voucher_type(&as_map(&foreign), COMPANY),
        Err("voucher_type_unresolved".to_string())
    );
    let mut two = full.clone();
    two.iter_mut()
        .find(|(tag, _)| *tag == ReservedVoucherClass::Sales.tag())
        .unwrap()
        .1 = "Yes".to_string();
    assert_eq!(
        resolve_row_voucher_type(&as_map(&two), COMPANY),
        Err("voucher_type_class_contradictory".to_string())
    );
    assert_eq!(
        resolve_row_voucher_type(&as_map(&purchase_row("Sales")), COMPANY),
        Err("voucher_type_class_contradictory".to_string()),
        "the type's own reserved name disagrees with the class function"
    );
    assert_eq!(
        resolve_row_voucher_type(&as_map(&purchase_row("Memorandum")), COMPANY),
        Err("voucher_type_class_contradictory".to_string()),
        "a reserved type outside the measured classes answering Yes is unmeasured"
    );
    for answer in ["", "Maybe"] {
        let mut invalid = full.clone();
        invalid
            .iter_mut()
            .find(|(tag, _)| *tag == ReservedVoucherClass::Sales.tag())
            .unwrap()
            .1 = answer.to_string();
        assert_eq!(
            resolve_row_voucher_type(&as_map(&invalid), COMPANY),
            Err("voucher_type_class_invalid".to_string()),
            "{answer:?} is not a No"
        );
    }
}

#[test]
fn the_selectors_are_exclusive_and_the_schema_offers_only_measured_classes() {
    let parse = |args: Value| VoucherTypeSelector::from_args(&args).map_err(|failure| failure.code);
    assert_eq!(parse(json!({})), Ok(None));
    assert_eq!(
        parse(json!({"voucher_class": "Debit Note"})),
        Ok(Some(VoucherTypeSelector::Class(
            ReservedVoucherClass::DebitNote
        )))
    );
    assert_eq!(
        parse(json!({"voucher_type": ""})),
        Err("voucher_type_invalid".to_string())
    );
    assert_eq!(
        parse(json!({"voucher_class": "Attendance"})),
        Err("voucher_class_unsupported".to_string())
    );
    assert_eq!(
        parse(json!({"voucher_class": "Purchase", "voucher_type": "Purchase"})),
        Err("voucher_type_selectors_conflict".to_string())
    );

    let tools = tool_definitions(false, false);
    let schema = tools
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "vouchers")
        .expect("vouchers is listed");
    let offered = schema["inputSchema"]["properties"]["voucher_class"]["enum"].clone();
    assert_eq!(
        offered,
        json!(ReservedVoucherClass::ALL.map(ReservedVoucherClass::name))
    );

    // The class read is the plain read plus exactly the class COMPUTEs.
    let (company, from, to) = ("Synthetic Book", "20250701", "20250731");
    let plain = VoucherReadShape::EntryWildcard
        .render(company, from, to, None)
        .unwrap();
    let classed = VoucherReadShape::ClassEntryWildcard
        .render(company, from, to, None)
        .unwrap();
    assert_eq!(
        classed.replacen(&voucher_type_class_computes(), "", 1),
        plain
    );
    for class in ReservedVoucherClass::ALL {
        assert!(classed.contains(&format!(
            "{}:{}:$VoucherTypeName",
            class.tag(),
            class.function()
        )));
    }
}

fn captured_window() -> String {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-vouchers-renamed-purchase-class.utf16le.xml"
    );
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

#[test]
fn the_live_capture_classes_a_renamed_type_and_its_child_from_the_request_sent() {
    // The fixture answers exactly the request this read sends.
    let request = VoucherReadShape::ClassEntryWildcard
        .render("BRIDGE READS LAB", "20250701", "20250731", None)
        .unwrap();
    assert_eq!(
        sha256_hex(request.as_bytes()),
        "913a6a68694dd09c2955dbc0e694e40072435b69fe7788e180f1df1bafe39279"
    );
    let rows = parse_agent_rows(&captured_window(), COMPANY).expect("the live capture parses");
    assert_eq!(
        rows.iter()
            .map(|row| (
                row["voucher_type"].as_str().unwrap(),
                row["voucher_class"].as_str().unwrap(),
                &row["voucher_type_guid"].as_str().unwrap()[37..]
            ))
            .collect::<Vec<_>>(),
        [
            ("PURCHASE A/C", "Purchase", "0000002e"),
            ("PURCHASE A/C", "Purchase", "0000002e"),
            ("Purchase Local", "Purchase", "000000d4"),
        ],
        "the child has no reserved name of its own and is still classed Purchase"
    );
    let selection = select_voucher_rows(
        rows.clone(),
        &VoucherTypeSelector::Class(ReservedVoucherClass::Purchase),
    )
    .unwrap();
    assert_eq!(selection.rows.len(), 3);
    assert_eq!(
        select_voucher_rows(rows, &VoucherTypeSelector::Name("Purchase".to_string()))
            .unwrap_err()
            .code,
        "voucher_type_ambiguous"
    );
}

mod through_the_tool {
    use super::*;
    use tally_protocol_simulator::{
        Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    fn xml(body: String) -> ScenarioPlan {
        ScenarioPlan::new(Fixture::SyntheticXml(body))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength)
    }

    fn status() -> ScenarioPlan {
        ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
            .with_framing(ResponseFraming::ContentLength)
    }

    fn company() -> ScenarioPlan {
        xml(format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"BRIDGE READS LAB\"><GUID>{COMPANY}</GUID><COMPANYNUMBER>100023</COMPANYNUMBER><BOOKSFROM>20250401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>"))
    }

    /// Identity, then the high-water pre-flight and the window, each read in
    /// its company bracket. The window is the live capture; the rest frames it.
    fn plans() -> Vec<ScenarioPlan> {
        let high_water = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>{COMPANY}</GUID><ALTVCHID>3</ALTVCHID><ALTMSTID>224</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>");
        let mut plans = vec![company(), status(), company(), status()];
        for payload in [high_water, captured_window()] {
            plans.extend([
                company(),
                xml(payload.clone()),
                status(),
                xml(payload),
                status(),
                company(),
            ]);
        }
        plans
    }

    async fn call(filter: Value) -> Value {
        let simulator = SequenceSimulator::spawn(plans()).expect("simulator");
        let directory = tempfile::tempdir().expect("directory");
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: simulator.address().ip().to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 500,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
        });
        let mut args = json!({"company_guid": COMPANY, "from": "20250701", "to": "20250731"});
        for (key, value) in filter.as_object().unwrap() {
            args[key] = value.clone();
        }
        server.call_tool_response("vouchers", args).await.value
    }

    #[tokio::test]
    async fn a_class_request_returns_every_purchase_and_the_class_name_is_refused_not_zero() {
        let response = call(json!({"voucher_class": "Purchase"})).await;
        assert_ne!(response["isError"], true, "{response}");
        let result = &response["structuredContent"]["result"];
        assert_eq!(result["items"].as_array().unwrap().len(), 3, "{response}");
        assert_eq!(
            result["voucher_types"]["in_scope"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            result["voucher_types"]["included"]
                .as_array()
                .unwrap()
                .iter()
                .map(|kind| (
                    kind["name"].as_str().unwrap(),
                    kind["rows"].as_u64().unwrap()
                ))
                .collect::<Vec<_>>(),
            [("PURCHASE A/C", 2), ("Purchase Local", 1)]
        );

        // Before #625 this returned zero items and no warning.
        let response = call(json!({"voucher_type": "Purchase"})).await;
        assert_eq!(response["isError"], true, "{response}");
        let error = &response["structuredContent"]["result"]["error"];
        assert_eq!(error["code"], "voucher_type_ambiguous");
        assert_eq!(error["candidates_total"], 2);
        assert_eq!(error["candidates_truncated"], false);
        assert_eq!(
            error["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|kind| kind["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["PURCHASE A/C", "Purchase Local"]
        );

        // Another company's type GUID names none of this company's types.
        let response = call(json!({
            "voucher_type_guid": "0f000000-0000-0000-0000-000000000000-0000002e"
        }))
        .await;
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "voucher_type_guid_foreign"
        );
    }
}
