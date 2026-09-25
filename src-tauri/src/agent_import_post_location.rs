//! Where a native post landed (bridge#574).
//!
//! The import names its company only by `SVCURRENTCOMPANY`, and Tally offers
//! no way to bind it to a GUID. Live on licensed 7.1 Silver (2026-09-21/22):
//! a matched name posts into the named company even when another is selected
//! (T1); a name matching no loaded company fails closed whichever company is
//! selected (T2, T3); a rename between build and post is refused at admission
//! (T5); and a post into another existing company that lacks one of the
//! voucher's ledgers is rejected by Tally (T7). What remains is a rename, in
//! the moments before the POST, to another loaded company's exact name whose
//! ledgers all overlap. Nothing prevents that. This module confirms the aim
//! from a snapshot taken as the last Tally request before the POST, and
//! classifies where the voucher went from a snapshot taken right after it.
//!
//! Identity is the GUID plus the row's `NAME`, compared as
//! `require_unique_company_scope` compares names (trimmed, ASCII case folded).
//! Number and books-from are not in this collection; that field set is
//! unmeasured, so it is not relied on.
use super::*;
pub(super) use crate::agent::change_parse::{parse_all_company_marks, LoadedCompanyMarks};

fn same_name(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

/// A broader fold for counting namesakes only: Unicode lower case and runs of
/// whitespace collapsed. How Tally resolves `SVCURRENTCOMPANY` beyond ASCII
/// case is unmeasured, so a name this fold makes equal is treated as a possible
/// landing. It can only refuse more, never admit a post the narrow match would
/// refuse.
fn possibly_same_name(left: &str, right: &str) -> bool {
    let fold = |name: &str| {
        name.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    fold(left) == fold(right)
}

fn is_target(row: &LoadedCompanyMarks, guid: &str, name: &str) -> bool {
    row.guid.eq_ignore_ascii_case(guid) && same_name(&row.name, name)
}

/// The aim check, on the snapshot sent last before the POST: exactly one loaded
/// company is the target (GUID and name), and no other loaded company shares
/// its name. Anything else refuses before any import request.
pub(super) fn admit_post_target(
    before: &[LoadedCompanyMarks],
    guid: &str,
    name: &str,
) -> Result<(), &'static str> {
    let targets = before
        .iter()
        .filter(|row| is_target(row, guid, name))
        .count();
    let namesakes = before
        .iter()
        .filter(|row| possibly_same_name(&row.name, name))
        .count();
    if targets == 1 && namesakes == 1 {
        Ok(())
    } else {
        Err("post_company_scope_changed")
    }
}

/// The target's master mark, compared between the snapshot taken as the
/// queue's binding reads begin and the aim snapshot sent last before the POST
/// (bridge#239). Tally offers no conditional import, so a master renamed,
/// regrouped or created after the catalogue re-read would otherwise go unseen.
/// `None` when either snapshot does not hold exactly one target row: nothing
/// can be said then, and the caller refuses.
pub(super) fn target_masters_unchanged(
    at_binding: &[LoadedCompanyMarks],
    at_aim: &[LoadedCompanyMarks],
    guid: &str,
    name: &str,
) -> Option<bool> {
    let target = |rows: &[LoadedCompanyMarks]| {
        let mut targets = rows.iter().filter(|row| is_target(row, guid, name));
        match (targets.next(), targets.next()) {
            (Some(row), None) => Some(row.masters),
            _ => None,
        }
    };
    Some(target(at_binding)? == target(at_aim)?)
}

/// The target's voucher mark in each snapshot, and whether it moved by exactly
/// what Tally reported creating. Each voucher create moves `ALTVCHID` by one,
/// and so does every alter or cancel (a delete by two; protocol reference
/// §11c.5), so a step above `CREATED` means another voucher in the target
/// changed within the snapshots' interval. Reported only: a single post is
/// proved by its readback. A batch post, not yet built, is to gate on it.
/// `Null` unless each snapshot holds exactly one target row.
fn target_voucher_step(
    before: &[LoadedCompanyMarks],
    after: &[LoadedCompanyMarks],
    guid: &str,
    name: &str,
    reported_created: Option<u64>,
) -> Value {
    let mark = |rows: &[LoadedCompanyMarks]| {
        let mut targets = rows.iter().filter(|row| is_target(row, guid, name));
        match (targets.next(), targets.next()) {
            (Some(row), None) => Some(row.vouchers),
            _ => None,
        }
    };
    let (Some(from), Some(to)) = (mark(before), mark(after)) else {
        return Value::Null;
    };
    // A mark that went backwards is no step at all.
    let step = to.checked_sub(from);
    json!({
        "before": from,
        "after": to,
        "step": step,
        "reported_created": reported_created,
        "matches_created": reported_created.map(|created| step == Some(created)),
    })
}

fn key(row: &LoadedCompanyMarks) -> (String, String) {
    (
        row.guid.to_ascii_lowercase(),
        row.name.trim().to_ascii_lowercase(),
    )
}

fn described(row: &LoadedCompanyMarks) -> Value {
    json!({"name": row.name, "guid": row.guid})
}

/// Where the post went, from the snapshots either side of it. Attribution of
/// the voucher itself stays with the marker readback; this only says which
/// companies' voucher marks moved. `after` is `None` when the snapshot after
/// the POST could not be read, and that is said, never guessed.
/// `reported_created` is Tally's CREATED counter, `None` when the response was
/// lost or unreadable. A mark that moved elsewhere while Tally reported
/// creating nothing is someone else's voucher, not a misdirected post.
pub(super) fn classify_post_location(
    before: &[LoadedCompanyMarks],
    after: Option<&[LoadedCompanyMarks]>,
    guid: &str,
    name: &str,
    reported_created: Option<u64>,
) -> Value {
    let Some(after) = after else {
        return json!({"state": "after_snapshot_unavailable"});
    };
    let before_by_key: BTreeMap<_, _> = before.iter().map(|row| (key(row), row)).collect();
    let after_by_key: BTreeMap<_, _> = after.iter().map(|row| (key(row), row)).collect();
    if before_by_key.len() != before.len() || after_by_key.len() != after.len() {
        // Two rows with one identity cannot be told apart, so neither can
        // be said to have moved or stayed.
        return json!({"state": "location_ambiguous_duplicate_rows"});
    }
    let added: Vec<Value> = after
        .iter()
        .filter(|row| !before_by_key.contains_key(&key(row)))
        .map(described)
        .collect();
    let removed: Vec<Value> = before
        .iter()
        .filter(|row| !after_by_key.contains_key(&key(row)))
        .map(described)
        .collect();
    let moved: Vec<(&LoadedCompanyMarks, u64, u64)> = after
        .iter()
        .filter_map(|row| {
            let previous = before_by_key.get(&key(row))?;
            (previous.vouchers != row.vouchers).then_some((row, previous.vouchers, row.vouchers))
        })
        .collect();
    let target_before = before.iter().any(|row| is_target(row, guid, name));
    let target_after = after.iter().any(|row| is_target(row, guid, name));
    let target_moved = moved.iter().any(|(row, _, _)| is_target(row, guid, name));
    let others: Vec<Value> = moved
        .iter()
        .filter(|(row, _, _)| !is_target(row, guid, name))
        .map(|(row, from, to)| {
            json!({"name": row.name, "guid": row.guid, "voucher_mark_before": from, "voucher_mark_after": to})
        })
        .collect();
    let state = if !target_before || !target_after {
        // The target itself was renamed or unloaded: where the voucher went
        // cannot be ruled out, so this is never "not posted".
        "target_scope_changed"
    } else if !added.is_empty() || !removed.is_empty() {
        if target_moved {
            "target_moved_scope_changed"
        } else {
            "location_ambiguous_scope_changed"
        }
    } else if target_moved && others.is_empty() {
        "target_only"
    } else if target_moved {
        "target_and_others_moved"
    } else if reported_created == Some(0) {
        "no_creation_reported"
    } else if others.len() == 1 {
        "suspected_other_company"
    } else if others.is_empty() {
        "not_observed"
    } else {
        "ambiguous_concurrent_changes"
    };
    json!({
        "state": state,
        "target_moved": target_moved,
        "target_voucher_step": target_voucher_step(before, after, guid, name, reported_created),
        "other_companies_moved": others,
        "companies_added": added,
        "companies_removed": removed,
    })
}

#[cfg(test)]
#[path = "agent_import_post_location_tests.rs"]
mod tests;
