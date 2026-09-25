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
    classify_post_location(before, Some(after), TARGET, "Synthetic Target", Some(1))
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
    // The target's name under another GUID, and no other company of that name:
    // a different company, however the name reads (a restore issues new GUIDs).
    let mut regenerated = book();
    regenerated[0].guid = "44444444-4444-4444-8444-444444444444".into();
    assert_eq!(
        admit_post_target(&regenerated, TARGET, "Synthetic Target"),
        Err("post_company_scope_changed")
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
        classify_post_location(&book(), None, TARGET, "Synthetic Target", Some(1))["state"],
        "after_snapshot_unavailable"
    );
}

#[test]
fn a_move_elsewhere_while_tally_created_nothing_is_not_called_a_misdirected_post() {
    let after = with_vouchers(&book(), OTHER, 21);
    let created_nothing =
        classify_post_location(&book(), Some(&after), TARGET, "Synthetic Target", Some(0));
    assert_eq!(
        created_nothing["state"], "no_creation_reported",
        "{created_nothing}"
    );
    // A lost or unreadable response cannot rule the other company out.
    let unknown = classify_post_location(&book(), Some(&after), TARGET, "Synthetic Target", None);
    assert_eq!(unknown["state"], "suspected_other_company", "{unknown}");
}

#[test]
fn two_rows_with_one_identity_are_ambiguous_not_merged() {
    let mut doubled = book();
    doubled.push(marks("Synthetic Other", OTHER, 99));
    assert_eq!(
        state(&book(), &doubled)["state"],
        "location_ambiguous_duplicate_rows"
    );
    assert_eq!(
        state(&doubled, &book())["state"],
        "location_ambiguous_duplicate_rows"
    );
}

#[test]
fn a_namesake_beyond_ascii_case_or_inner_spacing_still_refuses() {
    for lookalike in ["Synthetic  Target", "SYNTHETIC\tTARGET", "synthetic target"] {
        let mut namesake = book();
        namesake[1].name = lookalike.into();
        assert_eq!(
            admit_post_target(&namesake, TARGET, "Synthetic Target"),
            Err("post_company_scope_changed"),
            "{lookalike:?}"
        );
    }
    let mut accented = book();
    accented[0].name = "Café Target".into();
    accented[1].name = "CAFÉ TARGET".into();
    assert_eq!(
        admit_post_target(&accented, TARGET, "Café Target"),
        Err("post_company_scope_changed")
    );
}

#[test]
fn a_row_without_its_master_axis_is_refused_not_read_as_unchanged() {
    let bytes = include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-company-book-extents.utf16le.xml"
    );
    let words = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let xml = String::from_utf16(&words).unwrap();
    let start = xml.find("<ALTMSTID").unwrap();
    let end = start + xml[start..].find("</ALTMSTID>").unwrap() + "</ALTMSTID>".len();
    let without = format!("{}{}", &xml[..start], &xml[end..]);
    assert_eq!(
        parse_all_company_marks(&without),
        Err("master_checkpoint_not_observed".into())
    );
}

// bridge#239: the target's master mark, between the snapshot taken as the
// queue's binding reads begin and the aim snapshot.

fn with_masters(rows: &[LoadedCompanyMarks], guid: &str, masters: u64) -> Vec<LoadedCompanyMarks> {
    rows.iter()
        .cloned()
        .map(|mut row| {
            if row.guid == guid {
                row.masters = masters;
            }
            row
        })
        .collect()
}

#[test]
fn only_the_targets_master_mark_decides_whether_masters_moved() {
    let book = book();
    let unchanged = |at_aim: &[LoadedCompanyMarks]| {
        target_masters_unchanged(&book, at_aim, TARGET, "Synthetic Target")
    };
    assert_eq!(unchanged(&book), Some(true));
    assert_eq!(unchanged(&with_masters(&book, TARGET, 8)), Some(false));
    // Another company's masters, and the target's own vouchers, are not ours.
    assert_eq!(unchanged(&with_masters(&book, OTHER, 99)), Some(true));
    assert_eq!(unchanged(&with_vouchers(&book, TARGET, 11)), Some(true));
    // A mark that goes back (a company restored under the same identity) is a
    // change too, never an unchanged book.
    assert_eq!(
        target_masters_unchanged(
            &with_masters(&book, TARGET, 8),
            &book,
            TARGET,
            "Synthetic Target"
        ),
        Some(false)
    );
    // The target's name under another GUID is not the target.
    let mut regenerated = book.clone();
    regenerated[0].guid = "44444444-4444-4444-8444-444444444444".into();
    assert_eq!(unchanged(&regenerated), None);
    // A year-split sibling shares the GUID under another name; it is not the
    // target, whether or not its masters move.
    let mut split = book.clone();
    split.push(marks("Synthetic Target (2024-25)", TARGET, 5));
    let mut split_moved = split.clone();
    split_moved[3].masters = 9;
    assert_eq!(
        target_masters_unchanged(&split, &split_moved, TARGET, "Synthetic Target"),
        Some(true)
    );
}

#[test]
fn a_masters_comparison_without_exactly_one_target_row_says_nothing() {
    let book = book();
    let without_target = book[1..].to_vec();
    let mut doubled = book.clone();
    doubled.push(marks("synthetic target ", TARGET, 10));
    for (at_binding, at_aim) in [
        (without_target.as_slice(), book.as_slice()),
        (book.as_slice(), without_target.as_slice()),
        (doubled.as_slice(), book.as_slice()),
        (book.as_slice(), doubled.as_slice()),
    ] {
        assert_eq!(
            target_masters_unchanged(at_binding, at_aim, TARGET, "Synthetic Target"),
            None
        );
    }
}

/// The target's voucher mark is reported with its step, and whether the step
/// is exactly what Tally reported creating; any other change to a voucher in
/// the target within the interval makes it larger (protocol reference §11c.5).
#[test]
fn the_target_step_is_reported_against_what_tally_reported_creating() {
    let step = |after: u64, created: Option<u64>| {
        classify_post_location(
            &book(),
            Some(&with_vouchers(&book(), TARGET, after)),
            TARGET,
            "Synthetic Target",
            created,
        )["target_voucher_step"]
            .clone()
    };
    assert_eq!(
        step(11, Some(1)),
        json!({"before": 10, "after": 11, "step": 1, "reported_created": 1, "matches_created": true})
    );
    // Another voucher changed in the target: the step exceeds CREATED.
    assert_eq!(step(12, Some(1))["matches_created"], false);
    assert_eq!(step(12, Some(1))["step"], 2);
    // Nothing moved although Tally reported a create.
    assert_eq!(step(10, Some(1))["matches_created"], false);
    // A batch of three, exactly.
    assert_eq!(step(13, Some(3))["matches_created"], true);
    // A lost response: the step is reported, but nothing is matched.
    assert_eq!(step(11, None)["matches_created"], Value::Null);
    assert_eq!(step(11, None)["step"], 1);
    // A mark that went backwards is no step.
    assert_eq!(step(9, Some(1))["step"], Value::Null);
    assert_eq!(step(9, Some(1))["matches_created"], false);
}

#[test]
fn no_step_is_reported_unless_each_snapshot_holds_exactly_one_target() {
    let renamed = book()
        .into_iter()
        .map(|mut row| {
            if row.guid == TARGET {
                row.name = "Renamed Target".into();
                row.vouchers = 11;
            }
            row
        })
        .collect::<Vec<_>>();
    assert_eq!(state(&book(), &renamed)["target_voucher_step"], Value::Null);
    assert_eq!(state(&renamed, &book())["target_voucher_step"], Value::Null);
    // A second row with the target's GUID and name under another key cannot
    // occur (rows are keyed by both); a namesake with another GUID is not the
    // target, so the step stays the target's own.
    let mut namesake = with_vouchers(&book(), TARGET, 11);
    namesake.push(marks(
        "Synthetic Target",
        "44444444-4444-4444-8444-444444444444",
        5,
    ));
    let located = state(&book(), &namesake);
    assert_eq!(located["target_voucher_step"]["step"], 1, "{located}");
}
