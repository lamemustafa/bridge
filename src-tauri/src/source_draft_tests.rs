use std::fs;

use super::*;

fn source() -> ParsedSource {
    parse_source_xml(b"<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE><VOUCHER REMOTEID=\"x\" VCHTYPE=\"Receipt\"><DATE>20260901</DATE><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>1</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>", "source.xml".into()).unwrap()
}

#[test]
fn fresh_proposal_preserves_unknown_accounting_choices() {
    let source = source();
    let proposal = empty_proposals(&source);
    assert!(proposal[0].date.is_none());
    assert!(proposal[0].entries[0].side.is_none());
    assert!(proposal[0].notes.is_empty());
}

#[test]
fn editable_optional_text_normalizes_empty_at_json_boundary_without_changing_source_empty_or_whitespace(
) {
    let source = source();
    let mut saved: serde_json::Value =
        serde_json::from_slice(&serialize_draft(&source, &empty_proposals(&source)).unwrap())
            .unwrap();
    let proposal = &mut saved["proposals"][0];
    proposal["date"] = serde_json::Value::String(String::new());
    proposal["narration"] = serde_json::Value::String(String::new());
    proposal["entries"][0]["ledger"] = serde_json::Value::String(String::new());
    proposal["entries"][0]["amount"] = serde_json::Value::String(String::new());
    let (_, normalized_saved) = open_saved_draft(serde_json::to_vec(&saved).unwrap()).unwrap();
    assert!(normalized_saved[0].date.is_none());
    assert!(normalized_saved[0].narration.is_none());
    assert!(normalized_saved[0].entries[0].ledger.is_none());
    assert!(normalized_saved[0].entries[0].amount.is_none());

    let request: SourceDraftSaveRequest = serde_json::from_value(serde_json::json!({
        "draft_id": Uuid::new_v4().to_string(),
        "revision": 1,
        "proposals": [{
            "date": "",
            "voucher_type": null,
            "narration": "",
            "notes": "",
            "entries": [{ "ledger": "", "side": null, "amount": "" }]
        }]
    }))
    .unwrap();
    assert!(request.proposals[0].date.is_none());
    assert!(request.proposals[0].narration.is_none());
    assert!(request.proposals[0].entries[0].ledger.is_none());
    assert!(request.proposals[0].entries[0].amount.is_none());

    let whitespace_request: SourceDraftSaveRequest = serde_json::from_value(serde_json::json!({
        "draft_id": Uuid::new_v4().to_string(),
        "revision": 1,
        "proposals": [{
            "date": " ",
            "voucher_type": null,
            "narration": " ",
            "notes": "",
            "entries": [{ "ledger": " ", "side": null, "amount": " " }]
        }]
    }))
    .unwrap();
    assert_eq!(whitespace_request.proposals[0].date.as_deref(), Some(" "));
    assert_eq!(
        whitespace_request.proposals[0].narration.as_deref(),
        Some(" ")
    );
    assert_eq!(
        whitespace_request.proposals[0].entries[0].ledger.as_deref(),
        Some(" ")
    );
    assert_eq!(
        whitespace_request.proposals[0].entries[0].amount.as_deref(),
        Some(" ")
    );

    let source_empty = parse_source_xml(
        br#"<ENVELOPE><BODY><IMPORTDATA><REQUESTDATA><TALLYMESSAGE><VOUCHER REMOTEID="y" VCHTYPE="Receipt"><DATE>20260901</DATE><NARRATION/><ALLLEDGERENTRIES.LIST><LEDGERNAME>Cash</LEDGERNAME><AMOUNT>1</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></TALLYMESSAGE></REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>"#,
        "source.xml".into(),
    )
    .unwrap();
    assert_eq!(source_empty.vouchers[0].narration.as_deref(), Some(""));
}

#[test]
fn saved_schema_refuses_unknown_root_and_proposal_fields_from_valid_saved_drafts() {
    let source = source();
    let proposals = empty_proposals(&source);
    let saved: serde_json::Value =
        serde_json::from_slice(&serialize_draft(&source, &proposals).unwrap()).unwrap();

    let mut root_unknown = saved.clone();
    root_unknown["unexpected"] = serde_json::Value::Bool(true);
    assert_eq!(
        open_saved_draft(serde_json::to_vec(&root_unknown).unwrap())
            .unwrap_err()
            .code,
        "source_draft_saved_json_invalid"
    );

    let mut proposal_unknown = saved;
    proposal_unknown["proposals"][0]["unexpected"] = serde_json::Value::Bool(true);
    assert_eq!(
        open_saved_draft(serde_json::to_vec(&proposal_unknown).unwrap())
            .unwrap_err()
            .code,
        "source_draft_saved_json_invalid"
    );
    assert_eq!(
        validate_proposals(&source, &[]).unwrap_err().code,
        "source_draft_proposal_shape_invalid"
    );
}

#[test]
fn saved_draft_reauthenticates_source_hash_and_preserves_unresolved_proposals() {
    let source = source();
    let proposals = empty_proposals(&source);
    let mut saved: serde_json::Value =
        serde_json::from_slice(&serialize_draft(&source, &proposals).unwrap()).unwrap();
    assert!(open_saved_draft(serde_json::to_vec(&saved).unwrap()).is_ok());
    saved["source_sha256"] = serde_json::Value::String("0".repeat(64));
    assert_eq!(
        open_saved_draft(serde_json::to_vec(&saved).unwrap())
            .unwrap_err()
            .code,
        "source_draft_saved_source_hash_mismatch"
    );
}

#[test]
fn final_save_admission_refuses_stale_identity_or_revision_without_mutating_file_or_store() {
    let store = SourceDraftStore::default();
    let source_data = source();
    let active = ActiveDraft {
        id: Uuid::new_v4(),
        revision: 1,
        proposals: empty_proposals(&source_data),
        catalog_generation: 0,
        catalog: None,
        source: source_data,
    };
    let dto = store.replace(active).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let sentinel = directory.path().join("draft.bridge-draft.json");
    fs::write(&sentinel, b"unchanged").unwrap();
    let expected_proposals: Vec<SourceDraftProposal> =
        dto.rows.iter().map(|row| row.proposal.clone()).collect();
    let mut rejected_proposals = expected_proposals.clone();
    rejected_proposals[0].notes = "must not persist".into();

    for request in [
        SourceDraftSaveRequest {
            draft_id: Uuid::new_v4().to_string(),
            revision: dto.revision,
            proposals: rejected_proposals.clone(),
        },
        SourceDraftSaveRequest {
            draft_id: dto.draft_id.clone(),
            revision: dto.revision - 1,
            proposals: rejected_proposals.clone(),
        },
    ] {
        assert_eq!(
            commit_after_persist(&store.active, request, sentinel.clone())
                .unwrap_err()
                .code,
            "source_draft_revision_conflict"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
        let active = store.active.lock().unwrap().clone().unwrap();
        assert_eq!(active.id.to_string(), dto.draft_id);
        assert_eq!(active.revision, dto.revision);
        assert_eq!(
            serde_json::to_vec(&active.proposals).unwrap(),
            serde_json::to_vec(&expected_proposals).unwrap()
        );
    }

    let invalid = SourceDraftProposal {
        date: Some("20269999".into()),
        voucher_type: None,
        narration: None,
        notes: String::new(),
        entries: vec![SourceDraftEntryProposal {
            ledger: None,
            side: None,
            amount: None,
        }],
    };
    assert_eq!(
        validate_proposals(&source(), &[invalid]).unwrap_err().code,
        "source_draft_proposal_date_invalid"
    );
}

#[test]
fn save_destination_rejects_xml_and_hardlink_alias_without_replacing_source_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let source_path = directory.path().join("original.xml");
    let alias_path = directory.path().join("alias.bridge-draft.json");
    fs::write(&source_path, b"original").unwrap();
    fs::hard_link(&source_path, &alias_path).unwrap();

    assert_eq!(
        files::validate_save_destination(&source_path)
            .unwrap_err()
            .code,
        "source_draft_extension_invalid"
    );
    assert_eq!(
        write_private_file(&alias_path, b"replacement")
            .unwrap_err()
            .code,
        "source_draft_destination_unavailable"
    );
    assert_eq!(fs::read(&source_path).unwrap(), b"original");
    assert_eq!(fs::read(&alias_path).unwrap(), b"original");
}

#[test]
fn regular_draft_overwrite_is_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("draft.bridge-draft.json");
    fs::write(&path, b"old").unwrap();
    write_private_file(&path, b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
}
