use super::*;

fn source_rows() -> Vec<ReadVoucher> {
    parse_import_vouchers(&boundary_tests::captured_vouchers())
        .unwrap()
        .rows
}

#[test]
fn import_source_rejects_repeated_identity_independently_of_tags_or_other_identity_fields() {
    let rows = source_rows();
    let mut first = rows[0].clone();
    first.narration = Some("[BRIDGE:current]".into());
    for identical in [false, true] {
        let mut repeated = first.clone();
        if !identical {
            repeated.narration = None;
            repeated.master_id = Some("999".into());
        }
        repeated.guid = repeated.guid.map(|id| id.to_ascii_uppercase());
        assert_eq!(
            ImportReadSource::admit(vec![first.clone(), repeated]),
            Err("import_verification_identity_invalid".into())
        );
    }
    let mut same_master = rows[1].clone();
    same_master.master_id = Some(format!(" 000{} ", first.master_id.as_deref().unwrap()));
    assert_eq!(
        ImportReadSource::admit(vec![first.clone(), same_master]),
        Err("import_verification_identity_invalid".into())
    );
    let mut master_only = first.clone();
    master_only.guid = None;
    assert!(ImportReadSource::admit(vec![master_only.clone()]).is_ok());
    assert_eq!(
        ImportReadSource::admit(vec![master_only.clone(), master_only.clone()]),
        Err("import_verification_identity_invalid".into())
    );
    master_only.master_id = None;
    assert_eq!(
        ImportReadSource::admit(vec![master_only]),
        Err("import_verification_identity_invalid".into())
    );
    first.master_id = Some("not-an-id".into());
    assert_eq!(
        ImportReadSource::admit(vec![first]),
        Err("import_verification_master_id_invalid".into())
    );
}

#[test]
fn import_source_admits_only_one_well_formed_bridge_marker_without_batch_scope() {
    let template = source_rows().remove(0);
    for narration in [
        "[BRIDGE:current] [BRIDGE:other-batch]",
        "[BRIDGE:current] [BRIDGE:current]",
        "[BRIDGE:unrelated-a] [BRIDGE:unrelated-b]",
    ] {
        let mut row = template.clone();
        row.narration = Some(narration.into());
        assert_eq!(
            ImportReadSource::admit(vec![row]),
            Err("import_verification_tag_ambiguous".into())
        );
    }
    for narration in [
        "[BRIDGE:]",
        "[BRIDGE:open",
        "[BRIDGE bad]",
        "[BRIDGE:bad id]",
    ] {
        let mut row = template.clone();
        row.narration = Some(narration.into());
        assert_eq!(
            ImportReadSource::admit(vec![row]),
            Err("import_verification_tag_invalid".into())
        );
    }
    for narration in ["ordinary narration", "[BRIDGE:another-batch]"] {
        let mut row = template.clone();
        row.narration = Some(narration.into());
        assert!(ImportReadSource::admit(vec![row]).is_ok());
    }
}

#[test]
fn captured_import_sources_reject_failed_exports_duplicate_fields_and_invalid_scalars() {
    let captured = boundary_tests::captured_vouchers();
    // Mutate captured bytes only to test refusal; these are not new fixtures.
    for (needle, replacement, code) in [
        (
            "<STATUS>1</STATUS>",
            "<STATUS>0</STATUS>",
            "import_verification_protocol_invalid",
        ),
        (
            "> 1</ALTERID>",
            ">invalid</ALTERID>",
            "voucher_alter_id_invalid",
        ),
        ("> 1</ALTERID>", "></ALTERID>", "voucher_alter_id_invalid"),
        (
            "> 1</MASTERID>",
            ">invalid</MASTERID>",
            "import_verification_master_id_invalid",
        ),
        (
            "> 1</MASTERID>",
            "></MASTERID>",
            "import_verification_master_id_invalid",
        ),
        (
            "</ALTERID>",
            "</ALTERID><ALTERID>2</ALTERID>",
            "import_verification_export_invalid",
        ),
        (
            "</MASTERID>",
            "</MASTERID><MASTERID>2</MASTERID>",
            "import_verification_export_invalid",
        ),
        (
            "</AMOUNT>",
            "</AMOUNT><AMOUNT>1</AMOUNT>",
            "import_verification_export_invalid",
        ),
        (
            ">-101.01</AMOUNT>",
            ">101.01</AMOUNT>",
            "voucher_entry_polarity_mismatch",
        ),
        (
            "REMOTEID=\"",
            "REMOTEID=\"duplicate\" REMOTEID=\"",
            "import_verification_export_invalid",
        ),
    ] {
        let invalid = captured.replace(needle, replacement);
        assert_ne!(invalid, captured, "fault must reach {needle}");
        assert_eq!(
            parse_import_vouchers(&invalid),
            Err(code.into()),
            "{needle}"
        );
    }
    let rows = source_rows();
    let duplicate_guid = captured.replace(
        rows[1].guid.as_deref().unwrap(),
        rows[0].guid.as_deref().unwrap(),
    );
    assert_eq!(
        parse_import_vouchers(&duplicate_guid),
        Err("import_verification_identity_invalid".into())
    );
    let multi_tag = captured.replace(
        rows[0].narration.as_deref().unwrap(),
        "[BRIDGE:current] &#91;BRIDGE:other-batch&#93;",
    );
    assert_eq!(
        parse_import_vouchers(&multi_tag),
        Err("import_verification_tag_ambiguous".into())
    );
}

#[test]
fn captured_import_scalar_content_is_preserved_or_refused_without_silent_loss() {
    let captured = boundary_tests::captured_vouchers();
    let baseline = parse_import_vouchers(&captured).unwrap();
    let valid = captured.replace(">-101.01</AMOUNT>", ">-101.<![CDATA[01]]></AMOUNT>");
    assert_ne!(valid, captured);
    assert_eq!(parse_import_vouchers(&valid).unwrap(), baseline);
    for content in ["-101.01<EXTRA/>", "-101.01<EXTRA>9</EXTRA>"] {
        let malformed = captured.replace(">-101.01</AMOUNT>", &format!(">{content}</AMOUNT>"));
        assert_eq!(
            parse_import_vouchers(&malformed),
            Err("import_verification_export_invalid".into())
        );
    }
    let invalid = captured.replace(">-101.01</AMOUNT>", ">-101.01<![CDATA[invalid]]></AMOUNT>");
    assert_eq!(
        parse_import_vouchers(&invalid),
        Err("import_verification_amount_invalid".into())
    );
    let narration = baseline.rows[0].narration.as_deref().unwrap();
    let literal = captured.replace(narration, "<![CDATA[literal &amp; text]]>");
    assert_eq!(
        parse_import_vouchers(&literal).unwrap().rows[0]
            .narration
            .as_deref(),
        Some("literal &amp; text")
    );
    let multi_tag = captured.replace(narration, "[BRIDGE:current]<![CDATA[ [BRIDGE:other]]]>");
    assert_eq!(
        parse_import_vouchers(&multi_tag),
        Err("import_verification_tag_ambiguous".into())
    );
}

#[test]
fn import_readback_refuses_unexpected_collection_rows_through_shared_parser() {
    let captured = boundary_tests::captured_vouchers();
    for row in ["<LEDGER/>", "<GROUP/>", "<UNEXPECTED></UNEXPECTED>"] {
        let mismatched = captured.replace("</COLLECTION>", &format!("{row}</COLLECTION>"));
        assert_ne!(mismatched, captured);
        assert_eq!(
            parse_import_vouchers(&mismatched),
            Err("import_verification_export_invalid".into())
        );
    }
}
