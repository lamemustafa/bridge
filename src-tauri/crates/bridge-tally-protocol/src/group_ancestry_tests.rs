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
    // The marked form every Bridge decoder produces for `&#4; Primary`
    // (`TALLY_PROTOCOL_REFERENCE.md` §1.1(d)), padded or not.
    for root in ["\u{fffd}#4; Primary", "  \u{fffd}#4;   primary "] {
        assert_eq!(
            tree.reserved_ancestor(Some(root)),
            Err(AncestryGap::ReachedRoot),
            "{root:?}"
        );
    }
    // Every other spelling names a group, and a group this index does not
    // hold is absent, which still refuses. A bare `Primary` is a group a user
    // called that; a raw `U+0004` prefix and the undecoded reference are text
    // no Bridge decoder produces. Pinned so the narrow definition is a
    // decision, not a gap.
    for spelling in ["Primary", "\u{4} Primary", "&#4; Primary"] {
        assert_eq!(
            tree.reserved_ancestor(Some(spelling)),
            Err(AncestryGap::GroupAbsent),
            "{spelling:?}"
        );
    }
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

fn hop(name: &str, reserved: &str) -> AncestryHop {
    AncestryHop {
        name: name.to_string(),
        reserved_name: reserved.to_string(),
    }
}

#[test]
fn a_normal_multi_level_chain_resolves_every_hop_to_the_root() {
    // HDFC CC -> Bank OD A/c -> Loans (Liability) -> reserved root, matching
    // the lab's own multi-level tree (HDFC CC under Bank OD A/c under Loans
    // (Liability)). Two predefined groups stacked on each other: the walk
    // must not stop at the first one.
    let index = GroupIndex::build([
        group("Bank OD A/c", "Loans (Liability)", Some("Bank OD A/c")),
        group(
            "Loans (Liability)",
            "\u{fffd}#4; Primary",
            Some("Loans (Liability)"),
        ),
    ]);
    let chain = index.ancestry_chain(Some("Bank OD A/c"));
    assert!(chain.is_complete());
    assert_eq!(chain.gap, None);
    assert_eq!(
        chain.hops,
        vec![
            hop("Bank OD A/c", "Bank OD A/c"),
            hop("Loans (Liability)", "Loans (Liability)"),
        ]
    );
}

#[test]
fn a_user_created_group_is_recorded_as_its_own_hop_not_skipped() {
    // House Debtors is user-created (empty RESERVEDNAME) and sits under the
    // predefined Sundry Debtors. Unlike reserved_ancestor, which climbs
    // through a user-created group silently, the full chain must show it.
    let chain = tree().ancestry_chain(Some("House Debtors"));
    assert!(chain.is_complete());
    assert_eq!(
        chain.hops,
        vec![
            hop("House Debtors", ""),
            hop("Sundry Debtors", "Sundry Debtors"),
            hop("Current Assets", "Current Assets"),
        ]
    );
}

#[test]
fn a_ledger_directly_under_a_primary_group_yields_a_single_complete_hop() {
    // Cash-in-Hand's own parent is the reserved root directly, matching the
    // lab's own `Cash` ledger under `Cash-in-Hand`: one hop, then done.
    let index = GroupIndex::build([group(
        "Cash-in-Hand",
        "\u{fffd}#4; Primary",
        Some("Cash-in-Hand"),
    )]);
    let chain = index.ancestry_chain(Some("Cash-in-Hand"));
    assert!(chain.is_complete());
    assert_eq!(chain.gap, None);
    assert_eq!(chain.hops, vec![hop("Cash-in-Hand", "Cash-in-Hand")]);
}

#[test]
fn a_chain_with_an_ancestry_gap_keeps_the_resolved_prefix_and_names_the_gap() {
    // Bank Accounts' own parent ("Current Assets") is not in this narrower
    // index, so the walk resolves one real hop and then refuses rather than
    // guessing the rest.
    let narrow = GroupIndex::build([group(
        "Bank Accounts",
        "Current Assets",
        Some("Bank Accounts"),
    )]);
    let chain = narrow.ancestry_chain(Some("Bank Accounts"));
    assert!(!chain.is_complete());
    assert_eq!(chain.gap, Some(AncestryGap::GroupAbsent));
    assert_eq!(chain.hops, vec![hop("Bank Accounts", "Bank Accounts")]);
}

#[test]
fn ancestry_chain_reports_every_other_gap_with_its_resolved_prefix() {
    assert_eq!(
        GroupIndex::default().ancestry_chain(None),
        AncestryChain {
            hops: vec![],
            gap: Some(AncestryGap::NoParent),
        }
    );
    let repeated = GroupIndex::build([
        group("Odd Parent", "Bank Accounts", Some("")),
        group("Bank Accounts", "Current Assets", Some("Bank Accounts")),
        group("Bank Accounts", "Current Assets", Some("Bank Accounts")),
    ]);
    let chain = repeated.ancestry_chain(Some("Odd Parent"));
    assert_eq!(chain.hops, vec![hop("Odd Parent", "")]);
    assert_eq!(chain.gap, Some(AncestryGap::GroupNameRepeated));

    let unattributed = GroupIndex::build([
        group("Odd Parent", "No RESERVEDNAME", Some("")),
        group("No RESERVEDNAME", "Current Assets", None),
    ]);
    let chain = unattributed.ancestry_chain(Some("Odd Parent"));
    assert_eq!(chain.hops, vec![hop("Odd Parent", "")]);
    assert_eq!(chain.gap, Some(AncestryGap::ReservedNameMissing));

    let looping = GroupIndex::build([group("Loop", "Loop", Some(""))]);
    let chain = looping.ancestry_chain(Some("Loop"));
    // "Loop" is itself resolved as one hop (its own RESERVEDNAME is known)
    // before the walk revisits "Loop" as a parent and detects the cycle.
    assert_eq!(chain.hops, vec![hop("Loop", "")]);
    assert_eq!(chain.gap, Some(AncestryGap::Cycle));
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

#[test]
fn a_user_group_named_primary_is_climbed_through_not_taken_for_the_root() {
    // Tally's reserved root reaches every decoder as the marked
    // `U+FFFD#4; Primary`; a bare `Primary` can only be a group someone
    // named that. Taking it for the root ended the walk one hop early and
    // answered "no predefined ancestor" for a ledger that has one.
    let index = GroupIndex::build([
        group("Primary", "Sundry Debtors", Some("")),
        group(
            "Sundry Debtors",
            "\u{fffd}#4; Primary",
            Some("Sundry Debtors"),
        ),
    ]);
    assert_eq!(
        index.reserved_ancestor(Some("Primary")),
        Ok("Sundry Debtors")
    );
    let chain = index.ancestry_chain(Some("Primary"));
    assert!(chain.is_complete());
    assert_eq!(
        chain.hops,
        vec![hop("Primary", ""), hop("Sundry Debtors", "Sundry Debtors")]
    );
}

#[test]
fn every_captured_root_parent_is_the_root_and_its_bare_word_is_not() {
    // The committed capture's own PARENT values, through the native parser.
    const CAPTURE: &[u8] =
        include_bytes!("../tests/fixtures/native/group_snapshot_wr2_with_identity.utf16le.xml");
    let xml = crate::decode_tally_xml_response_bytes_limited(
        CAPTURE,
        "text/xml; charset=utf-16",
        crate::ExpectedTallyTextEncoding::Utf16Le,
        CAPTURE.len(),
    )
    .expect("captured BOM-less UTF-16LE response decodes")
    .text;
    let groups = crate::parse_native_group_source_records_with_evidence(
        &xml,
        "61c6de69-1748-461c-ad3f-162cb949df9f",
    )
    .expect("captured groups parse");
    let root_parents = groups
        .records
        .iter()
        .filter_map(|group| group.record.parent.returned_text())
        .filter(|parent| crate::is_tally_reserved_root(parent))
        .collect::<Vec<_>>();
    // Every top-level group in the capture, and nothing else.
    assert_eq!(
        root_parents.len(),
        xml.matches("<PARENT TYPE=\"String\">&#4; Primary</PARENT>")
            .count()
    );
    assert!(!root_parents.is_empty());
    for parent in root_parents {
        assert_eq!(parent, "\u{fffd}#4; Primary");
        let bare = parent.trim_start_matches(crate::TALLY_SANITIZED_ROOT_MARKER);
        assert!(!crate::is_tally_reserved_root(bare), "{bare:?}");
    }
}
