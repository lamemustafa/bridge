use super::*;
#[test]
fn import_name_scope_must_select_one_observed_company() {
    // Local identity-admission test. These are not claimed Tally responses.
    let original = bridge_tally_protocol::TallyCompany {
        name: "Synthetic Book".into(),
        guid: Some("00000000-0000-4000-8000-000000000001".into()),
        company_number: Some("1".into()),
        books_from: Some("20260401".into()),
    };
    let identity = super::super::VerifiedCompanyIdentity::from_observed_companies(
        original.name.clone(),
        original.guid.clone().unwrap(),
        "1".into(),
        "20260401".into(),
        std::slice::from_ref(&original),
    )
    .unwrap();
    assert!(require_unique_company_scope(std::slice::from_ref(&original), &identity).is_ok());
    for name in ["Synthetic Book", " synthetic book "] {
        let mut other = original.clone();
        other.name = name.into();
        other.guid = Some("00000000-0000-4000-8000-000000000002".into());
        assert_eq!(
            require_unique_company_scope(&[original.clone(), other], &identity),
            Err(AmbiguousImportCompany)
        );
    }
}

// #687: the child's own answer, which the parent's tests replace with a script.

const NONCE: &str = "00000000-0000-4000-8000-000000000687";

/// What the child wrote, and what it returned, for `input` and `dialog`.
fn child_answer(dialog: fn(&str) -> bool, input: &str) -> (bool, Vec<u8>) {
    let mut output = Vec::new();
    let approved = answer_with_token_over(POST_TOKEN_PREFIX, dialog, input.as_bytes(), &mut output);
    (approved, output)
}

#[test]
fn a_declined_dialog_writes_no_token() {
    let (approved, output) = child_answer(|_| false, &format!("{NONCE}\nPost one voucher"));
    assert!(!approved);
    assert!(output.is_empty(), "{output:?}");
}

#[test]
fn an_approved_dialog_writes_exactly_the_token_for_its_nonce() {
    let (approved, output) = child_answer(|_| true, &format!("{NONCE}\nPost one voucher"));
    assert!(approved);
    assert_eq!(
        output,
        format!("bridge-post-approved:{NONCE}\n").into_bytes()
    );
}

#[test]
fn the_dialog_shows_the_preview_after_the_nonce_line() {
    let (approved, _) = child_answer(
        |preview| preview == "Post one voucher\nsecond line",
        &format!("{NONCE}\nPost one voucher\nsecond line"),
    );
    assert!(approved);
}

#[test]
fn an_input_not_in_the_parents_shape_shows_no_dialog_and_writes_nothing() {
    let oversized = "x".repeat(MAX_PREVIEW_BYTES + 1);
    for input in [
        String::new(),
        NONCE.to_string(),
        format!("{NONCE}\n"),
        format!("not-a-uuid\nPost one voucher"),
        format!("{NONCE}\nPost\0one voucher"),
        format!("{NONCE}\n{oversized}"),
    ] {
        let (approved, output) = child_answer(
            |_| panic!("no dialog is shown for input the parent never sends"),
            &input,
        );
        assert!(!approved, "{input:?}");
        assert!(output.is_empty(), "{input:?}");
    }
}
