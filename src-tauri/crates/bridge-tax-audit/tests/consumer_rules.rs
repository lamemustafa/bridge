// SPDX-License-Identifier: Apache-2.0
//! One refusal per consumer rule the slice enforces (`docs/tax-audit/read-format-v1.md` section
//! 7), each driven by altering a scratch copy of the synthetic read and asserting the rule's
//! code, never a message substring.

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
fn c4_a_read_without_voucher_types_is_refused() {
    let scratch = common::ScratchRead::new("c4-voucher-types");
    let mut m = scratch.manifest();
    m["parts"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["kind"] != "voucher_types");
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C4-required");
}

#[test]
fn c4_a_voucher_type_that_does_not_resolve_is_refused_not_read_as_its_own_base() {
    // "Bank Transfer" is a Contra-derived type the synthetic vouchers use. Without its
    // definition it would otherwise be read as a base type of its own and counted as a real
    // receipt or payment.
    let scratch = common::ScratchRead::new("c4-vtype");
    scratch.edit_part(
        "voucher-types",
        "NAME=\"Bank Transfer\"",
        "NAME=\"Bank Transfer (renamed)\"",
        true,
    );
    assert_eq!(
        code_of(common::run(&scratch.dir, false)),
        "C4-vtype-unresolved"
    );
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
    assert!(matches!(err, AuditError::UnknownVoucherStatus(10)), "{err}");
}

// Company identity is (GUID, books_from), never the name. A Tally split company keeps its
// parent's GUID; on the one pair measured (2026-09-21) the two differed in books_from, 20250401
// against 20260401, so the GUID alone lets a read of one pass as the other.

const FIXTURE_GUID: &str = "6f1c2a3e-8b4d-4c5e-9a7f-0d1e2f3a4b5c";

fn run_pinned(
    read_dir: &std::path::Path,
    guid: &str,
    books_from: &str,
) -> bridge_tax_audit::Result<Value> {
    let mut e = common::engagement(read_dir, false);
    e.company_pin = Some(bridge_tax_audit::read::CompanyPin {
        guid: guid.to_string(),
        books_from: bridge_tally_primitives::TallyDate::parse(books_from.replace('-', "")).unwrap(),
    });
    bridge_tax_audit::cash_44ab_canonical(&e, &bridge_tax_audit::rules_for(&e)?)
}

/// A read of a company whose books begin on `iso`: manifest and company part agree.
fn set_books_from(scratch: &common::ScratchRead, iso: &str) {
    let mut m = scratch.manifest();
    m["company"]["books_from"] = json!(iso);
    scratch.set_manifest(&m);
    scratch.edit_part(
        "company",
        &format!("<GUID TYPE=\"String\">{FIXTURE_GUID}</GUID>"),
        &format!(
            "<GUID TYPE=\"String\">{FIXTURE_GUID}</GUID><BOOKSFROM TYPE=\"Date\">{}</BOOKSFROM>",
            iso.replace('-', "")
        ),
        true,
    );
}

#[test]
fn c5_a_matching_pin_is_admitted_and_guid_case_is_not_identity() {
    let scratch = common::ScratchRead::new("c5-pin-ok");
    assert!(run_pinned(&scratch.dir, &FIXTURE_GUID.to_uppercase(), "2025-04-01").is_ok());
}

#[test]
fn c5_a_pin_for_another_guid_is_refused() {
    let scratch = common::ScratchRead::new("c5-pin-guid");
    let r = run_pinned(
        &scratch.dir,
        "00000000-0000-4000-8000-000000000000",
        "2025-04-01",
    );
    assert_eq!(code_of(r), "C5-client");
}

#[test]
fn c5_a_pin_for_other_books_is_refused() {
    let scratch = common::ScratchRead::new("c5-pin-from");
    assert_eq!(
        code_of(run_pinned(&scratch.dir, FIXTURE_GUID, "2019-04-01")),
        "C5-client"
    );
}

#[test]
fn c5_a_pin_the_manifest_cannot_check_is_refused() {
    let scratch = common::ScratchRead::new("c5-pin-unrecorded");
    let mut m = scratch.manifest();
    m["company"]["books_from"] = Value::Null;
    scratch.set_manifest(&m);
    assert_eq!(
        code_of(run_pinned(&scratch.dir, FIXTURE_GUID, "2025-04-01")),
        "C5-client"
    );
}

/// The synthetic engagement with a `[client.tally]` table inserted, parsed by `from_toml`.
fn pinned_engagement(
    guid: &str,
    books_from: &str,
) -> bridge_tax_audit::Result<bridge_tax_audit::Engagement> {
    let text = std::fs::read_to_string(common::fixtures().join("synthetic-engagement.toml"))
        .unwrap()
        .replace(
            "[period]",
            &format!(
                "[client.tally]\ncompany_guid = {}\nbooks_from = {}\n\n[period]",
                toml::Value::from(guid),
                toml::Value::from(books_from)
            ),
        );
    bridge_tax_audit::Engagement::from_toml(&text, &common::fixtures())
}

#[test]
fn c5_a_valid_pin_survives_the_config_parse() {
    let e = pinned_engagement(&FIXTURE_GUID.to_uppercase(), "2025-04-01").unwrap();
    let pin = e.company_pin.expect("pin parsed");
    assert_eq!(pin.guid, FIXTURE_GUID);
    assert_eq!(pin.books_from.as_str(), "20250401");
}

#[test]
fn c5_a_malformed_pin_is_refused() {
    for bad in [
        "01-04-2025",
        "2025-4-1",
        "20250401",
        "2025-02-30",
        " 2025-04-01",
        "2025-04-01\n",
        "20250-4-01",
        "2025--0401",
        "2025-0401-",
    ] {
        let err = pinned_engagement(FIXTURE_GUID, bad).unwrap_err();
        assert_eq!(err.code(), Some("CFG-tally-pin"), "books_from {bad:?}");
    }
    for bad in [
        format!(" {FIXTURE_GUID}"),
        format!("{FIXTURE_GUID} "),
        format!("\u{1c}{FIXTURE_GUID}"),
        "not-a-guid".to_string(),
        String::new(),
    ] {
        let err = pinned_engagement(&bad, "2025-04-01").unwrap_err();
        assert_eq!(err.code(), Some("CFG-tally-pin"), "guid {bad:?}");
    }
}

#[test]
fn c5_books_that_begin_inside_the_period_are_admitted() {
    // A company's first year: not a split, and nothing in the read is inconsistent.
    let scratch = common::ScratchRead::new("c5-first-year");
    set_books_from(&scratch, "2025-06-15");
    assert!(common::run(&scratch.dir, false).is_ok());
}

#[test]
fn c5_books_that_begin_after_the_period_ends_are_refused_unpinned() {
    let scratch = common::ScratchRead::new("c5-books-from");
    set_books_from(&scratch, "2026-04-01");
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C5-books-from");
}

#[test]
fn c5_a_company_part_booksfrom_must_equal_the_manifest() {
    let scratch = common::ScratchRead::new("c5-part-from");
    set_books_from(&scratch, "2025-04-01");
    assert!(
        common::run(&scratch.dir, false).is_ok(),
        "agreeing BOOKSFROM is admitted"
    );
    let mut m = scratch.manifest();
    m["company"]["books_from"] = json!("2019-04-01");
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C5-identity");
    m["company"]["books_from"] = Value::Null;
    scratch.set_manifest(&m);
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C5-identity");
}

// Negative controls for the split pair: the parent's books begin 2019-04-01, its split's on the
// split date; both carry the same GUID.
#[test]
fn c5_the_split_read_for_the_parents_year_is_refused() {
    let scratch = common::ScratchRead::new("c5-split-for-parent");
    set_books_from(&scratch, "2026-04-01"); // the split: books begin the day after the audited year
    assert_eq!(
        code_of(run_pinned(&scratch.dir, FIXTURE_GUID, "2019-04-01")),
        "C5-client"
    );
    assert_eq!(code_of(common::run(&scratch.dir, false)), "C5-books-from");
}

/// Unpinned, this read is admitted: the parent's books began earlier, so nothing in the read is
/// inconsistent. Only the engagement's pin separates it.
#[test]
fn c5_the_parent_read_for_the_splits_year_is_refused_when_pinned() {
    let scratch = common::ScratchRead::new("c5-parent-for-split");
    set_books_from(&scratch, "2019-04-01"); // the parent, whose books still begin in 2019
    assert_eq!(
        code_of(run_pinned(&scratch.dir, FIXTURE_GUID, "2025-04-01")),
        "C5-client"
    );
}

/// A voucher's NARRATION reaches the book Python-stripped, as the reference's adapter reads it.
#[test]
fn a_voucher_narration_is_read_and_stripped() {
    let scratch = common::ScratchRead::new("narration");
    scratch.edit_part(
        "vouchers-2025-04-01",
        "<NARRATION TYPE=\"String\">Synthetic voucher CS/1</NARRATION>",
        "<NARRATION> \t Counter sale  week 1 \n</NARRATION>",
        true,
    );
    let e = common::engagement(&scratch.dir, false);
    let book = bridge_tax_audit::load_book(&e).unwrap();
    let narration = |number: &str| {
        book.vouchers
            .iter()
            .find(|v| v.number == number)
            .unwrap_or_else(|| panic!("{number}"))
            .narration
            .clone()
    };
    assert_eq!(narration("CS/1"), "Counter sale  week 1");
    assert_eq!(narration("R/1"), "Synthetic voucher R/1");
}

/// A voucher's PARTYLEDGERNAME reaches the book Python-stripped, as the reference's adapter reads
/// it (`_t(v, "PARTYLEDGERNAME")`); a voucher without one reads empty.
#[test]
fn a_voucher_party_field_is_read_and_stripped() {
    let scratch = common::ScratchRead::new("party-field");
    scratch.edit_part(
        "vouchers-2025-04-01",
        "<NARRATION TYPE=\"String\">Synthetic voucher CS/1</NARRATION>",
        "<NARRATION TYPE=\"String\">Synthetic voucher CS/1</NARRATION>\
         <PARTYLEDGERNAME TYPE=\"String\"> \t Pinecrest Builders \n</PARTYLEDGERNAME>",
        true,
    );
    let e = common::engagement(&scratch.dir, false);
    let book = bridge_tax_audit::load_book(&e).unwrap();
    let party = |number: &str| {
        book.vouchers
            .iter()
            .find(|v| v.number == number)
            .unwrap_or_else(|| panic!("{number}"))
            .party_field
            .clone()
    };
    assert_eq!(party("CS/1"), "Pinecrest Builders");
    assert_eq!(party("R/1"), "");
}
