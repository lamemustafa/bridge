//! The thirteen files pinned by the raise to 231 (bridge#416) must stay pinned.
//!
//! The compatibility gate cannot notice a pin disappearing. `rehash-surface`
//! updates hashes and never adds paths, and the only correct way to resolve a
//! conflict in a generated artifact is to take one side and regenerate -- so a
//! merge that takes the base side of `compatibility-surface.json` drops every
//! entry a branch added while keeping the raised `MAX_SURFACE_FILES`, and the
//! gate passes. `book_presence_tests.rs` guards its own contract's pins the same
//! way, for the same reason.
//!
//! This file is deliberately not pinned itself: a guard that lived in the
//! surface would be resolved away by the same merge it exists to catch.
use std::collections::BTreeSet;

const SURFACE: &str = include_str!("../../docs/tally/compatibility/compatibility-surface.json");

/// Each path is the unpinned half of a pair whose other half was already
/// pinned. The reason for each is recorded beside `MAX_SURFACE_FILES` in
/// `tools/bridge-tally-compatibility/src/lib.rs`; it is not repeated here, so
/// the two cannot drift apart.
const ADMISSION_AND_EGRESS: [&str; 13] = [
    "src-tauri/crates/bridge-tally-protocol/src/group_ancestry.rs",
    "src-tauri/src/agent_company.rs",
    "src-tauri/src/agent_delivery.rs",
    "src-tauri/src/agent_egress.rs",
    "src-tauri/src/agent_import_cash_bank.rs",
    "src-tauri/src/agent_import_persistence.rs",
    "src-tauri/src/agent_import_post.rs",
    "src-tauri/src/agent_protocol.rs",
    "src-tauri/src/axal.rs",
    "src-tauri/src/documents.rs",
    "src-tauri/src/endpoint_coordination.rs",
    "src-tauri/src/tally/approved_import.rs",
    "src-tauri/src/tally/runtime_control.rs",
];

fn pinned_paths(surface: &str) -> BTreeSet<String> {
    let surface: serde_json::Value = serde_json::from_str(surface).expect("surface json");
    surface["files"]
        .as_array()
        .expect("surface files")
        .iter()
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect()
}

fn unpinned<'a>(pinned: &BTreeSet<String>, required: &[&'a str]) -> Vec<&'a str> {
    required
        .iter()
        .copied()
        .filter(|path| !pinned.contains(*path))
        .collect()
}

#[test]
fn admission_and_egress_files_are_still_pinned() {
    let missing = unpinned(&pinned_paths(SURFACE), &ADMISSION_AND_EGRESS);
    assert!(
        missing.is_empty(),
        "dropped from the compatibility surface: {missing:?}. A merge that took \
         the base side of compatibility-surface.json loses added pins while \
         keeping the raised cap, and the gate cannot see it. Restore the entries \
         and run scripts/reseal.sh --pins-changed."
    );
}

/// The check above must be able to fail. Drive the same two functions over the
/// real surface with one entry removed, rather than a hand-built fixture that
/// would only prove `BTreeSet::contains` works.
#[test]
fn the_pin_check_reports_a_dropped_entry() {
    let dropped = ADMISSION_AND_EGRESS[0];
    let mut surface: serde_json::Value = serde_json::from_str(SURFACE).expect("surface json");
    surface["files"]
        .as_array_mut()
        .expect("surface files")
        .retain(|entry| entry["path"].as_str() != Some(dropped));
    let pinned = pinned_paths(&surface.to_string());
    assert_eq!(unpinned(&pinned, &ADMISSION_AND_EGRESS), vec![dropped]);
}
