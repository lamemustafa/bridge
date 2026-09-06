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
