//! Safety-property tests for `outstandings_shared`: the company-identity and
//! book-extent read shared by the native (always compiled) and voucher-scan
//! (feature-gated) outstandings paths.
//!
//! These moved out of `tests/outstandings.rs` -- which is gated behind
//! `voucher-scan` because it otherwise exercises only scan machinery -- so
//! that `parse_company_book_extent`'s identity-verification properties stay
//! covered in the default build too, where the native path is what actually
//! calls it.

use bridge_tally_protocol::outstandings_shared::{parse_company_book_extent, OutstandingsError};

const COMPANY_EXTENT: &str = include_str!("fixtures/unit_a_company_extent_live.xml");
const COMPANY_NAME: &str = "Aarav Trading Company Demo";
const COMPANY_GUID: &str = "bb8ad19e-6aef-4239-a917-87fec0c6215e";

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
