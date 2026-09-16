use super::*;
use crate::PartyLedgerMasterFieldObservation as Observed;

fn group(name: &str, parent: &str, reserved: Option<&str>) -> TallyNamedMaster {
    TallyNamedMaster {
        name: name.into(),
        parent: Observed::Returned(parent.into()),
        reserved_name: reserved.map(str::to_string),
    }
}

fn tree() -> GroupIndex {
    GroupIndex::build([
        group("Bank Accounts", "Current Assets", Some("Bank Accounts")),
        group(
            "Current Assets",
            "\u{fffd}#4; Primary",
            Some("Current Assets"),
        ),
        group("Sundry Debtors", "Current Assets", Some("Sundry Debtors")),
        // Tally's own signal that a group is user-created.
        group("House Debtors", "Sundry Debtors", Some("")),
    ])
}

#[test]
fn a_user_created_group_is_climbed_through_to_its_predefined_ancestor() {
    assert_eq!(
        tree().reserved_ancestor(Some("Bank Accounts")),
        Ok("Bank Accounts")
    );
    assert_eq!(
        tree().reserved_ancestor(Some("House Debtors")),
        Ok("Sundry Debtors")
    );
}

#[test]
fn a_renamed_predefined_group_answers_with_its_reserved_identity() {
    let renamed = GroupIndex::build([group(
        "Current Account",
        "Current Assets",
        Some("Bank Accounts"),
    )]);
    assert_eq!(
        renamed.reserved_ancestor(Some("Current Account")),
        Ok("Bank Accounts")
    );
    // The old name now belongs to nobody, and resolves to nothing.
    assert_eq!(
        renamed.reserved_ancestor(Some("Bank Accounts")),
        Err(AncestryGap::GroupAbsent)
    );
}

#[test]
fn every_refusal_is_distinguishable_and_none_is_an_answer() {
    let tree = tree();
    assert_eq!(tree.reserved_ancestor(None), Err(AncestryGap::NoParent));
    assert_eq!(
        tree.reserved_ancestor(Some("Nowhere")),
        Err(AncestryGap::GroupAbsent)
    );
    // The repaired form the tolerant reader actually produces, and the
    // bare word a report rendering leaves behind.
    for root in ["\u{fffd}#4; Primary", "Primary"] {
        assert_eq!(
            tree.reserved_ancestor(Some(root)),
            Err(AncestryGap::ReachedRoot),
            "{root:?}"
        );
    }
    // A raw `U+0004` prefix is deliberately *not* a spelling this
    // recognises: the observed PARENT carries the character reference, and
    // `is_tally_reserved_root` is defined against that. Should a raw one
    // ever arrive it resolves as an absent group, which still refuses —
    // pinned here so the narrower definition is a decision, not a gap.
    assert_eq!(
        tree.reserved_ancestor(Some("\u{4} Primary")),
        Err(AncestryGap::GroupAbsent)
    );
    let repeated = GroupIndex::build([
        group("Bank Accounts", "Current Assets", Some("Bank Accounts")),
        group("Bank Accounts", "Current Assets", Some("Bank Accounts")),
    ]);
    assert_eq!(
        repeated.reserved_ancestor(Some("Bank Accounts")),
        Err(AncestryGap::GroupNameRepeated)
    );
    let unattributed = GroupIndex::build([group("Bank Accounts", "Current Assets", None)]);
    assert_eq!(
        unattributed.reserved_ancestor(Some("Bank Accounts")),
        Err(AncestryGap::ReservedNameMissing)
    );
    let looping = GroupIndex::build([group("Loop", "Loop", Some(""))]);
    assert_eq!(
        looping.reserved_ancestor(Some("Loop")),
        Err(AncestryGap::Cycle)
    );
}

#[test]
fn a_hop_that_differs_only_by_case_or_space_is_absent_not_resolved() {
    // Measured across both captured companies: 21 distinct PARENT values,
    // every one an exact match to a group NAME except the reserved root,
    // and no case- or whitespace-only near match anywhere. So a pair that
    // differs is an incoherent or cross-snapshot pair, and resolving it
    // would admit a ledger on evidence that does not hold.
    let tree = tree();
    for near in [
        "bank accounts",
        "BANK ACCOUNTS",
        " Bank Accounts",
        "Bank Accounts ",
    ] {
        assert_eq!(
            tree.reserved_ancestor(Some(near)),
            Err(AncestryGap::GroupAbsent),
            "{near:?}"
        );
    }
    assert_eq!(
        tree.reserved_ancestor(Some("Bank Accounts")),
        Ok("Bank Accounts")
    );
}

#[test]
fn a_whitespace_only_reserved_name_is_no_more_an_identity_than_an_empty_one() {
    // Returning it as an ancestor let a caller read "established as
    // something other than money" from a row that established nothing,
    // which positively admitted a counterparty leg.
    let blank = GroupIndex::build([
        group("Odd", "Current Assets", Some("   ")),
        group(
            "Current Assets",
            "\u{fffd}#4; Primary",
            Some("Current Assets"),
        ),
    ]);
    assert_eq!(blank.reserved_ancestor(Some("Odd")), Ok("Current Assets"));
    let orphan = GroupIndex::build([group("Odd", "\u{fffd}#4; Primary", Some(" "))]);
    assert_eq!(
        orphan.reserved_ancestor(Some("Odd")),
        Err(AncestryGap::ReachedRoot)
    );
}

#[test]
fn an_empty_reserved_name_is_not_an_absent_one() {
    // The two look alike and mean opposite things: empty is Tally saying
    // "user-created, keep climbing", absent is a reader that never asked.
    let user_created = GroupIndex::build([group("Custom", "\u{fffd}#4; Primary", Some(""))]);
    assert_eq!(
        user_created.reserved_ancestor(Some("Custom")),
        Err(AncestryGap::ReachedRoot)
    );
    let never_captured = GroupIndex::build([group("Custom", "\u{fffd}#4; Primary", None)]);
    assert_eq!(
        never_captured.reserved_ancestor(Some("Custom")),
        Err(AncestryGap::ReservedNameMissing)
    );
}
