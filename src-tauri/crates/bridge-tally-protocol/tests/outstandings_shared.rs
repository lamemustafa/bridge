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
