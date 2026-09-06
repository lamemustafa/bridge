use super::*;
use crate::agent::*;
use std::path::Path;

fn receipt_for(directory: &Path, response: &Value) -> Value {
    let server = Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: "127.0.0.1".into(),
            port: 9,
        },
        data_dir: directory.to_path_buf(),
        max_rows: 10,
        max_bytes: 200_000,
        redaction: Redaction::DropNarration,
        import_enabled: false,
    });
    let wire = format!("{response}\n");
    server
        .append_framed_egress(
            EgressContext {
                tool: "outstandings".into(),
                args_sha256: sha256_json(&json!({})),
                company_guid: None,
            },
            response,
            &wire,
        )
        .unwrap();
    let receipt: Value = serde_json::from_str(
        fs::read_to_string(directory.join("agent-egress.jsonl"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(receipt["response_sha256"], sha256_hex(wire.as_bytes()));
    assert_eq!(receipt["bytes_prepared"], wire.len());
    receipt
}

#[test]
fn final_outstandings_receipt_covers_dates_directions_derived_totals_and_envelope() {
    let directory = tempfile::tempdir().unwrap();
    let bills = vec![OpenBillRow {
        party: "Private Customer".into(),
        reference: "Sensitive Invoice".into(),
        bill_date: "20260901".into(),
        due_date: "20260902".into(),
        amount: bridge_tally_core::ExactDecimal::parse("12").unwrap(),
        age_days: Some(4),
        kind: ExposureDirection::Receivable,
    }];
    let unallocated = vec![UnallocatedParty {
        party: "Private Supplier".into(),
        amount: bridge_tally_core::ExactDecimal::parse("3").unwrap(),
        direction: ExposureDirection::Payable,
    }];
    let structured = redact_value(
        json!({
            "company":{"name":"Private Company"}, "evidence":{"state":"complete"},
            "read_at":"2026-09-06", "truncated":false,
            "result":{
                "open_bills":bills.iter().map(open_bill_json).collect::<Vec<_>>(),
                "totals":outstanding_totals_from_open_bills(&bills).unwrap(),
                "ageing_buckets":ageing_buckets_from_open_bills(&bills).unwrap(),
                "top_parties":ranked_parties_from_exposure(&bills, &unallocated, 10).unwrap(),
                "unallocated":{"parties":unallocated.iter().map(unallocated_party_json).collect::<Vec<_>>()}
            }
        }),
        Redaction::MaskParties,
    );
    let response = json!({"jsonrpc":"2.0", "id":1, "result":{"structuredContent":structured}});
    let receipt = receipt_for(directory.path(), &response);
    let fields = receipt["fields_prepared"].as_array().unwrap();
    for path in [
        "company.name",
        "evidence.state",
        "read_at",
        "truncated",
        "result.open_bills[].bill_date",
        "result.open_bills[].due_date",
        "result.open_bills[].age_days",
        "result.open_bills[].kind",
        "result.totals.receivable",
        "result.ageing_buckets.days_0_30",
        "result.top_parties[].oldest_bill_age_days",
        "result.top_parties[].gross_exposure",
        "result.top_parties[].unallocated_payable",
        "result.unallocated.parties[].direction",
    ] {
        assert!(fields.contains(&json!(path)), "missing {path}");
    }
    assert_eq!(receipt["rows_prepared"], 2);
    for value in [
        "Private Customer",
        "Private Supplier",
        "Private Company",
        "Sensitive Invoice",
        "20260901",
    ] {
        assert!(!receipt.to_string().contains(value));
    }
}

#[test]
fn receipt_describes_only_rows_and_fields_surviving_redaction_and_byte_trimming() {
    let directory = tempfile::tempdir().unwrap();
    let structured = redact_value(
        json!({"result":{"offset":0,"items":[
        {"party":"Sensitive Customer","narration":"Sensitive Note"},
        {"party":"Removed Customer","narration":"Removed Note","only_removed_row":"x".repeat(1000)}
    ]},"truncated":false}),
        Redaction::DropNarration,
    );
    let (structured, truncated, _) = enforce_response_byte_cap(structured, 200).unwrap();
    assert!(truncated);
    let receipt = receipt_for(
        directory.path(),
        &json!({"result":{"structuredContent":structured}}),
    );
    assert_eq!(
        receipt["fields_prepared"],
        json!([
            "result.items[].party",
            "result.next_offset",
            "result.offset",
            "truncated"
        ])
    );
    assert_eq!(receipt["rows_prepared"], 1);
    assert_eq!(receipt["truncated"], true);
}

#[test]
fn unknown_contract_fields_empty_collections_and_metadata_strings_are_described_without_values() {
    let fields = released_fields(&json!({"result":{
        "new_contract_field":[{"amount":"Private Value"},{"amount":null}],
        "open_bills":[], "empty_object":{},
        "records":["{\"Sensitive Customer\":\"Private Value\"}"]
    }}));
    assert_eq!(
        fields,
        vec![
            "result.empty_object",
            "result.new_contract_field[].amount",
            "result.open_bills[]",
            "result.records[]"
        ]
    );
}
