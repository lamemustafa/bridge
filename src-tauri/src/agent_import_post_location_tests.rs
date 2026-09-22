//! bridge#574: the aim check before a native post and where it landed after.
use super::*;

const TARGET: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";
const THIRD: &str = "33333333-3333-4333-8333-333333333333";

fn marks(name: &str, guid: &str, vouchers: u64) -> LoadedCompanyMarks {
    LoadedCompanyMarks {
        name: name.into(),
        guid: guid.into(),
        vouchers,
        masters: 7,
    }
}

fn book() -> Vec<LoadedCompanyMarks> {
    vec![
        marks("Synthetic Target", TARGET, 10),
        marks("Synthetic Other", OTHER, 20),
        marks("Synthetic Third", THIRD, 30),
    ]
}

fn with_vouchers(
    rows: &[LoadedCompanyMarks],
    guid: &str,
    vouchers: u64,
) -> Vec<LoadedCompanyMarks> {
    rows.iter()
        .cloned()
        .map(|mut row| {
            if row.guid == guid {
                row.vouchers = vouchers;
            }
            row
        })
        .collect()
}

fn state(before: &[LoadedCompanyMarks], after: &[LoadedCompanyMarks]) -> Value {
    classify_post_location(before, Some(after), TARGET, "Synthetic Target")
}

#[test]
fn every_loaded_company_is_parsed_from_the_captured_collection() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).unwrap();
    let rows = parse_all_company_marks(&xml).unwrap();
    assert_eq!(rows.len(), xml.matches("<COMPANY NAME=").count());
    assert!(rows.len() > 1);
    assert_eq!(
        rows[0],
        LoadedCompanyMarks {
            name: "Aarav Trading Company Demo".into(),
            guid: "bb8ad19e-6aef-4239-a917-87fec0c6215e".into(),
            vouchers: 101605,
            masters: 328,
        }
    );
    // A row without its name cannot be matched, so it refuses rather than
    // being skipped as if that company had not moved.
    let nameless = xml.replacen(
        "<COMPANY NAME=\"Aarav Trading Company Demo\"",
        "<COMPANY",
        1,
    );
    assert_ne!(nameless, xml);
    assert_eq!(
        parse_all_company_marks(&nameless),
        Err("company_marks_name_absent".into())
    );
}

#[test]
fn the_aim_check_admits_one_target_and_refuses_a_rename_or_a_namesake() {
    assert_eq!(
        admit_post_target(&book(), TARGET, "Synthetic Target"),
        Ok(())
    );
    // Case and surrounding space do not matter, as in require_unique_company_scope.
    assert_eq!(
        admit_post_target(&book(), TARGET, " synthetic TARGET "),
        Ok(())
    );
    // The target renamed since admission.
    let mut renamed = book();
    renamed[0].name = "Synthetic Target Renamed".into();
    assert_eq!(
        admit_post_target(&renamed, TARGET, "Synthetic Target"),
        Err("post_company_scope_changed")
    );
    // Another company renamed to the target's name: the post could land in it.
    let mut namesake = book();
    namesake[1].name = "SYNTHETIC TARGET".into();
    assert_eq!(
        admit_post_target(&namesake, TARGET, "Synthetic Target"),
        Err("post_company_scope_changed")
    );
    // The target unloaded.
    assert_eq!(
        admit_post_target(&book()[1..], TARGET, "Synthetic Target"),
        Err("post_company_scope_changed")
    );
    // A year-split child shares the GUID under another name; it is not a namesake.
    let mut split = book();
    split.push(marks("Synthetic Target - (from 1-Apr-26)", TARGET, 3));
    assert_eq!(
        admit_post_target(&split, TARGET, "Synthetic Target"),
        Ok(())
    );
}

#[test]
fn only_the_target_moving_is_the_expected_landing() {
    let after = with_vouchers(&book(), TARGET, 11);
    let located = state(&book(), &after);
    assert_eq!(located["state"], "target_only", "{located}");
    assert_eq!(located["target_moved"], true);
    assert_eq!(located["other_companies_moved"], json!([]));
}

#[test]
fn another_company_moving_instead_of_the_target_is_named() {
    let after = with_vouchers(&book(), OTHER, 21);
    let located = state(&book(), &after);
    assert_eq!(located["state"], "suspected_other_company", "{located}");
    assert_eq!(located["target_moved"], false);
    assert_eq!(
        located["other_companies_moved"],
        json!([{"name":"Synthetic Other","guid":OTHER,"voucher_mark_before":20,"voucher_mark_after":21}])
    );
}

#[test]
fn concurrent_writers_are_reported_and_never_read_as_not_posted() {
    let two_others = with_vouchers(&with_vouchers(&book(), OTHER, 21), THIRD, 31);
    assert_eq!(
        state(&book(), &two_others)["state"],
        "ambiguous_concurrent_changes"
    );
    let target_and_other = with_vouchers(&with_vouchers(&book(), TARGET, 12), OTHER, 21);
    assert_eq!(
        state(&book(), &target_and_other)["state"],
        "target_and_others_moved"
    );
    assert_eq!(state(&book(), &book())["state"], "not_observed");
}

#[test]
fn a_changed_company_set_is_never_a_clean_landing() {
    // The target itself gone after the POST: renamed or unloaded.
    let mut target_renamed = with_vouchers(&book(), TARGET, 11);
    target_renamed[0].name = "Synthetic Target Renamed".into();
    assert_eq!(
        state(&book(), &target_renamed)["state"],
        "target_scope_changed"
    );
    // Another company unloaded while the target moved: the target moved, but
    // the vanished company cannot be ruled out as a second landing.
    let mut unloaded = with_vouchers(&book(), TARGET, 11);
    unloaded.remove(2);
    let located = state(&book(), &unloaded);
    assert_eq!(located["state"], "target_moved_scope_changed", "{located}");
    assert_eq!(
        located["companies_removed"],
        json!([{"name":"Synthetic Third","guid":THIRD}])
    );
    // A company appeared and the target did not move.
    let mut appeared = book();
    appeared.push(marks(
        "Synthetic Fourth",
        "44444444-4444-4444-8444-444444444444",
        1,
    ));
    assert_eq!(
        state(&book(), &appeared)["state"],
        "location_ambiguous_scope_changed"
    );
}

#[test]
fn an_unreadable_after_snapshot_is_said_not_guessed() {
    assert_eq!(
        classify_post_location(&book(), None, TARGET, "Synthetic Target")["state"],
        "after_snapshot_unavailable"
    );
}
