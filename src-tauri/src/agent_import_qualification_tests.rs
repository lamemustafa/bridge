//! New file generation is limited to the voucher shape qualified live.
use super::*;

#[test]
fn the_admitted_schema_offers_exactly_the_qualified_voucher_types() {
    // The schema enum is generated from the same list the build gate reads, so
    // an offered type and an admitted type cannot drift apart.
    assert_eq!(
        voucher_input_schema()["properties"]["vouchers"]["items"]["properties"]["voucher_type"]
            ["enum"],
        json!(["Journal", "Payment", "Receipt", "Contra"])
    );
    assert_eq!(
        serde_json::to_value(LIVE_QUALIFIED_VOUCHER_TYPES).unwrap(),
        json!(["Journal", "Payment", "Receipt", "Contra"])
    );
}

#[test]
fn a_voucher_type_outside_the_qualified_list_is_refused() {
    // Every declared type is qualified today, so the gate is exercised against
    // a narrowed list. It is what refuses a type added ahead of its evidence,
    // and it runs before the first live request of a build.
    let vouchers = payload().vouchers;
    assert_eq!(
        refuse_unqualified_types(&vouchers, LIVE_QUALIFIED_VOUCHER_TYPES),
        Ok(())
    );
    for unqualified in [
        VoucherType::Payment,
        VoucherType::Receipt,
        VoucherType::Contra,
    ] {
        let qualified = LIVE_QUALIFIED_VOUCHER_TYPES
            .iter()
            .filter(|candidate| **candidate != unqualified)
            .cloned()
            .collect::<Vec<_>>();
        let mut input = payload();
        input.vouchers[0].voucher_type = VoucherType::Journal;
        input.vouchers[1].voucher_type = unqualified;
        assert_eq!(
            refuse_unqualified_types(&input.vouchers, &qualified),
            Err("import_voucher_type_unqualified".to_string())
        );
    }
}

#[tokio::test]
async fn an_unshaped_bank_voucher_is_refused_before_dispatch_or_persistence() {
    // Each payload below is refused on its own shape rule, against a closed
    // port: no live read may be attempted and no local file may appear.
    for (reason, mutate) in [
        (
            "voucher_entry_pair_required",
            Box::new(|voucher: &mut ImportVoucher| {
                voucher.entries.push(ImportEntry {
                    ledger: "Rounding".into(),
                    amount: "1.00".into(),
                    side: EntrySide::Dr,
                });
                voucher.entries.push(ImportEntry {
                    ledger: "Rounding Off".into(),
                    amount: "1.00".into(),
                    side: EntrySide::Cr,
                });
            }) as Box<dyn Fn(&mut ImportVoucher)>,
        ),
        (
            "voucher_entry_ledger_repeated",
            Box::new(|voucher: &mut ImportVoucher| {
                let ledger = voucher.entries[0].ledger.clone();
                voucher.entries[1].ledger = ledger;
            }),
        ),
        (
            "voucher_number_unqualified_for_type",
            Box::new(|voucher: &mut ImportVoucher| {
                voucher.voucher_number = Some("PMT/0001".into());
            }),
        ),
    ] {
        for voucher_type in [
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
            input.vouchers.truncate(1);
            input.vouchers[0].voucher_type = voucher_type.clone();
            input.vouchers[0].voucher_number = None;
            mutate(&mut input.vouchers[0]);
            let error = server
                .build_import_xml(&serde_json::to_value(input).unwrap())
                .await
                .err()
                .unwrap();
            assert_eq!(error.code, reason);
            assert!(error.evidence.is_none(), "refusal precedes any source read");
            assert!(!directory.path().join("imports").exists());
            assert!(!directory.path().join("agent-import-ledger.jsonl").exists());
        }
    }
}

#[test]
fn a_journal_keeps_every_shape_freedom_a_bank_voucher_gives_up() {
    // The constraints above are per type, not a new global rule: the Journal
    // file shape qualified in section 9.8 is unchanged by any of them.
    let mut voucher = payload().vouchers.remove(0);
    voucher.voucher_type = VoucherType::Journal;
    voucher.voucher_number = Some("JV/0001".into());
    voucher.entries.push(ImportEntry {
        ledger: "Rounding".into(),
        amount: "1.00".into(),
        side: EntrySide::Dr,
    });
    voucher.entries.push(ImportEntry {
        ledger: "Rounding Off".into(),
        amount: "1.00".into(),
        side: EntrySide::Cr,
    });
    assert_eq!(
        validate_payload(&ImportPayload {
            company_guid: GUID.into(),
            vouchers: vec![voucher],
        }),
        Ok(())
    );
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
