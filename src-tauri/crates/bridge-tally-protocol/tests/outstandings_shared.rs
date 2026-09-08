//! Safety-property tests for `outstandings_shared`: the company-identity and
//! book-extent read shared by the native (always compiled) and voucher-scan
//! (feature-gated) outstandings paths.
//!
//! These moved out of `tests/outstandings.rs` -- which is gated behind
//! `voucher-scan` because it otherwise exercises only scan machinery -- so
//! that `parse_company_book_extent`'s identity-verification properties stay
//! covered in the default build too, where the native path is what actually
//! calls it.

use bridge_tally_protocol::outstandings_shared::{
    parse_company_book_extent, parse_company_book_extent_v2, CompanyBookExtentExpectation,
    OutstandingsError,
};

const COMPANY_EXTENT: &str = include_str!("fixtures/unit_a_company_extent_live.xml");
const COMPANY_NAME: &str = "Aarav Trading Company Demo";
const COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";
const COMPANY_EXTENT_V2: &str =
    include_str!("fixtures/agent/native-company-book-extents-with-number.utf8.xml");
const SPLIT_COMPANY_NAME: &str = "BRIDGE PROBE B SANDBOX";
const SPLIT_COMPANY_GUID: &str = "ec4454ae-5c4c-4bfa-b3b0-68182a749689";
const SPLIT_COMPANY_NUMBER: &str = "100005";
const SPLIT_BOOKS_FROM: &str = "20250401";

/// Real capture (`tests/fixtures/native/company_extent_9000.xml`) that
/// includes `ALTMSTID` for every company row, unlike `COMPANY_EXTENT` above.
/// `docs/tally/TEST_CORPUS.md` records this exact value (327) for
/// "Aarav Trading Company Demo".
const COMPANY_EXTENT_WITH_ALTMSTID: &str = include_str!("fixtures/native/company_extent_9000.xml");
const ALTMSTID_COMPANY_ALTMSTID: &str = "327";

fn extent() -> bridge_tally_protocol::outstandings_shared::CompanyBookExtent {
    parse_company_book_extent(COMPANY_EXTENT, COMPANY_NAME, COMPANY_GUID)
        .expect("real company extent capture parses")
}

fn split_expectation() -> CompanyBookExtentExpectation {
    CompanyBookExtentExpectation::new(
        SPLIT_COMPANY_NAME.to_string(),
        SPLIT_COMPANY_GUID.to_string(),
        SPLIT_COMPANY_NUMBER.to_string(),
        SPLIT_BOOKS_FROM.to_string(),
    )
    .expect("captured full tuple is a valid expectation")
}

fn company_row<'a>(xml: &'a str, name: &str) -> &'a str {
    let start = xml
        .find(&format!(r#"<COMPANY NAME="{name}""#))
        .expect("captured company row exists");
    let end = start
        + xml[start..]
            .find("</COMPANY>")
            .expect("captured company row closes")
        + "</COMPANY>".len();
    &xml[start..end]
}

#[test]
fn v2_captured_extent_selects_the_exact_tuple_among_same_guid_siblings() {
    let extent = parse_company_book_extent_v2(COMPANY_EXTENT_V2, &split_expectation())
        .expect("captured V2 full tuple selects the intended split book");
    assert_eq!(extent.company().name(), SPLIT_COMPANY_NAME);
    assert_eq!(extent.company().guid(), SPLIT_COMPANY_GUID);
    assert_eq!(extent.books_from().as_str(), SPLIT_BOOKS_FROM);
    assert_eq!(COMPANY_EXTENT_V2.matches("<COMPANYNUMBER").count(), 16);
    assert_eq!(COMPANY_EXTENT_V2.matches(SPLIT_COMPANY_GUID).count(), 2);
}

#[test]
fn v2_canonicalizes_a_captured_guid_case_variant_for_extent_brackets() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    let upper_guid = SPLIT_COMPANY_GUID.to_ascii_uppercase();
    let changed_target = target.replacen(SPLIT_COMPANY_GUID, &upper_guid, 1);
    assert_ne!(
        changed_target, target,
        "captured GUID case mutation must apply"
    );
    let changed = COMPANY_EXTENT_V2.replacen(target, &changed_target, 1);
    assert_ne!(
        changed, COMPANY_EXTENT_V2,
        "captured response mutation must apply"
    );

    let extent = parse_company_book_extent_v2(&changed, &split_expectation())
        .expect("GUID casing alone must not alter the selected extent");
    assert_eq!(extent.company().guid(), SPLIT_COMPANY_GUID);
}

#[test]
fn v2_full_tuple_expectation_refuses_noncanonical_caller_numbers() {
    for company_number in [
        "",
        " 100005",
        "100005 ",
        "100005\n",
        "1.0",
        "१२३",
        "12345678901234567",
    ] {
        assert!(
            CompanyBookExtentExpectation::new(
                SPLIT_COMPANY_NAME.to_string(),
                SPLIT_COMPANY_GUID.to_string(),
                company_number.to_string(),
                SPLIT_BOOKS_FROM.to_string(),
            )
            .is_err(),
            "{company_number:?}"
        );
    }
}

#[test]
fn v2_trims_the_wire_number_without_integer_coercion() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    let changed_target = target.replacen(" 100005</COMPANYNUMBER>", " 000001 </COMPANYNUMBER>", 1);
    assert_ne!(changed_target, target);
    let changed = COMPANY_EXTENT_V2.replacen(target, &changed_target, 1);
    let expectation = CompanyBookExtentExpectation::new(
        SPLIT_COMPANY_NAME.to_string(),
        SPLIT_COMPANY_GUID.to_string(),
        "000001".to_string(),
        SPLIT_BOOKS_FROM.to_string(),
    )
    .expect("canonical leading-zero number remains a string");

    assert!(parse_company_book_extent_v2(&changed, &expectation).is_ok());
}

#[test]
fn v2_refuses_missing_or_malformed_observed_company_number_without_fallback() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    for replacement in ["", r#"<COMPANYNUMBER TYPE="Number"> abc</COMPANYNUMBER>"#] {
        let changed_target = target.replacen(
            r#"<COMPANYNUMBER TYPE="Number"> 100005</COMPANYNUMBER>"#,
            replacement,
            1,
        );
        assert_ne!(changed_target, target);
        let changed = COMPANY_EXTENT_V2.replacen(target, &changed_target, 1);
        assert_eq!(
            parse_company_book_extent_v2(&changed, &split_expectation()),
            Err(OutstandingsError::InvalidResponse(
                if replacement.is_empty() {
                    "company_number_missing"
                } else {
                    "company_number_invalid"
                }
            ))
        );
    }
}

#[test]
fn v2_refuses_missing_or_malformed_observed_books_from_without_fallback() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    for replacement in ["", r#"<BOOKSFROM TYPE="Date">not-a-date</BOOKSFROM>"#] {
        let changed_target = target.replacen(
            r#"<BOOKSFROM TYPE="Date">20250401</BOOKSFROM>"#,
            replacement,
            1,
        );
        assert_ne!(changed_target, target);
        let changed = COMPANY_EXTENT_V2.replacen(target, &changed_target, 1);
        assert!(
            parse_company_book_extent_v2(&changed, &split_expectation()).is_err(),
            "{replacement:?} must not select another same-GUID row"
        );
    }
}

#[test]
fn v2_refuses_repeated_observed_company_number_field() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    let repeated = target.replacen(
        r#"<COMPANYNUMBER TYPE="Number"> 100005</COMPANYNUMBER>"#,
        r#"<COMPANYNUMBER TYPE="Number"> 100005</COMPANYNUMBER><COMPANYNUMBER TYPE="Number"> 100005</COMPANYNUMBER>"#,
        1,
    );
    assert_ne!(repeated, target);
    let repeated = COMPANY_EXTENT_V2.replacen(target, &repeated, 1);
    assert!(parse_company_book_extent_v2(&repeated, &split_expectation()).is_err());
}

#[test]
fn v2_refuses_duplicate_exact_tuple_and_presentation_collision() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    let duplicate =
        COMPANY_EXTENT_V2.replacen("</COLLECTION>", &format!("{target}</COLLECTION>"), 1);
    assert_eq!(
        parse_company_book_extent_v2(&duplicate, &split_expectation()),
        Err(OutstandingsError::InvalidResponse(
            "company_identity_ambiguous"
        ))
    );

    let collision = target
        .replace(SPLIT_COMPANY_NAME, "bridge probe b sandbox")
        .replacen(" 100005</COMPANYNUMBER>", " 100099</COMPANYNUMBER>", 1);
    assert_ne!(collision, target);
    let collision =
        COMPANY_EXTENT_V2.replacen("</COLLECTION>", &format!("{collision}</COLLECTION>"), 1);
    assert_eq!(
        parse_company_book_extent_v2(&collision, &split_expectation()),
        Err(OutstandingsError::InvalidResponse(
            "company_identity_presentation_collision"
        ))
    );
}

#[test]
fn v2_refuses_same_guid_selector_attribute_with_a_different_nested_name() {
    let target = company_row(COMPANY_EXTENT_V2, SPLIT_COMPANY_NAME);
    let sibling = target.replacen(
        &format!(r#"<NAME TYPE="String">{SPLIT_COMPANY_NAME}</NAME>"#),
        r#"<NAME TYPE="String">BRIDGE PROBE B OTHER BOOK</NAME>"#,
        1,
    );
    assert_ne!(sibling, target, "captured nested-name mutation must apply");
    let collision =
        COMPANY_EXTENT_V2.replacen("</COLLECTION>", &format!("{sibling}</COLLECTION>"), 1);
    assert_ne!(
        collision, COMPANY_EXTENT_V2,
        "captured sibling insertion must apply"
    );

    assert_eq!(
        parse_company_book_extent_v2(&collision, &split_expectation()),
        Err(OutstandingsError::InvalidResponse(
            "company_identity_presentation_collision"
        ))
    );
}

#[test]
fn company_pin_is_created_only_after_live_identity_matches() {
    let extent = extent();
    assert_eq!(extent.company().name(), COMPANY_NAME);
    assert_eq!(extent.company().guid(), COMPANY_GUID);
    assert!(matches!(
        parse_company_book_extent(COMPANY_EXTENT, COMPANY_NAME, "wrong-guid"),
        Err(OutstandingsError::CompanyIdentityMismatch)
    ));
}

#[test]
fn company_extent_selects_the_expected_guid_in_a_multi_company_collection() {
    let company_start = COMPANY_EXTENT
        .find("    <COMPANY ")
        .expect("real capture contains a company row");
    let company_end = COMPANY_EXTENT[company_start..]
        .find("    </COMPANY>")
        .map(|offset| company_start + offset + "    </COMPANY>".len())
        .expect("real capture company row is complete");
    let expected_row = &COMPANY_EXTENT[company_start..company_end];
    let unrelated_row = expected_row
        .replace(COMPANY_NAME, "Earlier Loaded Synthetic Company")
        .replace(COMPANY_GUID, "00000000-0000-4000-8000-000000000001");
    let response =
        COMPANY_EXTENT.replacen(expected_row, &format!("{unrelated_row}\n{expected_row}"), 1);

    let selected = parse_company_book_extent(&response, COMPANY_NAME, COMPANY_GUID)
        .expect("GUID selection is independent of collection order");
    assert_eq!(selected.company().name(), COMPANY_NAME);
    assert_eq!(selected.company().guid(), COMPANY_GUID);
}

#[test]
fn company_extent_rejects_duplicate_rows_for_the_expected_guid() {
    let company_start = COMPANY_EXTENT
        .find("    <COMPANY ")
        .expect("real capture contains a company row");
    let company_end = COMPANY_EXTENT[company_start..]
        .find("    </COMPANY>")
        .map(|offset| company_start + offset + "    </COMPANY>".len())
        .expect("real capture company row is complete");
    let expected_row = &COMPANY_EXTENT[company_start..company_end];
    let response =
        COMPANY_EXTENT.replacen(expected_row, &format!("{expected_row}\n{expected_row}"), 1);

    assert_eq!(
        parse_company_book_extent(&response, COMPANY_NAME, COMPANY_GUID),
        Err(OutstandingsError::InvalidResponse(
            "company_identity_ambiguous"
        ))
    );
}

/// The core-window bracket in `connector.rs` compares two `CompanyBookExtent`
/// reads by `PartialEq` and rejects the window when they differ
/// (`closing_extent != opening_extent`). `ALTMSTID` -- Tally's MASTER
/// alteration high-water mark -- has to move that comparison when a GROUP or
/// LEDGER master is edited mid-window, because nothing else in the struct
/// does. `docs/tally/TEST_CORPUS.md` records a real pair of extents that
/// agreed on GUID, `BooksFrom`, `LastVoucherDate` and `ALTVCHID` (252) while
/// differing only on `ALTMSTID` (218 vs 219), concealing roughly Rs 15 lakh
/// of outstandings from a voucher-only bracket.
#[test]
fn captured_extent_with_altmstid_parses_with_the_master_high_water_populated() {
    let extent =
        parse_company_book_extent(COMPANY_EXTENT_WITH_ALTMSTID, COMPANY_NAME, COMPANY_GUID)
            .expect("real company_extent_9000.xml capture parses");
    assert_eq!(
        extent.master_alter_id_high_water().map(|w| w.get()),
        Some(
            ALTMSTID_COMPANY_ALTMSTID
                .parse::<u64>()
                .expect("test constant is a valid u64")
        )
    );
}

/// Two otherwise-identical extents that differ ONLY in `ALTMSTID` must
/// compare as not-equal -- this is the entire point of wiring the field into
/// `CompanyBookExtent`'s derived `PartialEq`: it is what lets the existing
/// paired-extent bracket reject a core window torn by a mid-window master
/// edit, without any change to the bracket's control flow.
#[test]
fn extents_differing_only_in_master_high_water_are_not_equal() {
    let opening =
        parse_company_book_extent(COMPANY_EXTENT_WITH_ALTMSTID, COMPANY_NAME, COMPANY_GUID)
            .expect("real company_extent_9000.xml capture parses");
    let edited_master_response = COMPANY_EXTENT_WITH_ALTMSTID.replacen(
        &format!(r#"<ALTMSTID TYPE="Number"> {ALTMSTID_COMPANY_ALTMSTID}</ALTMSTID>"#),
        r#"<ALTMSTID TYPE="Number"> 328</ALTMSTID>"#,
        1,
    );
    assert_ne!(
        edited_master_response, COMPANY_EXTENT_WITH_ALTMSTID,
        "the replacement must actually change the response for this test to prove anything"
    );
    let closing = parse_company_book_extent(&edited_master_response, COMPANY_NAME, COMPANY_GUID)
        .expect("edited capture still parses");

    assert_eq!(
        opening.voucher_alter_id_high_water(),
        closing.voucher_alter_id_high_water()
    );
    assert_eq!(opening.books_from(), closing.books_from());
    assert_eq!(opening.last_voucher_date(), closing.last_voucher_date());
    assert_ne!(
        opening.master_alter_id_high_water(),
        closing.master_alter_id_high_water()
    );
    assert_ne!(
        opening, closing,
        "a master-only edit must be visible to the whole-struct PartialEq the bracket compares"
    );
}

/// A response that omits `ALTMSTID` entirely (as `COMPANY_EXTENT`, captured
/// before this field was fetched, does) must still parse, with the field
/// `None` -- absence is not a hard failure, matching `ALTVCHID`'s existing
/// optional-field pattern.
#[test]
fn extent_response_omitting_altmstid_still_parses_with_it_none() {
    let extent = parse_company_book_extent(COMPANY_EXTENT, COMPANY_NAME, COMPANY_GUID)
        .expect("real company extent capture parses");
    assert!(!COMPANY_EXTENT.contains("ALTMSTID"));
    assert_eq!(extent.master_alter_id_high_water(), None);
}
