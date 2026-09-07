//! New file generation is limited to the voucher shape qualified live.
use super::*;

#[tokio::test]
async fn unqualified_voucher_types_are_refused_before_dispatch_or_persistence() {
    assert_eq!(
        voucher_input_schema()["properties"]["vouchers"]["items"]["properties"]["voucher_type"]
            ["enum"],
        json!(["Journal"])
    );
    for unqualified in [
        VoucherType::Payment,
        VoucherType::Receipt,
        VoucherType::Contra,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let server = Server::new(super::super::super::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".into(),
                port: 9,
            },
            data_dir: directory.path().into(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: super::super::super::Redaction::None,
            import_enabled: true,
            writes_enabled: false,
        });
        let mut input = payload();
        input.vouchers[0].voucher_type = VoucherType::Journal;
        input.vouchers[1].voucher_type = unqualified;
        let error = server
            .build_import_xml(&serde_json::to_value(input).unwrap())
            .await
            .err()
            .unwrap();
        assert_eq!(error.code, "import_voucher_type_unqualified");
        assert!(error.evidence.is_none(), "refusal precedes any source read");
        assert!(!directory.path().join("imports").exists());
        assert!(!directory.path().join("agent-import-ledger.jsonl").exists());
    }
}

#[test]
fn journal_renderer_preserves_create_remote_identity_with_optional_number() {
    let mut voucher = captured_catalogue_payload().vouchers.remove(0);
    for number in [None, Some("CLIENT-42".to_string())] {
        voucher.voucher_number = number.clone();
        let xml = render_import_xml(
            "Synthetic Book",
            std::slice::from_ref(&voucher),
            "batch-render",
        );
        assert_eq!(
            xml,
            render_import_xml(
                "Synthetic Book",
                std::slice::from_ref(&voucher),
                "batch-render"
            )
        );
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut vouchers = 0;
        let mut numbers = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"VOUCHER" => {
                    vouchers += 1;
                    let attributes = tag
                        .attributes()
                        .map(|attribute| {
                            let attribute = attribute.unwrap();
                            (
                                String::from_utf8(attribute.key.as_ref().to_vec()).unwrap(),
                                attribute
                                    .decoded_and_normalized_value(
                                        quick_xml::XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .unwrap()
                                    .into_owned(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>();
                    assert_eq!(
                        attributes,
                        BTreeMap::from([
                            ("ACTION".into(), "Create".into()),
                            (
                                "REMOTEID".into(),
                                import_identity("batch-render", &voucher.bridge_txn_id).to_string()
                            ),
                            ("VCHTYPE".into(), "Journal".into()),
                            ("OBJVIEW".into(), "Accounting Voucher View".into()),
                        ])
                    );
                }
                quick_xml::events::Event::Start(tag) if tag.name().as_ref() == b"VOUCHERNUMBER" => {
                    numbers.push(
                        reader
                            .read_text(tag.name())
                            .unwrap()
                            .decode()
                            .unwrap()
                            .into_owned(),
                    );
                }
                quick_xml::events::Event::Start(tag) | quick_xml::events::Event::Empty(tag) => {
                    assert!(!matches!(
                        tag.name().as_ref(),
                        b"GUID" | b"MASTERID" | b"REMOTEID"
                    ));
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(vouchers, 1);
        assert_eq!(numbers, number.into_iter().collect::<Vec<_>>());
    }
}
