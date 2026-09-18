// SPDX-License-Identifier: Apache-2.0
//! One refusal per consumer rule the slice enforces (READ-FORMAT-v1 section 7), each driven by
//! altering a scratch copy of the synthetic read and asserting the rule's code, never a
//! message substring.

mod common;

use bridge_tax_audit::AuditError;
use serde_json::{json, Value};

fn code_of(result: bridge_tax_audit::Result<Value>) -> &'static str {
    match result {
        Err(e) => e
            .code()
            .unwrap_or_else(|| panic!("not a rule refusal: {e}")),
        Ok(_) => panic!("the read was admitted"),
    }
}

fn part_mut<'a>(manifest: &'a mut Value, id: &str) -> &'a mut Value {
    manifest["parts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["id"] == id)
        .unwrap()
}

#[test]
fn the_unaltered_read_is_admitted() {
    let scratch = common::ScratchRead::new("clean");
    assert!(common::run(&scratch.dir, false).is_ok());
}

#[test]
fn c3_bytes_that_do_not_match_the_manifest_are_refused() {
    let scratch = common::ScratchRead::new("c3");
    scratch.edit_part("vouchers-2025-04-01", "-42000.00", "-42000.01", false);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C3-stored-hash");
}

#[test]
fn c3_an_unconsumed_part_is_verified_too() {
    let scratch = common::ScratchRead::new("c3-unconsumed");
    let mut m = scratch.manifest();
    part_mut(&mut m, "high-water-before")["response"]["sha256"] = json!("0".repeat(64));
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C3-content-hash");
}

#[test]
fn c4_a_missing_required_kind_is_refused() {
    let scratch = common::ScratchRead::new("c4");
    let mut m = scratch.manifest();
    m["parts"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["kind"] != "trial_balance");
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C4-required");
}

#[test]
fn c5_a_company_part_for_another_guid_is_refused() {
    let scratch = common::ScratchRead::new("c5");
    let mut m = scratch.manifest();
    m["company"]["guid"] = json!("00000000-0000-4000-8000-000000000000");
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C5-identity");
}

#[test]
fn c6_a_moved_bracket_is_refused_whatever_the_label_says() {
    let scratch = common::ScratchRead::new("c6");
    let mut m = scratch.manifest();
    m["consistency"]["after"]["alter_master_id"] = json!(58);
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C6-status");
    m["consistency"]["status"] = json!("moved");
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C6-moved");
}

#[test]
fn c6_an_unbracketed_read_needs_the_opt_in() {
    let scratch = common::ScratchRead::new("c6-unbracketed");
    let mut m = scratch.manifest();
    m["consistency"] = json!({"status": "not_recorded", "before": null, "after": null});
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C6-unbracketed");
    assert!(common::run(&scratch.dir, true).is_ok());
}

#[test]
fn c7_a_gap_between_voucher_windows_is_refused() {
    let scratch = common::ScratchRead::new("c7");
    let mut m = scratch.manifest();
    part_mut(&mut m, "vouchers-2025-10-01")["window"]["from"] = json!("2025-10-02");
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C7-gap");
}

/// The Education-mode widening case: a first-half part that also carries a second-half voucher.
#[test]
fn c8_a_voucher_outside_its_window_is_refused() {
    let scratch = common::ScratchRead::new("c8");
    scratch.edit_part(
        "vouchers-2025-04-01",
        "<DATE TYPE=\"Date\">20250928</DATE>",
        "<DATE TYPE=\"Date\">20251003</DATE>",
        true,
    );
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C8-window");
}

#[test]
fn c9_a_declared_row_count_that_disagrees_is_refused() {
    let scratch = common::ScratchRead::new("c9");
    let mut m = scratch.manifest();
    part_mut(&mut m, "vouchers-2025-04-01")["rows"] = json!(11);
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C9-rows");
}

#[test]
fn c9_an_alter_id_above_the_closing_high_water_is_refused() {
    let scratch = common::ScratchRead::new("c9-hw");
    let mut m = scratch.manifest();
    for side in ["before", "after"] {
        m["consistency"][side]["alter_voucher_id"] = json!(115);
    }
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C9-high-water");
}

#[cfg(unix)]
#[test]
fn c2_a_symlinked_part_is_refused() {
    let scratch = common::ScratchRead::new("c2");
    let target = scratch.dir.join("parts/groups.xml");
    let moved = scratch.dir.join("groups-real.xml");
    std::fs::rename(&target, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &target).unwrap();
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C2-symlink");
}

/// Without the side list, the second-half vouchers carry no status flags, so their status is
/// unknown and the population refuses rather than guessing they are regular.
#[test]
fn unknown_voucher_status_refuses_the_population() {
    let scratch = common::ScratchRead::new("status");
    let mut m = scratch.manifest();
    m["parts"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["kind"] != "voucher_status_list");
    scratch.set_manifest(&m);
    let err = common::run(&scratch.dir, false).unwrap_err();
    assert!(matches!(err, AuditError::UnknownVoucherStatus(6)), "{err}");
}
