//! Every name, code and number here is fabricated from a placeholder
//! alphabet — Greek-letter party names, `PH-` stock codes, and numbers drawn
//! from the `55500000xx` placeholder block. Nothing is edited down from an
//! observed book, and none of it is evidence about any Tally instance: these
//! tests establish the behaviour of the binding rules only.

use super::*;

fn ledgers(names: &[&str]) -> MasterCatalog {
    MasterCatalog::new(MasterClass::Ledger, names).expect("fabricated catalog is valid")
}

fn entity(name: &str) -> SourceEntity {
    SourceEntity::new(1, name).expect("fabricated source name is valid")
}

fn bound(catalog: &MasterCatalog, entities: &[SourceEntity]) -> BindingReport {
    bind(catalog, entities).expect("fabricated entity list is within bounds")
}

fn bind_one_name(catalog: &MasterCatalog, name: &str) -> EntityBinding {
    bound(catalog, &[entity(name)])
        .entities()
        .first()
        .cloned()
        .expect("one entity in, one binding out")
}

fn candidate_names(binding: &EntityBinding) -> Vec<&str> {
    binding
        .unresolved()
        .expect("binding did not resolve")
        .candidates
        .listed()
        .iter()
        .map(|candidate| candidate.catalog_name.as_str())
        .collect()
}

fn reason(binding: &EntityBinding) -> UnboundReason {
    binding
        .unresolved()
        .expect("binding did not resolve")
        .reason
}

// --- inputs are valid by construction -------------------------------------

#[test]
fn an_unread_book_is_an_error_not_an_empty_report() {
    // Binding against a book nobody read is the failure this contract exists
    // to prevent, so it cannot be expressed as "everything is missing".
    let empty: [&str; 0] = [];
    assert_eq!(
        MasterCatalog::new(MasterClass::Ledger, empty),
        Err(MasterBindingError::CatalogEmpty)
    );
}

#[test]
fn a_duplicate_master_name_refuses_the_catalog() {
    assert_eq!(
        MasterCatalog::new(MasterClass::Ledger, ["Alpha Traders", "Alpha Traders"]),
        Err(MasterBindingError::CatalogDuplicateName)
    );
}

#[test]
fn unusable_names_are_refused_at_the_boundary() {
    assert_eq!(
        MasterCatalog::new(MasterClass::Ledger, ["   "]),
        Err(MasterBindingError::NameBlank)
    );
    assert_eq!(
        MasterCatalog::new(MasterClass::Ledger, ["Alpha\u{7}Traders"]),
        Err(MasterBindingError::NameUnsafe)
    );
    let long = "A".repeat(MAX_NAME_CHARS + 1);
    assert_eq!(
        MasterCatalog::new(MasterClass::Ledger, [long.as_str()]),
        Err(MasterBindingError::NameTooLong)
    );
    assert_eq!(SourceEntity::new(0, ""), Err(MasterBindingError::NameBlank));
}

#[test]
fn the_measured_transformations_compose() {
    // Twelve single-axis results license each transformation alone and say
    // nothing about applying several at once — which is what a canonical form
    // does on every comparison. Two reviewers raised that independently, so
    // §9.4d measured it rather than arguing it: eight composed variants, all
    // matched, all confirmed by day-book readback against the intended master.
    let catalog = ledgers(&[
        "MB PILOT ALPHA (5550001001)",
        "MB-PROBE-LEDGER-A",
        "Beta Supply",
    ]);
    for (supplied, expected) in [
        (
            "  mb pilot alpha (5550001001)  ",
            "MB PILOT ALPHA (5550001001)",
        ),
        ("mb-pilot-alpha-(5550001001)", "MB PILOT ALPHA (5550001001)"),
        (
            "  mb-pilot-alpha-(5550001001)  ",
            "MB PILOT ALPHA (5550001001)",
        ),
        (
            "MB/PILOT  ALPHA (5550001001)",
            "MB PILOT ALPHA (5550001001)",
        ),
        ("mb-pilot alpha/(5550001001)", "MB PILOT ALPHA (5550001001)"),
        (
            "  mb-pilot/alpha  (5550001001) ",
            "MB PILOT ALPHA (5550001001)",
        ),
        ("  mb probe  ledger a ", "MB-PROBE-LEDGER-A"),
    ] {
        assert_eq!(
            bind_one_name(&catalog, supplied).bound_name(),
            Some(expected),
            "{supplied:?} did not compose to {expected:?}"
        );
    }

    // Composition does not create equivalences out of unmeasured parts: an en
    // dash stays rejected however much measured folding surrounds it.
    let dashed = ledgers(&["Alpha \u{2013} Traders", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&dashed, "  alpha   traders ").bound_name(),
        None,
        "an unmeasured transformation was carried in by composition"
    );
}

#[test]
fn surrounding_and_repeated_whitespace_is_folded_on_both_sides() {
    // §9.4d: leading whitespace, one trailing space and a collapsed internal
    // run all matched on licensed 7.1, in both directions.
    for (master, source) in [
        ("Alpha Traders", "Alpha Traders "),
        ("Alpha Traders ", "Alpha Traders"),
        ("Alpha Traders", "  Alpha Traders"),
        ("  Alpha Traders", "Alpha Traders"),
        ("Alpha Traders", "Alpha  Traders"),
        ("Alpha  Traders", "Alpha Traders"),
    ] {
        let catalog = ledgers(&[master, "Beta Supply"]);
        assert_eq!(
            bind_one_name(&catalog, source).bound_name(),
            Some(master),
            "{source:?} did not reach {master:?}"
        );
    }

    // An observed name is still retained byte for byte: a caller writes it back.
    let catalog = ledgers(&["Alpha Traders ", "Beta Supply"]);
    assert_eq!(catalog.names().next(), Some("Alpha Traders "));
    let binding = bind_one_name(&catalog, "Alpha Traders");
    assert_eq!(binding.bound_name(), Some("Alpha Traders "));
    assert_eq!(
        binding.source_name, "Alpha Traders",
        "a source name is recorded as the document wrote it"
    );

    // ASCII space only. A no-break space was never sent, so it stays an
    // ordinary character rather than joining the separator set on the strength
    // of looking like whitespace.
    let nbsp = ledgers(&["Alpha\u{a0}Traders", "Beta Supply"]);
    assert_eq!(bind_one_name(&nbsp, "Alpha Traders").bound_name(), None);
}

#[test]
fn near_identical_masters_are_an_ambiguity_where_they_collide_and_never_a_refused_catalog() {
    // A catalog holding two names one fold or another could merge must not fail
    // the whole read. Whether they are *ambiguous* is a separate question, and
    // the answer changed when the fold was held to what §9.4b measured.

    // Case-only siblings do collide: ASCII case folding is verified, so both
    // answer to one key and a third spelling resolves to neither.
    let cased = ledgers(&["Alpha Traders", "alpha traders"]);
    assert_eq!(
        bind_one_name(&cased, "Alpha Traders").bound_name(),
        Some("Alpha Traders"),
        "byte equality still picks the exact one"
    );
    let other = bind_one_name(&cased, "ALPHA TRADERS");
    assert_eq!(reason(&other), UnboundReason::NameAmbiguous);
    assert_eq!(candidate_names(&other), ["Alpha Traders", "alpha traders"]);

    // Two masters differing only in trailing whitespace collapse under the
    // measured fold too, so they are an ambiguity rather than a refused
    // catalog — and byte equality still picks one where the source has it.
    let spaced = ledgers(&["Alpha Traders", "Alpha Traders "]);
    assert_eq!(
        bind_one_name(&spaced, "Alpha Traders ").bound_name(),
        Some("Alpha Traders "),
        "byte equality outranks the fold"
    );
    let ambiguous = bind_one_name(&spaced, "alpha traders");
    assert_eq!(reason(&ambiguous), UnboundReason::NameAmbiguous);
    assert_eq!(
        candidate_names(&ambiguous),
        ["Alpha Traders", "Alpha Traders "]
    );
}

#[test]
fn an_identifier_hint_that_yields_nothing_is_refused_rather_than_ignored() {
    assert_eq!(
        SourceEntity::with_identifier_hints(0, "Alpha Traders", ["not-an-identifier"]),
        Err(MasterBindingError::IdentifierHintUnusable)
    );
    let entity = SourceEntity::with_identifier_hints(0, "Alpha Traders", ["5550000001"])
        .expect("a usable hint is accepted");
    assert_eq!(
        entity.identifiers(),
        [Identifier {
            kind: IdentifierKind::Numeric,
            value: "5550000001".to_string(),
        }]
    );
}

#[test]
fn the_source_entity_bound_is_enforced_where_a_document_is_unbounded() {
    let catalog = ledgers(&["Alpha Traders"]);
    let entities = (0..=MAX_SOURCE_ENTITIES)
        .map(|position| SourceEntity::new(position, "Alpha Traders").expect("valid"))
        .collect::<Vec<_>>();
    assert_eq!(
        bind(&catalog, &entities),
        Err(MasterBindingError::TooManySourceEntities)
    );
}

// --- what binds ------------------------------------------------------------

#[test]
fn an_exact_name_binds() {
    let catalog = ledgers(&["Alpha Traders", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Alpha Traders");
    assert_eq!(
        binding.status,
        BindingStatus::Bound {
            catalog_name: "Alpha Traders".to_string(),
            basis: BindingBasis::ExactName,
        }
    );
}

#[test]
fn case_binds_but_an_unverified_fold_only_suggests() {
    // ASCII case folding is measured, so it resolves.
    let cased = ledgers(&["Alpha Traders", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&cased, "ALPHA traders").status,
        BindingStatus::Bound {
            catalog_name: "Alpha Traders".to_string(),
            basis: BindingBasis::NormalizedName,
        }
    );

    // An en dash, a collapsed whitespace run and leading whitespace are all on
    // §9.4b's unverified list. The wide fold still reaches the master, so it is
    // offered — a candidate a human confirms, which is exactly what §9.4b says
    // a looser fold is for. Nothing is lost here except the automatic answer.
    let catalog = ledgers(&["Alpha \u{2013} Traders", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "  alpha - TRADERS  ");
    assert_eq!(binding.bound_name(), None);
    assert_eq!(reason(&binding), UnboundReason::NearMiss);
    assert_eq!(
        binding.unresolved().expect("unbound").candidates.listed(),
        [Candidate {
            catalog_name: "Alpha \u{2013} Traders".to_string(),
            rule: CandidateRule::NormalizedEqual,
        }]
    );
}

#[test]
fn an_embedded_identifier_beats_three_wrong_name_candidates() {
    // The engagement case, fabricated: the source names the party one way, the
    // ledger another, and the only thing that agrees is the number the
    // operator buried in the ledger name. Name matching offers three wrong
    // people; the identifier decides.
    let catalog = ledgers(&[
        "GAMMA (5550000001)",
        "GAMMA ALPHA",
        "GAMMA BETA",
        "GAMMA DELTA",
    ]);
    let source =
        SourceEntity::with_identifier_hints(3, "GAMMA. EPSILON", ["5550000001"]).expect("valid");
    let report = bound(&catalog, &[source]);
    assert_eq!(
        report.entities()[0].status,
        BindingStatus::Bound {
            catalog_name: "GAMMA (5550000001)".to_string(),
            basis: BindingBasis::Identifier,
        }
    );
}

#[test]
fn an_identifier_inside_both_names_binds_without_a_hint() {
    let catalog = ledgers(&["GAMMA (5550000001)", "GAMMA ALPHA"]);
    let binding = bind_one_name(&catalog, "5550000001 GAMMA EPSILON");
    assert_eq!(binding.bound_name(), Some("GAMMA (5550000001)"));
}

#[test]
fn a_punctuated_stock_code_binds_to_its_unpunctuated_form() {
    let catalog = MasterCatalog::new(
        MasterClass::StockItem,
        ["PH-01A-B00", "PH-02A-B00", "Labour Placeholder"],
    )
    .expect("valid");
    let binding = bound(&catalog, &[entity("PH01AB00")])
        .entities()
        .first()
        .cloned()
        .expect("one binding");
    assert_eq!(
        binding.status,
        BindingStatus::Bound {
            catalog_name: "PH-01A-B00".to_string(),
            basis: BindingBasis::Identifier,
        }
    );
}

// --- what refuses to bind --------------------------------------------------

#[test]
fn one_identifier_carried_by_two_masters_is_ambiguous_never_a_bind() {
    let catalog = ledgers(&["ALPHA (5550000001)", "BETA (5550000001)", "GAMMA Supply"]);
    let binding = bind_one_name(&catalog, "PARTY 5550000001");
    assert_eq!(reason(&binding), UnboundReason::IdentifierConflict);
    assert_eq!(
        candidate_names(&binding),
        ["ALPHA (5550000001)", "BETA (5550000001)"]
    );
}

#[test]
fn an_identifier_and_an_exact_name_pointing_apart_is_shown_not_decided() {
    let catalog = ledgers(&["ALPHA (5550000001)", "BETA Supply"]);
    let source =
        SourceEntity::with_identifier_hints(0, "BETA Supply", ["5550000001"]).expect("valid");
    let report = bound(&catalog, &[source]);
    let binding = &report.entities()[0];
    assert_eq!(reason(binding), UnboundReason::IdentifierNameConflict);
    assert!(candidate_names(binding).contains(&"ALPHA (5550000001)"));
    assert!(candidate_names(binding).contains(&"BETA Supply"));
}

#[test]
fn near_duplicate_masters_produce_candidates_and_choose_none() {
    // Three masters differing by one character and word order. The operator
    // decides; the module only shows the field.
    let catalog = ledgers(&["ALPHA SALE", "ALPHA SALES", "SALES - ALPHA", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "ALPHA");
    assert_eq!(reason(&binding), UnboundReason::NearMiss);
    assert_eq!(
        candidate_names(&binding),
        ["ALPHA SALE", "ALPHA SALES", "SALES - ALPHA"]
    );
    let unresolved = binding.unresolved().expect("unbound");
    assert_eq!(
        unresolved
            .candidates
            .listed()
            .iter()
            .map(|candidate| candidate.rule)
            .collect::<Vec<_>>(),
        [
            CandidateRule::CatalogPrefix,
            CandidateRule::CatalogPrefix,
            CandidateRule::SharedToken
        ]
    );
    assert_eq!(unresolved.candidates.found(), 3);
    assert!(!unresolved.candidates.is_incomplete());
}

#[test]
fn a_single_candidate_still_does_not_bind() {
    // Four near-misses of ledgers that already existed rejected 61 vouchers.
    // Uniqueness of a guess is not evidence.
    let catalog = ledgers(&["ALPHA TRADING COMPANY", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "ALPHA TRADING COMP");
    assert_eq!(reason(&binding), UnboundReason::NearMiss);
    assert_eq!(candidate_names(&binding), ["ALPHA TRADING COMPANY"]);
    assert_eq!(binding.bound_name(), None);
}

#[test]
fn a_truncated_source_name_surfaces_the_longer_master() {
    let catalog = ledgers(&["DELTA WHOLESALE PLACEHOLDER", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "DELTA WHOLESALE PL");
    assert_eq!(
        binding
            .unresolved()
            .expect("unbound")
            .candidates
            .listed()
            .first()
            .map(|candidate| candidate.rule),
        Some(CandidateRule::CatalogPrefix)
    );
}

#[test]
fn a_source_name_extending_a_master_surfaces_the_shorter_master() {
    let catalog = ledgers(&["DELTA WHOLESALE", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "DELTA WHOLESALE PLACEHOLDER BRANCH");
    let unresolved = binding.unresolved().expect("unbound");
    assert!(unresolved
        .candidates
        .listed()
        .iter()
        .any(|candidate| candidate.rule == CandidateRule::SourcePrefix
            && candidate.catalog_name == "DELTA WHOLESALE"));
}

#[test]
fn nothing_defensible_is_unmatched_with_no_candidate() {
    let catalog = ledgers(&["Alpha Traders", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Zeta Placeholder");
    assert!(matches!(binding.status, BindingStatus::Unmatched(_)));
    assert_eq!(reason(&binding), UnboundReason::NoCandidate);
    assert!(candidate_names(&binding).is_empty());
    assert_eq!(
        reason(&binding).safe_reason_code(),
        "master_binding_no_candidate"
    );
}

// --- the identifier rules fail closed --------------------------------------

#[test]
fn a_date_shaped_run_is_not_an_identifier() {
    // Two unrelated period-labelled masters must not fuse on their period.
    let catalog = ledgers(&["ALPHA 2026-09-10", "BETA 2026-09-10", "Gamma Supply"]);
    let binding = bind_one_name(&catalog, "DELTA 2026-09-10");
    assert!(binding.unresolved().is_some());
    assert!(binding
        .unresolved()
        .expect("unbound")
        .unresolved_identity
        .is_empty());
}

#[test]
fn a_short_digit_run_is_not_an_identifier() {
    // A masked last-four cannot bind two accounts that share four digits.
    let catalog = ledgers(&["ALPHA BANK CA 2129", "BETA BANK CA 2129"]);
    let binding = bind_one_name(&catalog, "GAMMA BANK CA 2129");
    assert!(binding
        .unresolved()
        .expect("unbound")
        .unresolved_identity
        .is_empty());
}

#[test]
fn separated_digit_groups_do_not_fuse_into_an_identifier() {
    let entity = entity("ALPHA 5550 0000 01");
    assert!(entity.identifiers().is_empty());
}

// --- the review findings, pinned -------------------------------------------

#[test]
fn a_byte_exact_name_carrying_a_number_is_reported_exact_not_identifier() {
    // The write gate admits `ExactName` only. Reporting `Identifier` when the
    // two agree made every ledger with a number in its name permanently
    // unimportable — the exact population this contract exists to serve.
    let catalog = ledgers(&["GAMMA (5550000001)", "GAMMA ALPHA"]);
    let binding = bind_one_name(&catalog, "GAMMA (5550000001)");
    assert_eq!(
        binding.status,
        BindingStatus::Bound {
            catalog_name: "GAMMA (5550000001)".to_string(),
            basis: BindingBasis::ExactName,
        }
    );
}

#[test]
fn a_byte_exact_name_binds_even_when_its_identifier_is_shared() {
    // Found by seeding two live ledgers that share an embedded number, which no
    // fabricated fixture had combined. Refusing a name that exactly names one
    // master makes that ledger permanently unimportable.
    let catalog = ledgers(&[
        "MB PARTY DELTA (5550001009)",
        "MB PARTY EPSILON (5550001009)",
        "Beta Supply",
    ]);
    let binding = bind_one_name(&catalog, "MB PARTY DELTA (5550001009)");
    assert_eq!(
        binding.status,
        BindingStatus::Bound {
            catalog_name: "MB PARTY DELTA (5550001009)".to_string(),
            basis: BindingBasis::ExactName,
        }
    );
    // The shared identifier alone, with no exact name, still refuses.
    let source =
        SourceEntity::with_identifier_hints(0, "SOME PARTY", ["5550001009"]).expect("valid");
    let report = bound(&catalog, &[source]);
    assert_eq!(
        report.entities()[0].unresolved().expect("unbound").reason,
        UnboundReason::IdentifierConflict
    );
}

#[test]
fn a_decisive_identifier_pointing_elsewhere_still_outranks_a_byte_exact_name() {
    let catalog = ledgers(&["ALPHA (5550000001)", "BETA Supply"]);
    let source =
        SourceEntity::with_identifier_hints(0, "BETA Supply", ["5550000001"]).expect("valid");
    let report = bound(&catalog, &[source]);
    assert_eq!(
        report.entities()[0].unresolved().expect("unbound").reason,
        UnboundReason::IdentifierNameConflict
    );
}

#[test]
fn space_hyphen_and_slash_are_one_separator_in_both_directions() {
    // `TALLY_PROTOCOL_REFERENCE.md` §9.4d, measured on licensed TallyPrime 7.1
    // by naming each spelling in a voucher and reading the day book back to see
    // which master it reached. Both hyphen directions matched, and so did a
    // slash — so this fold is symmetric, and one key per side is enough.
    for (master, source) in [
        ("BRIDGE-PROBE-LEDGER-A", "BRIDGE PROBE LEDGER A"),
        ("BRIDGE PROBE LEDGER A", "BRIDGE-PROBE-LEDGER-A"),
        ("BRIDGE PROBE LEDGER A", "BRIDGE/PROBE/LEDGER/A"),
        ("BRIDGE/PROBE/LEDGER/A", "BRIDGE-PROBE-LEDGER-A"),
    ] {
        let catalog = ledgers(&[master, "Beta Supply"]);
        assert_eq!(
            bind_one_name(&catalog, source).bound_name(),
            Some(master),
            "{source} did not reach {master}"
        );
    }

    // `X - Y` is a common ledger convention, and it needs the separator step
    // and the whitespace-run step together. Both are measured, so it resolves.
    let spaced_hyphen = ledgers(&["Bank - HDFC Current", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&spaced_hyphen, "Bank HDFC Current").bound_name(),
        Some("Bank - HDFC Current")
    );

    // An en dash and an underscore were **sent and rejected**. They are not
    // separators to Tally, however much they look like them, so they may only
    // suggest — this is the half of §9.4d that a "normalises separators"
    // reading would get wrong in the dangerous direction.
    for (master, source) in [
        ("Alpha \u{2013} Traders", "Alpha Traders"),
        ("Alpha_Traders", "Alpha Traders"),
    ] {
        let catalog = ledgers(&[master, "Beta Supply"]);
        let binding = bind_one_name(&catalog, source);
        assert_eq!(
            binding.bound_name(),
            None,
            "{source} resolved onto {master} on an equivalence Tally rejects"
        );
        assert_eq!(candidate_names(&binding), [master]);
    }
}

#[test]
fn masters_that_collapse_under_the_fold_are_refused_never_chosen() {
    // `TALLY_PROTOCOL_REFERENCE.md` §9.4b requires prefer-exact,
    // refuse-ambiguous, never-pick. `A-B` and `A B` collapse under the three
    // verified transformations and nothing measured says which one Tally would
    // choose, so a fold that returns the first match is the failure mode.
    let catalog = ledgers(&["Alpha-Beta", "Alpha Beta", "Gamma"]);

    // Prefer-exact: byte equality outranks a key shared by two masters.
    assert_eq!(
        bind_one_name(&catalog, "Alpha Beta").bound_name(),
        Some("Alpha Beta")
    );
    assert_eq!(
        bind_one_name(&catalog, "Alpha-Beta").bound_name(),
        Some("Alpha-Beta")
    );

    // Refuse-ambiguous, never-pick: with no exact spelling to prefer, the
    // collapse is reported with both masters offered, not resolved to one.
    // Both spellings answer to one key now that §9.4d has measured the hyphen
    // in both directions, so a third spelling reaching both is an ambiguity —
    // and Tally agrees, because it would match that name to either.
    for spelling in ["alpha beta", "ALPHA  BETA", "alpha/beta"] {
        let binding = bind_one_name(&catalog, spelling);
        assert_eq!(reason(&binding), UnboundReason::NameAmbiguous, "{spelling}");
        assert_eq!(binding.bound_name(), None);
        assert_eq!(candidate_names(&binding), ["Alpha Beta", "Alpha-Beta"]);
    }
}

#[test]
fn the_master_fold_stops_where_tally_stops() {
    // `IMPLEMENTATION_GUIDE.md` §3.3b also measured what Tally does NOT
    // normalise: `AND` for `&`, a missing suffix word, and a singular for a
    // plural were all rejected. The first two were re-measured on licensed
    // 7.1 in §9.4d against `Profit & Loss A/c` and rejected there too.
    // Folding further than the authority would bind names Tally refuses.
    let catalog = ledgers(&["ZZ Ram & Sons Pvt Ltd", "Beta Supply"]);
    for wrong in [
        "ZZ Ram AND Sons Pvt Ltd",
        "ZZ Ram & Sons",
        "ZZ Ram & Son Pvt Ltd",
    ] {
        assert_eq!(
            bind_one_name(&catalog, wrong).bound_name(),
            None,
            "{wrong:?} bound, but Tally rejects it"
        );
    }

    // The *absent-master* direction, which prefer-exact and refuse-ambiguous
    // do not cover: the requested master is not in the book and one different
    // ledger collapses onto the request, so there is one candidate and no
    // ambiguity to refuse. Uniqueness under a fold is only as meaningful as
    // the fold, and `&` has to stay significant for this to hold.
    for (requested, present) in [("A & B", "AB"), ("AB", "A & B")] {
        let only = ledgers(&[present, "Gamma"]);
        assert_eq!(
            bind_one_name(&only, requested).bound_name(),
            None,
            "{requested:?} bound to {present:?}, which Tally treats as a different master"
        );
    }
}

#[test]
fn a_trailing_space_never_claims_byte_equality() {
    // `Bank ` against live `Bank` must not report exact: the import file would
    // still carry the trailing space. Normalized is the correct, loud outcome —
    // the write gate refuses it.
    let catalog = ledgers(&["Bank"]);
    let binding = bind_one_name(&catalog, "Bank ");
    assert_eq!(
        binding.status,
        BindingStatus::Bound {
            catalog_name: "Bank".to_string(),
            basis: BindingBasis::NormalizedName,
        }
    );
    assert_eq!(
        binding.source_name, "Bank ",
        "the requested value is echoed verbatim"
    );
}

#[test]
fn digits_inside_a_mixed_code_are_not_also_a_standalone_identifier() {
    // Otherwise `Part AB12345678` collides with an unrelated `Bank 12345678`.
    let entity = entity("Part AB12345678");
    assert_eq!(
        entity.identifiers(),
        [Identifier {
            kind: IdentifierKind::Code,
            value: "AB12345678".to_string(),
        }]
    );
    let catalog = ledgers(&["Bank 12345678", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Part AB12345678");
    assert_eq!(
        binding.bound_name(),
        None,
        "a part code must not reach a bank ledger"
    );
}

#[test]
fn a_fiscal_period_label_is_not_an_identity_bearing_code() {
    // Two unrelated ledgers routinely share a period label. Identifier-first
    // matching would otherwise bind the source to whichever one exists before
    // it ever compared the names.
    for label in [
        "FY25",
        "FY2025",
        "AY2026",
        "Q3",
        "H2",
        "PER2026",
        "APR2025",
        "2025Q1",
        "MAR26",
        "H12026",
        // A month name can be any length; a year cannot. Capping the
        // alphabetic run kept losing to longer spellings.
        "SEPTEMBER2025",
        "2025QUARTER1",
        "DECEMBER2026",
        // Ranges: operators write these with the very separators that
        // canonicalization strips, so the test has to see the raw token.
        "FY2025-26",
        "FY2025/26",
        // The comparison key folds these dash variants; the period boundary
        // has to admit the same set or the range fuses instead of splitting.
        "FY2025\u{2013}26",
        "FY2025\u{2014}26",
        "2025\u{2013}2026",
        "2025-2026",
        "APR2025-MAR2026",
        // Written without the separator, the range arrives as one run that
        // every length test above missed, and the token passed as a code.
        "FY202425",
        "FY20242025",
        "AY202526",
        "202425FY",
    ] {
        assert!(
            entity(&format!("Purchases {label}"))
                .identifiers()
                .is_empty(),
            "{label} was treated as a code identifier"
        );
    }
    for label in ["FY2025", "FY202425"] {
        let catalog = ledgers(&[&format!("Sales {label}"), "Beta Supply"]);
        let binding = bind_one_name(&catalog, &format!("Purchases {label}"));
        assert_eq!(
            binding.bound_name(),
            None,
            "the shared period label {label} bound two unrelated ledgers"
        );
    }
    // A genuine identity-bearing code still is one.
    assert_eq!(entity("Item PH01AB00").identifiers().len(), 1);
}

#[test]
fn a_name_in_another_script_does_not_shed_its_letters_into_a_code() {
    // Canonicalization keeps only ASCII, so a Devanagari party name fused to an
    // ASCII suffix yielded the code `AB12345678` — a string the name never
    // contained — and reached an unrelated bank ledger. The ASCII spelling of
    // the same shape never did, which is what makes it a defect rather than a
    // policy: the boundary was an ASCII boundary wearing a general name.
    let party = "\u{92a}\u{93e}\u{930}\u{94d}\u{91f}\u{940}";
    let fused = format!("{party}AB12345678");
    assert!(
        entity(&fused).identifiers().is_empty(),
        "a dropped non-ASCII prefix manufactured a code"
    );
    let catalog = ledgers(&["Bank AB12345678", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&catalog, &fused).bound_name(),
        None,
        "a party name must not reach a bank ledger by shedding its script"
    );
    // The ASCII spelling this is measured against, unchanged: it keeps every
    // letter, so it carries a code of its own and reaches no bank.
    assert_eq!(
        entity("PartyAB12345678").identifiers(),
        [Identifier {
            kind: IdentifierKind::Code,
            value: "PARTYAB12345678".to_string(),
        }]
    );
    assert_eq!(
        bind_one_name(&catalog, "PartyAB12345678").bound_name(),
        None
    );
    // A code standing on its own beside a name in any script is still a code.
    assert_eq!(
        entity(&format!("{party} AB12345678")).identifiers().len(),
        1
    );

    // Letters were the first guard and were too narrow: `char::is_alphabetic`
    // is false for a Devanagari digit, so non-ASCII numerals walked through it
    // and canonicalization dropped them just the same. The numeric branch had
    // the identical hole, which no thread named — a trailing run of Devanagari
    // digits is not alphabetic either, so the ASCII digits before it were
    // emitted as a whole account number.
    let digits = "\u{967}\u{968}\u{969}";
    for fused in [
        format!("Purchases AB{digits}12345678"),
        format!("Purchases 12345678{digits}"),
        format!("Purchases {digits}12345678"),
    ] {
        assert!(
            entity(&fused).identifiers().is_empty(),
            "{fused} manufactured an identifier out of what canonicalization dropped"
        );
    }
    let bank = ledgers(&["Bank AB12345678", "Bank 12345678", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&bank, &format!("Purchases AB{digits}12345678")).bound_name(),
        None
    );
    assert_eq!(
        bind_one_name(&bank, &format!("Purchases 12345678{digits}")).bound_name(),
        None
    );
    // Only the separators §9.4d measured are discarded from a code. An ASCII
    // hyphen and slash are; a non-breaking hyphen is not, because §9.4d sent a
    // non-ASCII dash and watched Tally reject it. So a code punctuated with one
    // no longer agrees with the plain spelling — a refusal, which is the safe
    // direction, and the token is still admitted as a candidate by name.
    assert_eq!(
        entity("Item PH-01/AB-00").identifiers(),
        entity("Item PH01AB00").identifiers()
    );
    assert_ne!(
        entity("Item PH\u{2011}01AB00").identifiers(),
        entity("Item PH01AB00").identifiers()
    );
}

#[test]
fn two_encodings_of_one_name_are_two_masters_to_tally_and_so_to_this() {
    // Measured 2026-08-19 on TallyPrime 7.1: a voucher naming a UI-created
    // ledger in its canonically equivalent NFD spelling was rejected with
    // `EXCEPTIONS=1` and a LINEERROR saying the ledger does not exist, while
    // the NFC spelling created it. Tally stores the bytes it was given and
    // matches on exact codepoints, so these are different masters to Tally and
    // must be different masters here.
    //
    // This is stronger than the UNVERIFIED rows in §9.4b's table: folding it
    // is not unproven, it is proven wrong. It reads like decoding rather than
    // folding, which is why it nearly stayed in the resolving fold.
    let precomposed = "Caf\u{e9} Traders";
    let decomposed = "Cafe\u{301} Traders";
    let catalog = ledgers(&[precomposed, "Beta Supply"]);
    let binding = bind_one_name(&catalog, decomposed);
    assert_eq!(
        binding.bound_name(),
        None,
        "an NFD source name resolved onto an NFC master Tally keeps apart"
    );
    // The wide fold still reaches it, so an operator sees the one master worth
    // looking at rather than nothing at all.
    assert_eq!(candidate_names(&binding), [precomposed]);
    // And the spelling Tally would actually match still binds.
    assert_eq!(
        bind_one_name(&catalog, precomposed).bound_name(),
        Some(precomposed)
    );
}

#[test]
fn a_date_fused_into_a_code_shaped_token_is_still_a_date() {
    // `is_plausible_date` guarded the numeric branch only, and `is_period`
    // never sees this one: its eight-digit case admits a year followed by a
    // year, and `0911` is neither. So `DATED20250911` cleared every code test
    // and two unrelated ledgers bound to each other on a shared date label.
    for label in [
        "DATED20250911",
        "DT20250911",
        "INV20250911",
        "DATED11092025",
    ] {
        assert!(
            entity(&format!("Purchases {label}"))
                .identifiers()
                .is_empty(),
            "{label} was treated as a code identifier"
        );
    }
    let catalog = ledgers(&["Sales DATED20250911", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&catalog, "Purchases DATED20250911").bound_name(),
        None,
        "a shared date label must not bind two unrelated ledgers"
    );
    // A run no calendar would produce is still a code, and a longer run is not
    // a date at all — the test is on eight digits exactly.
    assert_eq!(entity("Item PH01AB00").identifiers().len(), 1);
    assert_eq!(entity("Party AB5550001001").identifiers().len(), 1);
}

#[test]
fn a_catalog_is_bounded_by_total_bytes_and_not_only_by_count() {
    // Both documented bounds can hold while their product does not: 20,000
    // names of 16,384 characters satisfies each and is 327 MB before the
    // constructor builds keys, tokens and four indexes over them. The bound has
    // to be on the aggregate, and has to fire while the iterator is consumed.
    let long = "N".repeat(MAX_NAME_CHARS);
    let many = (0..600)
        .map(|index| format!("{index:04}{}", &long[..MAX_NAME_CHARS - 4]))
        .collect::<Vec<_>>();
    assert_eq!(
        MasterCatalog::new(MasterClass::Ledger, &many),
        Err(MasterBindingError::CatalogTooLarge)
    );
    // A catalog far larger than any observed book still loads: the largest read
    // here is 470 names, so the bound must not be reachable in practice.
    let ordinary = (0..5_000)
        .map(|index| format!("Placeholder Party {index:05}"))
        .collect::<Vec<_>>();
    assert!(MasterCatalog::new(MasterClass::Ledger, &ordinary).is_ok());
}

#[test]
fn repeating_one_source_name_does_not_repeat_the_search_or_change_the_answer() {
    // A draft may name one ledger on every row, and the candidate search is not
    // cheap when the name reaches a family. Remembering it must not change what
    // the report says — the memo is keyed on the source key and the masters its
    // identifiers reached, which is all `collect_candidates` reads.
    let names = (0..60)
        .map(|index| format!("Acme Branch {index:05}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");

    let alone = bind_one_name(&catalog, "Acme Branch");
    let repeated = (0..40)
        .map(|position| SourceEntity::new(position, "Acme Branch").expect("valid"))
        .collect::<Vec<_>>();
    // Counted, not assumed. The first version of this memo was consulted on the
    // conflict and ambiguity paths but not on the ordinary near miss — which is
    // the case this test uses — and every assertion below still passed, because
    // they check the answer rather than the work. `Acme Branch` matches no
    // master exactly, by identifier or by the narrow fold, so all forty entities
    // take that path.
    super::CANDIDATE_SEARCHES.with(|count| count.set(0));
    let report = bound(&catalog, &repeated);
    assert_eq!(
        super::CANDIDATE_SEARCHES.with(std::cell::Cell::get),
        1,
        "forty rows naming one ledger ran the candidate search more than once"
    );
    assert_eq!(report.totals().requested, 40);
    assert_eq!(report.totals().bound, 0);
    for entity in report.entities() {
        assert_eq!(
            entity.status, alone.status,
            "remembering the search changed the answer"
        );
    }

    // Same key, different spelling: one is byte-exact and one is not, and the
    // shared memo must not leak the exact hit into the other's answer.
    let mixed = vec![
        SourceEntity::new(0, "Acme Branch 00007").expect("valid"),
        SourceEntity::new(1, "acme branch 00007").expect("valid"),
    ];
    let report = bound(&catalog, &mixed);
    assert_eq!(report.entities()[0].bound_name(), Some("Acme Branch 00007"));
    assert_eq!(
        report.entities()[1].bound_name(),
        Some("Acme Branch 00007"),
        "a normalized hit is still a hit"
    );

    // Same source *name*, different identifier hints. The key is identical, so
    // a memo keyed on the key alone would hand the second entity the first
    // one's candidates — a different pair of ledgers entirely. This is the case
    // that proves the second half of the memo key, and nothing else reaches it.
    let shared = ledgers(&[
        "Party Alpha (5550001009)",
        "Party Beta (5550001009)",
        "Party Gamma (5550001007)",
        "Party Delta (5550001007)",
    ]);
    let hinted = vec![
        SourceEntity::with_identifier_hints(0, "Zeta Holdings", ["5550001009"]).expect("valid"),
        SourceEntity::with_identifier_hints(1, "Zeta Holdings", ["5550001007"]).expect("valid"),
    ];
    let report = bound(&shared, &hinted);
    assert_eq!(
        candidate_names(&report.entities()[0]),
        ["Party Alpha (5550001009)", "Party Beta (5550001009)"]
    );
    assert_eq!(
        candidate_names(&report.entities()[1]),
        ["Party Delta (5550001007)", "Party Gamma (5550001007)"],
        "the memo handed one entity another's candidates"
    );
}

#[test]
fn a_retained_identity_stays_short_enough_to_write_back() {
    // An unresolved entity carries its identifiers into a fallback so the money
    // can be found later, and the documented way to carry them is a narration —
    // which the import path refuses over 2,000 characters. Unbounded values let
    // `assign_fallback` succeed while producing a tag nobody could write, which
    // fails at the write rather than here.
    let long_run = "5".repeat(400);
    assert!(
        entity(&format!("Party {long_run}"))
            .identifiers()
            .is_empty(),
        "a 400-digit run is not an account number"
    );
    let long_code = format!("AB{}", "7".repeat(400));
    assert!(entity(&format!("Party {long_code}"))
        .identifiers()
        .is_empty());

    // The bound is on the value, so the tag stays complete rather than
    // truncated — a truncated identity is worse than none, because it looks
    // usable. Thirty-two of the longest admitted identifiers still fit.
    let hints = (0..MAX_IDENTIFIERS_PER_NAME)
        .map(|index| format!("{index:02}{}", "5".repeat(MAX_IDENTIFIER_CHARS - 2)))
        .collect::<Vec<_>>();
    let source =
        SourceEntity::with_identifier_hints(0, "Zeta Holdings", hints.iter().map(String::as_str))
            .expect("the longest admitted identifiers are still admitted");
    let catalog = ledgers(&["Alpha Traders", "Beta Supply"]);
    let report = bound(&catalog, &[source]);
    let tag = report
        .assign_fallback(0, &catalog, "Alpha Traders")
        .expect("a fallback in the same catalog")
        .retained_tag();
    assert!(
        tag.len() <= 2_000,
        "a retained identity of {} characters cannot be written back",
        tag.len()
    );
    // Complete, not truncated: every identifier is still in it.
    assert_eq!(tag.matches("numeric:").count(), MAX_IDENTIFIERS_PER_NAME);
}

#[test]
fn an_identifier_held_by_a_whole_family_is_a_conflict_without_expanding_it() {
    // One identifier on more masters than a candidate list may show is already
    // a conflict, and its holders are a family this entity does not separate.
    // Building the set anyway cloned it per source row, before the candidate
    // memo was consulted — the cost is paid on a result nothing can use.
    let names = (0..MAX_CANDIDATES_PER_ENTITY + 5)
        .map(|index| format!("Shared Party {index:03} (5550009999)"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    // Counted, not inferred. Refusing to expand and expanding then refusing
    // produce the same verdict, so only a count can tell them apart.
    super::HOLDER_EXPANSIONS.with(|count| count.set(0));
    let binding = bind_one_name(&catalog, "Zeta Holdings 5550009999");
    assert_eq!(
        super::HOLDER_EXPANSIONS.with(std::cell::Cell::get),
        0,
        "a family larger than any candidate list was expanded anyway"
    );
    assert_eq!(
        binding.bound_name(),
        None,
        "a shared identifier never binds"
    );
    assert_eq!(reason(&binding), UnboundReason::IdentifierConflict);
    // The identity is still reported, so the operator can still find the money.
    assert_eq!(
        binding.unresolved().expect("unbound").unresolved_identity,
        [Identifier {
            kind: IdentifierKind::Numeric,
            value: "5550009999".to_string(),
        }]
    );
}

#[test]
fn a_hint_pointing_elsewhere_outranks_an_exact_name_at_any_family_size() {
    // The large-holder skip added for cost made this invariant size-dependent:
    // below the cap the holder set was built and `identifier_points_elsewhere`
    // saw that it did not contain the exact master; above the cap the set was
    // skipped, the signal went with it, and the exact name bound while the
    // hint pointed entirely elsewhere. Skipping the expansion must not skip
    // the question the expansion was asked.
    let mut names = vec!["Alpha Traders".to_string()];
    names.extend(
        (0..MAX_CANDIDATES_PER_ENTITY + 5)
            .map(|index| format!("Other Party {index:03} (5550008888)")),
    );
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let source =
        SourceEntity::with_identifier_hints(0, "Alpha Traders", ["5550008888"]).expect("valid");
    let report = bound(&catalog, &[source]);
    let binding = &report.entities()[0];
    assert_eq!(
        binding.bound_name(),
        None,
        "a byte-exact name bound while its hint pointed at a different family"
    );
    assert_eq!(reason(binding), UnboundReason::IdentifierNameConflict);

    // Below the cap the same shape already behaved; both sides of the boundary
    // are asserted so the fix cannot regress on one of them alone.
    let mut small = vec!["Alpha Traders".to_string()];
    small.extend((0..3).map(|index| format!("Other Party {index:03} (5550008888)")));
    let small = MasterCatalog::new(MasterClass::Ledger, &small).expect("valid");
    let source =
        SourceEntity::with_identifier_hints(0, "Alpha Traders", ["5550008888"]).expect("valid");
    let report = bound(&small, &[source]);
    assert_eq!(
        reason(&report.entities()[0]),
        UnboundReason::IdentifierNameConflict
    );
}

#[test]
fn a_date_range_is_dates_even_after_its_separator_is_removed() {
    // `20250911-20250912` fuses to sixteen digits, which is no length
    // `is_plausible_date` recognizes, and `is_period` reads neither half as a
    // year range. So a date *range* walked through a guard a single date does
    // not — the fusing is what hid the components, so they are checked first.
    for range in [
        "20250911-20250912",
        "20250911/20250912",
        "01012026-02012026",
        "20250911-20250912-20250913",
    ] {
        assert!(
            entity(&format!("Purchases {range}"))
                .identifiers()
                .is_empty(),
            "{range} was treated as an identifier"
        );
    }
    let catalog = ledgers(&["Sales 20250911-20250912", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&catalog, "Purchases 20250911-20250912").bound_name(),
        None,
        "a shared date range must not bind two unrelated ledgers"
    );
    // A punctuated account number is untouched: no component reads as a date.
    assert_eq!(entity("Party 5550001-002").identifiers().len(), 1);
}

#[test]
fn a_stock_item_may_suggest_on_a_fold_but_not_resolve_on_one() {
    // §9.4d measured **ledgers**. Whether stock items match by the same rule
    // was never sent, so the same folded pair that resolves for a ledger may
    // only be offered for a stock item.
    let folded = ["Sales-Item", "Beta Supply"];
    let ledger = MasterCatalog::new(MasterClass::Ledger, folded).expect("valid");
    assert_eq!(
        bind_one_name(&ledger, "sales item").bound_name(),
        Some("Sales-Item")
    );

    let items = MasterCatalog::new(MasterClass::StockItem, folded).expect("valid");
    let source = SourceEntity::new(0, "sales item").expect("valid");
    let report = bind(&items, &[source]).expect("valid");
    let binding = &report.entities()[0];
    assert_eq!(
        binding.bound_name(),
        None,
        "a stock item resolved on an unmeasured fold"
    );
    assert_eq!(candidate_names(binding), ["Sales-Item"]);
    // A lone unlicensed fold is a **near miss**, not an ambiguity. Nothing
    // shares its key; one master simply did not qualify, and `NameAmbiguous`
    // would tell a consumer several masters collided — a different fact with a
    // different remedy.
    assert_eq!(reason(binding), UnboundReason::NearMiss);

    // Byte equality needs no fold and is unaffected by the class.
    let exact = SourceEntity::new(0, "Sales-Item").expect("valid");
    let report = bind(&items, &[exact]).expect("valid");
    assert_eq!(report.entities()[0].bound_name(), Some("Sales-Item"));
}

#[test]
fn a_repeated_key_is_remembered_however_many_distinct_ones_precede_it() {
    // The entry cap made the memo's protection depend on **source order**:
    // enough distinct cheap misses at the head of a draft filled it, and the
    // repeated expensive key behind them was then never cached — the stall the
    // memo exists to prevent, reachable by reordering the same rows.
    let names = (0..60)
        .map(|index| format!("Acme Branch {index:05}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");

    let mut entities = (0..MAX_CANDIDATE_MEMO_ENTRIES)
        .map(|index| SourceEntity::new(index, &format!("Distinct Miss {index:05}")).expect("valid"))
        .collect::<Vec<_>>();
    entities.extend((0..4).map(|offset| {
        SourceEntity::new(MAX_CANDIDATE_MEMO_ENTRIES + offset, "Acme Branch").expect("valid")
    }));

    super::CANDIDATE_SEARCHES.with(|count| count.set(0));
    let report = bound(&catalog, &entities);
    let searches = super::CANDIDATE_SEARCHES.with(std::cell::Cell::get);
    assert_eq!(report.totals().requested, MAX_CANDIDATE_MEMO_ENTRIES + 4);
    // One search for the repeated key, not four. The distinct misses each cost
    // one of their own, so the total is bounded by the distinct count plus one.
    assert!(
        searches <= MAX_CANDIDATE_MEMO_ENTRIES + 1,
        "the repeated key was searched more than once: {searches} searches"
    );
}

#[test]
fn unmeasured_punctuation_keeps_two_codes_apart() {
    // Canonicalization filtered to alphanumerics, so **every** ASCII
    // punctuation mark was discarded and `AB_123456` canonicalized the same as
    // `AB-123456` — while §9.4d had sent an underscore at a live master and
    // watched Tally reject it. The fold's evidence is about hyphens and
    // slashes; everything else is content.
    let catalog = ledgers(&["Sales AB-123456", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Purchases AB_123456");
    assert_eq!(
        binding.bound_name(),
        None,
        "an underscore was treated as a hyphen on evidence that says it is not"
    );
    // The measured separators still agree, and the contrast is the point: with
    // a hyphen the two codes are one identifier and the bind is the
    // identifier-first rule working; with an underscore they are two
    // identifiers and nothing binds.
    let measured = ledgers(&["Sales PH-01-AB-00", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&measured, "Purchases PH01AB00").bound_name(),
        Some("Sales PH-01-AB-00"),
        "a hyphen and no hyphen are one code, which §9.4d measured"
    );
    assert_eq!(
        entity("Item PH-01-AB-00").identifiers(),
        entity("Item PH01AB00").identifiers()
    );
    assert_ne!(
        entity("Item AB_123456").identifiers(),
        entity("Item AB-123456").identifiers()
    );
}

#[test]
fn a_withheld_family_still_reports_how_many_share_the_identifier() {
    // Skipping the expansion discarded the holder count with the set, so a
    // withheld family reported `found() == 0` and an empty listing — telling
    // the operator nothing shares the identifier when hundreds do. The count is
    // the one thing a reader still needs from a set too large to show.
    let names = (0..MAX_CANDIDATES_PER_ENTITY + 7)
        .map(|index| format!("Shared Party {index:03} (5550007777)"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let source =
        SourceEntity::with_identifier_hints(0, "Zeta Holdings", ["5550007777"]).expect("valid");
    let report = bound(&catalog, &[source]);
    let unresolved = report.entities()[0].unresolved().expect("unbound");
    assert_eq!(unresolved.reason, UnboundReason::IdentifierConflict);
    assert_eq!(
        unresolved.candidates.found(),
        MAX_CANDIDATES_PER_ENTITY + 7,
        "a withheld family reported no holders at all"
    );
    assert!(
        unresolved.candidates.is_incomplete(),
        "a count without a listing must say the listing is incomplete"
    );
}

#[test]
fn a_report_bounds_its_own_candidate_allocation() {
    // A per-entity cap does not bound a report: the clones exist the moment it
    // is built, and a consumer capping its own copy afterwards bounds only the
    // copy. The budget is spent in entity order; entities past it keep their
    // true count and flag truncation.
    let long = "Z".repeat(400);
    let names = (0..30)
        .map(|index| format!("SHARED PREFIX {index:03} {long}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let entities = (0..2_000)
        .map(|position| SourceEntity::new(position, "SHARED PREFIX 001").expect("valid"))
        .collect::<Vec<_>>();
    let report = bound(&catalog, &entities);
    let listed: usize = report
        .unbound()
        .filter_map(|entity| entity.unresolved())
        .map(|unresolved| {
            unresolved
                .candidates
                .listed()
                .iter()
                .map(|candidate| candidate.catalog_name.len())
                .sum::<usize>()
        })
        .sum();
    assert!(
        listed <= MAX_REPORT_CANDIDATE_BYTES,
        "report allocated {listed} candidate bytes"
    );
    let starved = report
        .unbound()
        .filter_map(|entity| entity.unresolved())
        .filter(|unresolved| unresolved.candidates.listed().is_empty())
        .collect::<Vec<_>>();
    assert!(!starved.is_empty(), "the budget must actually bite here");
    assert!(starved.iter().all(
        |unresolved| unresolved.candidates.found() > 0 && unresolved.candidates.is_incomplete()
    ));
}

#[test]
fn a_token_carrying_letters_never_yields_a_standalone_number() {
    // A one-letter token fails the code test, and its digits were then escaping
    // as a numeric of their own — so `Part A12345678` could reach an unrelated
    // `Bank 12345678`. A token identifies by its whole shape or not at all.
    assert!(entity("Part A12345678").identifiers().is_empty());
    let catalog = ledgers(&["Bank 12345678", "Beta Supply"]);
    assert_eq!(bind_one_name(&catalog, "Part A12345678").bound_name(), None);
    // A bare digit run beside no letters is still an identifier.
    assert_eq!(entity("Party (5550001001)").identifiers().len(), 1);
}

#[test]
fn conflicting_identifiers_outrank_a_byte_exact_name_but_a_shared_one_does_not() {
    // Two hints selecting two other masters is conflicting evidence, and
    // binding the name silently discarded it. A *shared* identifier is
    // different: the ambiguous set still contains the master the name spells,
    // so the name is what separates it from its siblings.
    let catalog = ledgers(&["ACME", "BETA 11111111", "GAMMA 22222222"]);
    let source =
        SourceEntity::with_identifier_hints(0, "ACME", ["11111111", "22222222"]).expect("valid");
    let report = bound(&catalog, &[source]);
    let binding = &report.entities()[0];
    assert_eq!(reason(binding), UnboundReason::IdentifierNameConflict);
    // Every master the evidence reached is offered, so the operator sees the
    // disagreement rather than one side of it — and the byte-exact name leads,
    // labelled as itself. This refusal exists *because* byte equality was
    // observed, so burying that under the identifier that outranked it left
    // the operator reading two facts without being told one of them was exact.
    assert_eq!(
        candidate_names(binding),
        ["ACME", "BETA 11111111", "GAMMA 22222222"]
    );
    assert_eq!(
        binding.unresolved().expect("unbound").candidates.listed()[0].rule,
        CandidateRule::ExactName
    );

    // The shared-identifier case must keep binding: one identifier reached the
    // master the name spells along with its sibling, and the name separates
    // them.
    let shared = ledgers(&[
        "MB PARTY DELTA (5550001009)",
        "MB PARTY EPSILON (5550001009)",
    ]);
    assert_eq!(
        bind_one_name(&shared, "MB PARTY DELTA (5550001009)").bound_name(),
        Some("MB PARTY DELTA (5550001009)")
    );

    // The mixed case, which a union test answers wrongly: the exact master is
    // in the union because its own number is one of the identifiers, while a
    // second identifier plainly reaches somewhere else. Provenance per
    // identifier is the only thing that separates this from the shared case.
    let mixed = ledgers(&["ACME 11111111", "BETA 22222222"]);
    let source =
        SourceEntity::with_identifier_hints(0, "ACME 11111111", ["22222222"]).expect("valid");
    let report = bound(&mixed, &[source]);
    assert_eq!(
        reason(&report.entities()[0]),
        UnboundReason::IdentifierNameConflict
    );
}

#[test]
fn an_indic_name_is_not_torn_apart_at_its_joins() {
    // `char::is_alphanumeric` is false for a Devanagari virama — the halant
    // that joins consonants — and false for a nukta. Splitting on "not
    // alphanumeric" cut these names at the joins, so a shared word stopped
    // being a shared token. These names are in the books this binder reads.
    let catalog = ledgers(&[
        "\u{936}\u{94d}\u{930}\u{940} \u{917}\u{923}\u{947}\u{936} \u{91f}\u{94d}\u{930}\u{947}\u{921}\u{930}\u{94d}\u{938}",
        "\u{930}\u{93e}\u{92f} \u{90f}\u{923}\u{94d}\u{921} \u{938}\u{928}\u{94d}\u{938}",
        "Beta Supply",
    ]);
    // The second book's distinctive word, which the virama used to fragment
    // away entirely, now reaches its own master.
    let binding = bind_one_name(&catalog, "\u{938}\u{928}\u{94d}\u{938}");
    assert!(
        candidate_names(&binding)
            .iter()
            .any(|name| name.contains("\u{930}\u{93e}\u{92f}")),
        "a shared Indic word did not surface its master: {:?}",
        candidate_names(&binding)
    );
}

#[test]
fn a_non_ascii_name_beside_digits_is_still_a_name() {
    // The observed books carry Devanagari, Tamil and Bengali ledger names. An
    // ASCII-only letter guard read `पार्टी12345678` as digits standing alone
    // and bound a party to an unrelated bank ledger.
    let party = "\u{92a}\u{93e}\u{930}\u{94d}\u{91f}\u{940}12345678";
    assert!(entity(party).identifiers().is_empty());
    let catalog = ledgers(&["Bank 12345678", "Beta Supply"]);
    assert_eq!(bind_one_name(&catalog, party).bound_name(), None);
}

#[test]
fn a_masked_value_identifies_nothing() {
    // `XXXXX1234X` clears every length and composition test while carrying only
    // a last four that any number of parties share.
    assert!(entity("Purchases XXXXX1234X").identifiers().is_empty());
    let catalog = ledgers(&["Sales XXXXX1234X", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&catalog, "Purchases XXXXX1234X").bound_name(),
        None
    );
    // Distinct letters are what an identity-bearing code has and a mask does not.
    assert_eq!(entity("Item PH01AB00").identifiers().len(), 1);

    // A mask spelled with punctuation reaches the numeric branch instead, where
    // every non-digit is an ordinary delimiter — so `********12345678` split
    // cleanly and offered its visible suffix as though it were the account.
    for masked in ["Purchases ********12345678", "Purchases ####12345678"] {
        assert!(
            entity(masked).identifiers().is_empty(),
            "{masked} exposed its suffix as an identifier"
        );
    }
    let punctuated = ledgers(&["Sales ********12345678", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&punctuated, "Purchases ********12345678").bound_name(),
        None
    );
    // A mask and the digits it hides are often written apart; that is the same
    // statement, and reading tokens independently lost the relationship.
    for separated in ["Purchases **** 12345678", "Purchases #### 12345678"] {
        assert!(
            entity(separated).identifiers().is_empty(),
            "{separated} exposed its suffix as an identifier"
        );
    }
    // A mask spelled with letters is the same statement as one spelled with
    // punctuation, and it too is written apart from the digits it hides. Read
    // token by token, `XXXX` failed the punctuation test and the visible suffix
    // escaped as a whole account number.
    for separated in [
        "Purchases XXXX 12345678",
        "Purchases XXXXXXXX 12345678",
        "Purchases (XXXX) 12345678",
    ] {
        assert!(
            entity(separated).identifiers().is_empty(),
            "{separated} exposed its suffix as an identifier"
        );
    }
    let alphabetic = ledgers(&["Sales XXXX 12345678", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&alphabetic, "Purchases XXXX 12345678").bound_name(),
        None,
        "a masked last-eight must not bind two unrelated ledgers"
    );
    // A suffix shaped as a code is no less hidden than one shaped as a number.
    assert!(entity("Purchases XXXX AB12345678").identifiers().is_empty());
    // A delimiter between the mask and its suffix does not unmask it. Reading
    // the state token by token, a `-` reset it and the suffix walked out.
    for punctuated in [
        "Purchases XXXX - 12345678",
        "Purchases **** / 12345678",
        "Purchases XXXX . 12345678",
        "Purchases XXXX - - 12345678",
    ] {
        assert!(
            entity(punctuated).identifiers().is_empty(),
            "{punctuated} exposed its suffix as an identifier"
        );
    }
    let separated = ledgers(&["Sales XXXX - 12345678", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&separated, "Purchases XXXX - 12345678").bound_name(),
        None
    );
    // An ordinary word after a mask does end it, or nothing downstream of one
    // could ever identify anything again.
    assert_eq!(
        entity("Purchases XXXX Invoice 5550001001")
            .identifiers()
            .len(),
        1
    );
    // Ordinary words are not masks, however repetitive: only a run of one
    // repeated letter is, and one letter alone is an ordinary word.
    assert_eq!(entity("Purchases Unit 5550001001").identifiers().len(), 1);
    assert_eq!(entity("Purchases A 5550001001").identifiers().len(), 1);
    // A mask is a shape, not a list of glyphs. Enumerating four of them lost
    // to a dotted and an underscored mask, and would have lost to the next.
    for shaped in [
        "Purchases ........12345678",
        "Purchases ____ 12345678",
        "Purchases ~~~ 12345678",
        "Purchases --- 12345678",
    ] {
        assert!(
            entity(shaped).identifiers().is_empty(),
            "{shaped} exposed its suffix as an identifier"
        );
    }
    let dotted = ledgers(&["Sales ........12345678", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&dotted, "Purchases ........12345678").bound_name(),
        None
    );
    // Ordinary punctuation around a whole number is not a mask.
    assert_eq!(entity("Party (5550001001)").identifiers().len(), 1);
    assert_eq!(entity("Party 5550001-002").identifiers().len(), 1);
    // Names punctuate; they do not repeat punctuation. Two of a character is
    // ordinary noise, so the run has to be longer than an operator's slip.
    assert_eq!(entity("S.K. Traders 5550001001").identifiers().len(), 1);
    assert_eq!(entity("Party -- 5550001001").identifiers().len(), 1);
    // And a number following an ordinary word is untouched.
    assert_eq!(entity("Invoice 5550001001").identifiers().len(), 1);
}

#[test]
fn a_fiscal_year_range_is_a_period_not_an_account_number() {
    // `2025-2026` strips to an eight-digit run that no calendar reading
    // rejects, and two unrelated ledgers share a fiscal year as routinely as
    // they share a month.
    for range in ["2025-2026", "2025/2026", "1999-2000"] {
        assert!(
            entity(&format!("Purchases {range}"))
                .identifiers()
                .is_empty(),
            "{range} was treated as an identifier"
        );
    }
    let catalog = ledgers(&["Sales 2025-2026", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&catalog, "Purchases 2025-2026").bound_name(),
        None
    );
    // A punctuated account number that is not a year range still binds.
    let accounts = ledgers(&["Party 5550001-002", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&accounts, "Other 5550001002").bound_name(),
        Some("Party 5550001-002")
    );
}

#[test]
fn an_eight_digit_date_in_any_admitted_order_is_not_an_identifier() {
    for date in ["20260910", "01012026", "31122026", "12312026"] {
        assert!(
            entity(&format!("Period {date}")).identifiers().is_empty(),
            "{date} was treated as an identifier"
        );
    }
    // A number that reads as no calendar date at all still is one.
    assert_eq!(entity("Party 55500001").identifiers().len(), 1);
}

#[test]
fn more_identifiers_than_the_bound_is_refused_not_truncated() {
    // Keeping the first few can discard the identifier that pointed at a
    // different master, turning a conflict into a bind.
    let many = (0..MAX_IDENTIFIERS_PER_NAME + 1)
        .map(|index| format!("5550{index:04}00"))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        SourceEntity::new(0, &many),
        Err(MasterBindingError::TooManyIdentifiers)
    );
    assert_eq!(
        MasterBindingError::TooManyIdentifiers.safe_reason_code(),
        "master_identifiers_too_many"
    );
}

#[test]
fn the_listing_variant_says_what_an_absent_candidate_means() {
    // The three facts that used to share one empty vector, now told apart by
    // the type. A consumer matching exhaustively is made to decide each.
    let catalog = ledgers(&["Alpha Traders", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&catalog, "Zeta Placeholder")
            .unresolved()
            .expect("unbound")
            .candidates,
        Candidates::None,
        "nothing resembles it"
    );

    let family = (0..MAX_PREFIX_FAMILY + 5)
        .map(|index| format!("ALPHAGROUP UNIT {index:02}"))
        .collect::<Vec<_>>();
    let family = MasterCatalog::new(MasterClass::Ledger, &family).expect("valid");
    assert_eq!(
        bind_one_name(&family, "ALPHAGROUP")
            .unresolved()
            .expect("unbound")
            .candidates,
        Candidates::Withheld {
            found: MAX_PREFIX_FAMILY + 5
        },
        "many exist and none separates them"
    );

    let listed = ledgers(&["ALPHA SALE", "ALPHA SALES", "SALES - ALPHA", "Beta Supply"]);
    let binding = bind_one_name(&listed, "ALPHA");
    let candidates = &binding.unresolved().expect("unbound").candidates;
    assert!(matches!(candidates, Candidates::Listed { .. }));
    assert_eq!(candidates.found(), 3);
}

#[test]
fn only_an_incomplete_listing_may_withhold_an_absence() {
    // The predicate a consumer needs before reporting "nothing like this is
    // present". `None` permits that conclusion; the other two forbid it.
    assert!(!Candidates::None.is_incomplete());
    assert!(!Candidates::Listed { listed: Vec::new() }.is_incomplete());
    assert!(Candidates::Withheld { found: 30 }.is_incomplete());
    assert!(Candidates::Truncated {
        listed: Vec::new(),
        found: 9
    }
    .is_incomplete());
    // `found` is the total, never the listed length, wherever it is known.
    assert_eq!(Candidates::Withheld { found: 30 }.found(), 30);
    assert!(Candidates::Withheld { found: 30 }.listed().is_empty());
}

// --- candidate discipline --------------------------------------------------

#[test]
fn a_catalog_wide_token_stops_discriminating() {
    let mut names = (0..40)
        .map(|index| format!("PLACEHOLDER UNIT {index:02}"))
        .collect::<Vec<_>>();
    names.push("ALPHA PLACEHOLDER TRADERS".to_string());
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    // "placeholder" is carried by every entry, so it may not pull all 41 in.
    let binding = bind_one_name(&catalog, "PLACEHOLDER ZETA");
    assert!(binding.unresolved().expect("unbound").candidates.found() <= 1);
}

#[test]
fn a_prefix_matching_a_whole_family_is_counted_and_deliberately_not_listed() {
    // Measured against live books: listing an arbitrary capped slice of a name
    // family put the right master out of view about a third of the time,
    // because the slice is ordered by name and the family is uniform. Counting
    // the family and listing none of it is the honest answer — the source name
    // genuinely does not distinguish one from another.
    let names = (0..MAX_PREFIX_FAMILY + 5)
        .map(|index| format!("ALPHAGROUP UNIT {index:02}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let binding = bind_one_name(&catalog, "ALPHAGROUP");
    let unresolved = binding.unresolved().expect("unbound");
    assert_eq!(reason(&binding), UnboundReason::NoDiscriminatingCandidate);
    assert!(unresolved.candidates.listed().is_empty());
    assert_eq!(unresolved.candidates.found(), MAX_PREFIX_FAMILY + 5);
    assert!(unresolved.candidates.is_incomplete());
}

#[test]
fn a_weaker_rule_cannot_reinstate_a_withheld_family() {
    // A token shared across a family *is* the family. Where the catalog is
    // large enough that the token stays under the common-token threshold — 30
    // rows among 330 is 9% — the shared-token pass was re-offering exactly the
    // rows the prefix pass had withheld, restoring the arbitrary capped slice
    // the withholding exists to prevent.
    let mut names = (0..30)
        .map(|index| format!("Acme Branch {index:03}"))
        .collect::<Vec<_>>();
    names.extend((0..300).map(|index| format!("Unrelated Ledger {index:03}")));
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    // The scenario only exercises the path while the token stays
    // discriminating: 10% of 330 is 33, and a 30-row family sits below it.
    assert!(
        30 <= names.len() * COMMON_TOKEN_PERCENT / 100,
        "the family would be suppressed as a common token, proving nothing"
    );

    let binding = bind_one_name(&catalog, "Acme Branch");
    let unresolved = binding.unresolved().expect("unbound");
    assert_eq!(reason(&binding), UnboundReason::NoDiscriminatingCandidate);
    assert!(unresolved.candidates.listed().is_empty());
    assert_eq!(unresolved.candidates.found(), 30);

    // A decisive rule still reaches a family member on its own evidence: the
    // whole key separates that one from its siblings, which is the difference
    // between withholding a family and hiding a match.
    let exact = bind_one_name(&catalog, "Acme Branch 017");
    assert_eq!(exact.bound_name(), Some("Acme Branch 017"));
}

#[test]
fn a_family_within_the_bound_is_still_listed_in_full() {
    let names = (0..MAX_PREFIX_FAMILY)
        .map(|index| format!("ALPHAGROUP UNIT {index:02}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let binding = bind_one_name(&catalog, "ALPHAGROUP");
    let unresolved = binding.unresolved().expect("unbound");
    assert_eq!(reason(&binding), UnboundReason::NearMiss);
    assert_eq!(unresolved.candidates.listed().len(), MAX_PREFIX_FAMILY);
    assert!(!unresolved.candidates.is_incomplete());
}

#[test]
fn the_reported_count_is_the_union_of_suppressed_and_listed_candidates() {
    // A suppressed family and the candidates still worth listing are not the
    // same masters. Reporting the larger of the two counts under-reports what
    // the name actually reaches, and candidate_count is promised as the total
    // found before truncation.
    let mut names = (0..MAX_PREFIX_FAMILY + 5)
        .map(|index| format!("Alpha Beta {index:02}"))
        .collect::<Vec<_>>();
    names.push("Alpha".to_string());
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");

    let binding = bind_one_name(&catalog, "Alpha Beta");
    let unresolved = binding.unresolved().expect("unbound");
    // The shorter master is still listed; the family behind it is not.
    assert_eq!(candidate_names(&binding), ["Alpha"]);
    assert_eq!(unresolved.candidates.found(), MAX_PREFIX_FAMILY + 6);
    assert!(unresolved.candidates.is_incomplete());
}

#[test]
fn an_identifier_hint_is_bounded_before_anything_scans_it() {
    let huge = "5".repeat(MAX_NAME_CHARS + 1);
    assert_eq!(
        SourceEntity::with_identifier_hints(0, "Alpha Traders", [huge.as_str()]),
        Err(MasterBindingError::NameTooLong)
    );
    // Bounding each hint does not bound the iterator. Repeated hints fold to
    // one identifier, so the deduplicated check never fired however many
    // arrived, while every one of them was scanned and copied first.
    let repeated = vec!["5550001001"; MAX_IDENTIFIERS_PER_NAME + 1];
    assert_eq!(
        SourceEntity::with_identifier_hints(0, "Alpha Traders", repeated),
        Err(MasterBindingError::TooManyIdentifiers)
    );
    // The bound admits everything a usable entity could carry.
    let distinct = (0..MAX_IDENTIFIERS_PER_NAME)
        .map(|index| format!("555000{index:04}"))
        .collect::<Vec<_>>();
    assert!(SourceEntity::with_identifier_hints(
        0,
        "Alpha Traders",
        distinct.iter().map(String::as_str)
    )
    .is_ok());
}

#[test]
fn candidate_order_is_rule_then_name_and_never_a_ranking() {
    let catalog = ledgers(&[
        "ALPHA (5550000002)",
        "ALPHA WHOLESALE",
        "ZETA ALPHA STORE",
        "ALPHA (5550000003)",
    ]);
    let source = SourceEntity::with_identifier_hints(0, "ALPHA", ["5550000002", "5550000003"])
        .expect("valid");
    let report = bound(&catalog, &[source]);
    let unresolved = report.entities()[0].unresolved().expect("unbound");
    assert_eq!(
        unresolved
            .candidates
            .listed()
            .iter()
            .map(|candidate| (candidate.catalog_name.as_str(), candidate.rule))
            .collect::<Vec<_>>(),
        [
            ("ALPHA (5550000002)", CandidateRule::SharedIdentifier),
            ("ALPHA (5550000003)", CandidateRule::SharedIdentifier),
            ("ALPHA WHOLESALE", CandidateRule::CatalogPrefix),
            ("ZETA ALPHA STORE", CandidateRule::SharedToken),
        ]
    );
}

#[test]
fn the_report_does_not_depend_on_the_order_the_book_returned() {
    let forward = ledgers(&["ALPHA SALE", "ALPHA SALES", "SALES - ALPHA", "Beta Supply"]);
    let reversed = ledgers(&["Beta Supply", "SALES - ALPHA", "ALPHA SALES", "ALPHA SALE"]);
    let entities = [entity("ALPHA"), entity("Beta Supply")];
    assert_eq!(
        bound(&forward, &entities).entities(),
        bound(&reversed, &entities).entities()
    );
}

// --- the unbound list is the product ---------------------------------------

#[test]
fn totals_reconcile_the_run() {
    let catalog = ledgers(&["Alpha Traders", "ALPHA SALE", "ALPHA SALES"]);
    let entities = [
        entity("Alpha Traders"),
        entity("ALPHA"),
        entity("Zeta Placeholder"),
    ];
    let report = bound(&catalog, &entities);
    let totals = report.totals();
    assert_eq!(totals.requested, 3);
    assert_eq!(totals.bound, 1);
    assert_eq!(totals.ambiguous, 1);
    assert_eq!(totals.unmatched, 1);
    assert_eq!(totals.requested, totals.bound + totals.unbound);
    assert_eq!(totals.unbound, totals.ambiguous + totals.unmatched);
    assert_eq!(report.bound().count(), totals.bound);
    assert_eq!(report.unbound().count(), totals.unbound);
    assert_eq!(report.class(), MasterClass::Ledger);
}

#[test]
fn an_unbound_entry_retains_the_identity_that_will_reallocate_it() {
    let catalog = ledgers(&["ALPHA (5550000001)", "BETA (5550000001)"]);
    let binding = bind_one_name(&catalog, "PARTY 5550000001");
    assert_eq!(
        binding.unresolved().expect("unbound").unresolved_identity,
        [Identifier {
            kind: IdentifierKind::Numeric,
            value: "5550000001".to_string(),
        }]
    );
}

// --- the fallback is constructed, never inferred ---------------------------

#[test]
fn an_ambiguous_entity_parks_against_a_verified_fallback() {
    let catalog = ledgers(&[
        "ALPHA (5550000001)",
        "BETA (5550000001)",
        "Suspense Placeholder",
    ]);
    let report = bound(&catalog, &[entity("PARTY 5550000001")]);
    let fallback = report
        .assign_fallback(0, &catalog, "Suspense Placeholder")
        .expect("an unbound entity may be parked");
    assert_eq!(fallback.fallback_name(), "Suspense Placeholder");
    assert_eq!(fallback.source_name(), "PARTY 5550000001");
    assert_eq!(fallback.reason(), UnboundReason::IdentifierConflict);
    assert_eq!(fallback.retained_tag(), "numeric:5550000001");
    assert_eq!(fallback.class(), MasterClass::Ledger);
}

#[test]
fn a_bound_entity_cannot_be_parked() {
    let catalog = ledgers(&["Alpha Traders", "Suspense Placeholder"]);
    let report = bound(&catalog, &[entity("Alpha Traders")]);
    assert_eq!(
        report.assign_fallback(0, &catalog, "Suspense Placeholder"),
        Err(MasterBindingError::FallbackNotInCatalog)
    );
}

#[test]
fn a_fallback_master_that_does_not_exist_is_refused() {
    // A suspense ledger that was never created is how one batch was lost.
    let catalog = ledgers(&["Alpha Traders", "Beta Supply"]);
    let report = bound(&catalog, &[entity("Zeta Placeholder")]);
    assert_eq!(
        report.assign_fallback(0, &catalog, "Suspense Placeholder"),
        Err(MasterBindingError::FallbackNotInCatalog)
    );
}

#[test]
fn a_fallback_cannot_be_drawn_from_another_catalog_class_or_another_report() {
    // A stock-item binding parked against a ledger catalog was a representable
    // state that nothing downstream could detect.
    let stock = MasterCatalog::new(MasterClass::StockItem, ["PH-01A-B00", "Scrap Placeholder"])
        .expect("valid");
    let ledger = ledgers(&["Alpha Traders", "Suspense Placeholder"]);
    let stock_report = bound(&stock, &[entity("Zeta Placeholder")]);
    assert_eq!(
        stock_report.assign_fallback(0, &ledger, "Suspense Placeholder"),
        Err(MasterBindingError::ClassMismatch)
    );
    // An index outside this report cannot name another report's entity.
    assert_eq!(
        stock_report.assign_fallback(7, &stock, "Scrap Placeholder"),
        Err(MasterBindingError::ClassMismatch)
    );
    // Nor may a *same-class* catalog the report was never produced from supply
    // the fallback: class is not provenance, and the master would never have
    // been a candidate for this entity.
    let other_ledgers = ledgers(&["Alpha Traders", "Different Suspense"]);
    let ledger_report = bound(&ledger, &[entity("Zeta Placeholder")]);
    assert_eq!(
        ledger_report.assign_fallback(0, &other_ledgers, "Different Suspense"),
        Err(MasterBindingError::ClassMismatch)
    );
    assert_ne!(ledger.fingerprint(), other_ledgers.fingerprint());
    // The same masters read twice fingerprint alike, whatever order they came
    // back in — a re-read must not invalidate a report.
    let reordered = ledgers(&["Suspense Placeholder", "Alpha Traders", "Beta Supply"]);
    let forward = ledgers(&["Alpha Traders", "Beta Supply", "Suspense Placeholder"]);
    assert_eq!(reordered.fingerprint(), forward.fingerprint());
    assert_eq!(
        MasterBindingError::ClassMismatch.safe_reason_code(),
        "master_class_mismatch"
    );
}

#[test]
fn the_adr_quotes_the_thresholds_this_module_actually_uses() {
    // ADR 0016 is the contract two surfaces integrate against, so a threshold
    // that moves in code and not in the document sends a future integration
    // the wrong rule. "Remember to update the record" is the kind of rule this
    // project prefers to replace with something that fails.
    const ADR: &str = include_str!("../../../../docs/adr/0016-master-binding-authority.md");
    for (constant, value) in [
        (
            "MIN_NUMERIC_IDENTIFIER_DIGITS",
            MIN_NUMERIC_IDENTIFIER_DIGITS,
        ),
        ("MIN_CODE_IDENTIFIER_DIGITS", MIN_CODE_IDENTIFIER_DIGITS),
        ("MIN_CODE_IDENTIFIER_CHARS", MIN_CODE_IDENTIFIER_CHARS),
        ("MAX_CANDIDATES_PER_ENTITY", MAX_CANDIDATES_PER_ENTITY),
        ("COMMON_TOKEN_PERCENT", COMMON_TOKEN_PERCENT),
    ] {
        // A percentage reads naturally as `(10%)`; both spellings count, and
        // neither lets a changed number pass.
        let plain = format!("`{constant}` ({value})");
        let percent = format!("`{constant}` ({value}%)");
        assert!(
            ADR.contains(&plain) || ADR.contains(&percent),
            "ADR 0016 does not quote {constant} as {value}; it must read {plain:?}"
        );
    }
}

// --- the vocabulary is stable ----------------------------------------------

#[test]
fn reason_and_error_codes_are_stable_and_safe() {
    assert_eq!(
        UnboundReason::IdentifierConflict.safe_reason_code(),
        "master_binding_identifier_conflict"
    );
    assert_eq!(
        UnboundReason::IdentifierNameConflict.safe_reason_code(),
        "master_binding_identifier_name_conflict"
    );
    assert_eq!(
        UnboundReason::NameAmbiguous.safe_reason_code(),
        "master_binding_name_ambiguous"
    );
    assert_eq!(
        UnboundReason::NearMiss.safe_reason_code(),
        "master_binding_near_miss"
    );
    assert_eq!(
        MasterBindingError::CatalogEmpty.safe_reason_code(),
        "master_catalog_empty"
    );
}

#[test]
fn every_unresolved_shape_survives_serialization() {
    // A newtype variant under internal tagging cannot carry a sequence, and it
    // failed at runtime on the *most common* unresolved result while the other
    // three variants serialized fine. No test caught it because none had ever
    // serialized an `Unresolved` — only a `Bound`.
    let listed = ledgers(&["ALPHA SALE", "ALPHA SALES", "SALES - ALPHA", "Beta Supply"]);
    let family = (0..MAX_PREFIX_FAMILY + 5)
        .map(|index| format!("ALPHAGROUP UNIT {index:02}"))
        .collect::<Vec<_>>();
    let family = MasterCatalog::new(MasterClass::Ledger, &family).expect("valid");
    let missing = ledgers(&["Alpha Traders", "Beta Supply"]);

    for (label, binding) in [
        ("listed", bind_one_name(&listed, "ALPHA")),
        ("withheld", bind_one_name(&family, "ALPHAGROUP")),
        ("none", bind_one_name(&missing, "Zeta Placeholder")),
    ] {
        let json = serde_json::to_string(&binding)
            .unwrap_or_else(|error| panic!("{label} failed to serialize: {error}"));
        let back: EntityBinding = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("{label} failed to deserialize: {error}"));
        assert_eq!(back, binding, "{label} did not round-trip");
    }
}

#[test]
fn a_bound_status_serializes_without_a_score_field() {
    let catalog = ledgers(&["Alpha Traders"]);
    let binding = bind_one_name(&catalog, "Alpha Traders");
    let json = serde_json::to_value(&binding).expect("serializable");
    assert_eq!(json["status"], "bound");
    assert_eq!(json["catalog_name"], "Alpha Traders");
    assert_eq!(json["basis"], "exact_name");
    assert!(json.get("score").is_none());
    assert!(json.get("confidence").is_none());
}

// ---------------------------------------------------------------------------
// Characterization against a realistically shaped book
//
// Every rule above is tested in isolation on a handful of names. Three of the
// rules only engage at scale — common-token suppression needs 20+ entries,
// candidate capping needs 25+, and the prefix ranges only matter when many
// keys share a head — so their interaction is untested by any of it.
//
// This section fabricates one 200-master catalog carrying the naming
// pathologies actually recorded (a firm word on most ledgers, numbers typed
// into party names, a near-duplicate sales trio, a masked bank last-four) and
// pins the *outcome* for a document-sized set of source names.
//
// The assertion that matters is not the count. It is that **no entity binds to
// a master a human would not have chosen**: a wrong bind puts money against the
// wrong party, and is strictly worse than an unbound row. The counts are pinned
// underneath it so that loosening a threshold has to move a number in a diff.
//
// This is fabricated input. It characterizes the rules and is not evidence
// about any Tally instance or any real book's bindability.
// ---------------------------------------------------------------------------

const GREEK: [&str; 20] = [
    "ALPHA", "BETA", "GAMMA", "DELTA", "EPSILON", "ZETA", "ETA", "THETA", "IOTA", "KAPPA",
    "LAMBDA", "MU", "NU", "XI", "OMICRON", "PI", "RHO", "SIGMA", "TAU", "UPSILON",
];

/// One fabricated book: 200 ledgers, shaped like a small trading firm's.
fn fabricated_book() -> Vec<String> {
    let mut names = Vec::new();
    // 20 party ledgers with a number typed into the name, as operators do.
    for (index, greek) in GREEK.iter().enumerate() {
        names.push(format!(
            "{greek} PLACEHOLDER ({})",
            5_550_001_001_u64 + index as u64
        ));
    }
    // 20 party ledgers without one.
    for greek in GREEK {
        names.push(format!("{greek} PLACEHOLDER TRADING CO"));
    }
    // The near-duplicate trio that one engagement actually met.
    names.push("ALPHA SALE".to_string());
    names.push("ALPHA SALES".to_string());
    names.push("SALES - ALPHA".to_string());
    // Tax heads, which share heavy word overlap with each other.
    for head in ["CGST", "SGST", "IGST"] {
        for side in ["OUTPUT", "INPUT"] {
            for rate in ["9%", "18%"] {
                names.push(format!("{head} {side} {rate}"));
            }
        }
    }
    // Banks, carrying a masked last-four rather than a full account number.
    names.push("PLACEHOLDER BANK CA 2129".to_string());
    names.push("PLACEHOLDER BANK OD 7745".to_string());
    // The accounts every book has.
    for name in [
        "Cash",
        "Suspense Placeholder",
        "Round Off",
        "Profit & Loss A/c",
    ] {
        names.push(name.to_string());
    }
    // Filler carrying one firm-wide word, to the size of a real small book.
    let mut index = 0;
    while names.len() < 200 {
        names.push(format!("PLACEHOLDER UNIT {index:03}"));
        index += 1;
    }
    names
}

#[derive(Debug, PartialEq, Eq)]
enum Expected {
    /// The master a human reading the source would have chosen.
    Bound(&'static str),
    Unbound(UnboundReason),
}

/// What one document names, and what a human would do with each.
fn fabricated_document() -> Vec<(&'static str, Option<&'static str>, Expected)> {
    vec![
        // Named exactly as the book spells it.
        ("Cash", None, Expected::Bound("Cash")),
        ("CGST OUTPUT 9%", None, Expected::Bound("CGST OUTPUT 9%")),
        // Case noise alone resolves, and so does spacing noise: §9.4d measured
        // leading whitespace and a collapsed run matching on licensed 7.1.
        ("cgst output 9%", None, Expected::Bound("CGST OUTPUT 9%")),
        (
            "  cgst   output 9%  ",
            None,
            Expected::Bound("CGST OUTPUT 9%"),
        ),
        (
            "beta placeholder trading co",
            None,
            Expected::Bound("BETA PLACEHOLDER TRADING CO"),
        ),
        // The engagement case: the source names the party its own way and
        // carries the number in a separate column. Name matching would offer
        // twenty wrong parties; the number decides.
        (
            "GAMMA. K.",
            Some("5550001003"),
            Expected::Bound("GAMMA PLACEHOLDER (5550001003)"),
        ),
        // Same, with the number inside the name rather than a hint.
        (
            "DELTA K 5550001004",
            None,
            Expected::Bound("DELTA PLACEHOLDER (5550001004)"),
        ),
        // A truncated party name: one candidate, and one candidate is still
        // not a decision.
        (
            "EPSILON PLACEHOLDER TRADING",
            None,
            Expected::Unbound(UnboundReason::NearMiss),
        ),
        // The near-duplicate trio. Nothing here may resolve.
        ("ALPHA SALE", None, Expected::Bound("ALPHA SALE")),
        ("ALPHA", None, Expected::Unbound(UnboundReason::NearMiss)),
        // A masked bank last-four must not bind on four digits.
        (
            "PLACEHOLDER BANK 2129",
            None,
            Expected::Unbound(UnboundReason::NearMiss),
        ),
        // A party the book simply does not have.
        (
            "OMEGA WHOLESALE",
            None,
            Expected::Unbound(UnboundReason::NoCandidate),
        ),
        // A number the book does not carry: the hint finds nothing, and the
        // name is left to answer on its own.
        (
            "PSI SUPPLY",
            Some("5559999999"),
            Expected::Unbound(UnboundReason::NoCandidate),
        ),
    ]
}

#[test]
fn a_document_against_a_realistic_book_binds_only_where_a_human_would() {
    let names = fabricated_book();
    assert_eq!(names.len(), 200);
    let catalog =
        MasterCatalog::new(MasterClass::Ledger, &names).expect("the fabricated book is valid");
    assert_eq!(catalog.master_count(), 200);

    let document = fabricated_document();
    let entities = document
        .iter()
        .enumerate()
        .map(|(position, (name, hint, _))| match hint {
            Some(hint) => SourceEntity::with_identifier_hints(position, name, [*hint]),
            None => SourceEntity::new(position, name),
        })
        .map(|entity| entity.expect("fabricated source names are valid"))
        .collect::<Vec<_>>();
    let report = bound(&catalog, &entities);

    for ((source, _, expected), binding) in document.iter().zip(report.entities()) {
        match (&binding.status, expected) {
            (BindingStatus::Bound { catalog_name, .. }, Expected::Bound(intended)) => assert_eq!(
                catalog_name, intended,
                "{source:?} bound to a master a human would not have chosen"
            ),
            (
                BindingStatus::Ambiguous(unresolved) | BindingStatus::Unmatched(unresolved),
                Expected::Unbound(intended),
            ) => assert_eq!(
                unresolved.reason, *intended,
                "{source:?} was unbound for an unintended reason"
            ),
            (status, expected) => {
                panic!("{source:?}: expected {expected:?}, got {status:?}")
            }
        }
    }

    // The shape of the answer, pinned so a loosened threshold moves a number.
    let totals = report.totals();
    assert_eq!(totals.requested, 13);
    assert_eq!(totals.bound, 8);
    assert_eq!(totals.ambiguous, 3);
    assert_eq!(totals.unmatched, 2);
    assert_eq!(totals.requested, totals.bound + totals.unbound);
}

#[test]
fn a_firm_wide_word_does_not_drag_the_whole_book_into_every_candidate_list() {
    // "placeholder" is carried by most of this book. Without suppression the
    // unbound list stops being a work item and becomes a second data-entry job.
    let names = fabricated_book();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let carriers = names
        .iter()
        .filter(|name| name.to_lowercase().contains("placeholder"))
        .count();
    assert!(carriers > 100, "the fabricated book must exercise this");

    let binding = bind_one_name(&catalog, "OMEGA PLACEHOLDER");
    let unresolved = binding.unresolved().expect("unbound");
    assert!(
        unresolved.candidates.found() <= MAX_CANDIDATES_PER_ENTITY,
        "a firm-wide word pulled in {} candidates",
        unresolved.candidates.found()
    );
}

/// The mutations a source document actually applies to a name it copied from
/// somewhere else: case, spacing, a dropped tail, a dropped last word.
fn source_mutations(name: &str) -> Vec<String> {
    let mut mutations = vec![name.to_uppercase(), name.to_lowercase()];
    mutations.push(format!(
        "  {}  ",
        name.split_whitespace().collect::<Vec<_>>().join("  ")
    ));
    let characters = name.chars().collect::<Vec<_>>();
    let kept = characters.len() * 4 / 5;
    if kept >= MIN_PREFIX_KEY_CHARS {
        mutations.push(characters[..kept].iter().collect());
    }
    let words = name.split_whitespace().collect::<Vec<_>>();
    if words.len() > 2 {
        mutations.push(words[..words.len() - 1].join(" "));
    }
    mutations
}

#[test]
fn no_mutation_of_a_master_name_ever_binds_to_a_different_master() {
    // The safety property, stated over a whole book rather than three chosen
    // names: a wrong bind puts money against the wrong party, and is strictly
    // worse than an unbound row. Roughly a thousand cases.
    //
    // Mutations that collide with *another* master under the comparison key
    // are excluded, and deliberately so: truncating `ALPHA SALES` by one
    // character yields `ALPHA SALE`, which is a real and different ledger. No
    // rule can distinguish a truncation of one name from an exact spelling of
    // another, and binding it to the name it actually spells is correct.
    //
    // **This sweep was checked against two positive controls**, because an
    // assertion that has never failed is not yet known to be an instrument:
    //
    // - Resolving a near-miss to its first-ordered candidate — precisely what
    //   the deleted MCP helper did through `exact_live_spelling` — trips it on
    //   a truncated party name. So it does report presence.
    // - Binding a *lone* candidate does **not** trip it, because in this book a
    //   lone candidate is nearly always the master the mutation came from. That
    //   regression is caught by `a_single_candidate_still_does_not_bind` and by
    //   the two prefix tests instead.
    //
    // Read this test as "no mutation reaches the wrong master", never as "no
    // rule change can loosen binding".
    let names = fabricated_book();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let keys = names
        .iter()
        .map(|name| (comparison_key(name), name.as_str()))
        .collect::<BTreeMap<_, _>>();

    // What the *wide* fold would resolve. Narrowing the resolving fold to the
    // three transformations §9.4b verified is only defensible if it withdraws
    // answers, never masters, so the cases it used to settle are tracked by
    // name rather than by a percentage that drifts with the fixture.
    let wide = names
        .iter()
        .map(|name| (master_identity_key(name), name.as_str()))
        .collect::<BTreeMap<_, _>>();

    let mut checked = 0_usize;
    let mut self_bound = 0_usize;
    let mut self_offered = 0_usize;
    for name in &names {
        for mutation in source_mutations(name) {
            let key = comparison_key(&mutation);
            if keys.get(&key).is_some_and(|owner| owner != name) {
                continue; // the mutation spells a different real ledger
            }
            checked += 1;
            let binding = bind_one_name(&catalog, &mutation);
            let wide_would_bind = wide.get(&master_identity_key(&mutation)) == Some(&name.as_str());
            match binding.bound_name() {
                None => {
                    let offered = candidate_names(&binding).iter().any(|shown| shown == name);
                    if offered {
                        self_offered += 1;
                    }
                    // The invariant that makes the narrowing a trade rather than
                    // a loss: anything the wide fold settled is still shown.
                    assert!(
                        !wide_would_bind || offered,
                        "narrowing the fold hid {name:?} from its own mutation {mutation:?}"
                    );
                }
                Some(bound_to) => {
                    assert_eq!(
                        bound_to, name,
                        "mutation {mutation:?} of {name:?} bound to a different master"
                    );
                    self_bound += 1;
                }
            }
        }
    }
    assert!(
        checked > 900,
        "the sweep must actually cover the book: {checked}"
    );
    // Most of this book's mutations are spacing and separator noise, and §9.4d
    // measured Tally folding all of it on the SKU this writes to — so most of
    // them resolve again. The per-case invariant above still holds and is the
    // point: anything the wide fold reaches is bound or shown, never hidden.
    //
    // On this book the two folds now agree on every mutation, so the sweep does
    // **not** exercise the gap between them. The cases that still separate them
    // — an en dash, an underscore, an NFD spelling — are covered by
    // `space_hyphen_and_slash_are_one_separator_in_both_directions` and
    // `two_encodings_of_one_name_are_two_masters_to_tally_and_so_to_this`
    // instead, and this comment exists so nobody reads a green sweep as
    // evidence about them.
    assert!(
        self_bound * 2 > checked,
        "most mutations should resolve once the fold matches the gateway: \
         {self_bound} bound of {checked}"
    );
    assert!(
        self_bound + self_offered > checked * 2 / 3,
        "{self_bound} bound and {self_offered} offered of {checked}"
    );
}

#[test]
fn a_number_typed_into_a_master_name_finds_it_from_any_source_name() {
    // The rule that decided the case fuzzy matching got wrong, exercised
    // against every party ledger in the book rather than one.
    let names = fabricated_book();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let numbered = names
        .iter()
        .filter(|name| name.contains("PLACEHOLDER ("))
        .collect::<Vec<_>>();
    assert_eq!(numbered.len(), 20);

    for name in numbered {
        let number = name
            .rsplit_once('(')
            .and_then(|(_, tail)| tail.strip_suffix(')'))
            .expect("fabricated party names carry a number");
        // A source name sharing nothing with the ledger name at all.
        let entity = SourceEntity::with_identifier_hints(0, "UNRELATED SOURCE PARTY", [number])
            .expect("valid");
        let report = bound(&catalog, &[entity]);
        assert_eq!(
            report.entities()[0].bound_name(),
            Some(name.as_str()),
            "the number typed into {name:?} did not find it"
        );
    }
}

#[test]
fn a_dash_variant_does_not_spell_a_short_code_into_a_decisive_one() {
    // The code threshold was `canonical.len()`, which is UTF-8 bytes. An en
    // dash is three of them, so `AB–123` measured eight and cleared a bound
    // meant for eight *characters* while carrying five alphanumerics. Its
    // ASCII twin reduces to `AB123` and is refused, so the same code decided
    // or did not on the strength of which dash the document happened to use —
    // and the wrong master was reached only in the spelling nobody checks.
    let catalog = ledgers(&["Sales AB\u{2013}123", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Purchases AB\u{2013}123");
    assert_eq!(
        binding.bound_name(),
        None,
        "five alphanumerics identifier-bound because one of them was three bytes"
    );

    // Padding is the same defect without the twin to compare against: bytes
    // counted the dashes, characters would have counted them too.
    let padded = ledgers(&["Sales AB\u{2013}\u{2013}\u{2013}123", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&padded, "Purchases AB\u{2013}\u{2013}\u{2013}123").bound_name(),
        None,
        "a code padded with dashes cleared the threshold on its punctuation"
    );

    // The measured shape still binds: `PH-01-AB-00` is eight alphanumerics
    // once the separators §9.4d measured are discarded, and it identifies.
    let stock = ledgers(&["Sales PH-01-AB-00", "Beta Supply"]);
    assert_eq!(
        bind_one_name(&stock, "Purchases PH01AB00").bound_name(),
        Some("Sales PH-01-AB-00"),
        "the fold §9.4d measured stopped working"
    );
}

#[test]
fn a_withheld_family_counts_the_masters_the_other_identifier_listed_too() {
    // The count was the largest skipped family's size, which ignores every
    // master the entity's *other* identifiers reached. Thirty masters share
    // one number and a thirty-first carries the other: the operator was told
    // thirty, and the master that made the two disagree was not in the number.
    let mut names = (0..MAX_CANDIDATES_PER_ENTITY + 5)
        .map(|index| format!("Shared Party {index:03} (5550007777)"))
        .collect::<Vec<_>>();
    names.push("Lone Party (5550008888)".to_string());
    let family = MAX_CANDIDATES_PER_ENTITY + 5;
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");

    let entity =
        SourceEntity::with_identifier_hints(0, "Zeta Holdings", ["5550007777", "5550008888"])
            .expect("valid");
    let binding = bound(&catalog, &[entity])
        .entities()
        .first()
        .cloned()
        .expect("one entity in, one binding out");

    assert_eq!(reason(&binding), UnboundReason::IdentifierConflict);
    let unresolved = binding.unresolved().expect("unbound");
    assert!(
        unresolved.candidates.is_incomplete(),
        "the family was listed"
    );
    assert_eq!(
        unresolved.candidates.found(),
        family + 1,
        "the master the other identifier listed was not counted"
    );
    // Still without building the union: the large family is a membership test,
    // never an expansion.
    assert!(
        family + 1 > MAX_CANDIDATES_PER_ENTITY,
        "this fixture no longer exercises the skip"
    );
}

#[test]
fn hint_variants_of_one_name_do_not_crowd_out_a_key_that_repeats() {
    // The memo is keyed by the source key *and* the masters the identifiers
    // reached, but repetition was counted on the key alone. Every hint variant
    // of one name therefore counted as repeated, filled the memo with entries
    // nothing asks for twice, and the pair that genuinely repeated behind them
    // could no longer be inserted — the same stall as the source-order defect,
    // through a different door.
    let names = (0..60)
        .map(|index| format!("Acme Branch {index:05}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");

    // Distinct hints, one name: same key, different memo key, each asked once.
    let mut entities = (0..MAX_CANDIDATE_MEMO_ENTRIES)
        .map(|index| {
            SourceEntity::with_identifier_hints(
                index,
                "Acme Branch",
                [format!("5550{index:06}").as_str()],
            )
            .expect("valid")
        })
        .collect::<Vec<_>>();
    // And then a pair that does repeat, four times over.
    entities.extend((0..4).map(|offset| {
        SourceEntity::with_identifier_hints(
            MAX_CANDIDATE_MEMO_ENTRIES + offset,
            "Acme Branch",
            ["5559999999"],
        )
        .expect("valid")
    }));

    super::CANDIDATE_SEARCHES.with(|count| count.set(0));
    let report = bound(&catalog, &entities);
    let searches = super::CANDIDATE_SEARCHES.with(std::cell::Cell::get);
    assert_eq!(report.totals().requested, MAX_CANDIDATE_MEMO_ENTRIES + 4);
    assert!(
        searches <= MAX_CANDIDATE_MEMO_ENTRIES + 1,
        "the repeated pair was searched more than once behind {MAX_CANDIDATE_MEMO_ENTRIES} variants: {searches} searches"
    );
}

#[test]
fn a_request_within_every_other_bound_is_still_refused_on_its_total_size() {
    // The count bound and the per-name bound do not bound their product. Each
    // of these names is valid, and the list is well inside the entity count;
    // together they are more than the aggregate the catalog side has always
    // had, and this module would have cloned them into a report first.
    // One long token rather than many short ones: the aggregate is what this
    // test is about, and a name of 1,800 words would spend the whole test in
    // identifier extraction proving nothing extra.
    let name = format!("Zeta{}", "z".repeat(MAX_NAME_CHARS - 5));
    assert!(
        name.chars().count() < MAX_NAME_CHARS,
        "each name must stay individually valid"
    );
    let catalog = ledgers(&["Alpha Supply", "Beta Supply"]);
    let entities = (0..1_200)
        .map(|index| SourceEntity::new(index, &name).expect("each name is valid alone"))
        .collect::<Vec<_>>();
    assert!(
        entities.len() < MAX_SOURCE_ENTITIES,
        "the count bound must not be what refuses this"
    );
    assert_eq!(
        bind(&catalog, &entities),
        Err(MasterBindingError::SourceNamesTooLarge)
    );

    // And the bound does not refuse an ordinary request: the same names, well
    // under the aggregate, bind as before.
    let ordinary = (0..40)
        .map(|index| SourceEntity::new(index, &name).expect("valid"))
        .collect::<Vec<_>>();
    assert!(bind(&catalog, &ordinary).is_ok());
}

#[test]
fn the_listing_word_is_the_one_the_wire_carries() {
    // `listing()` exists so a projection need not reconstruct the state from
    // an empty list and a count. If it drifted from the serde tag, a consumer
    // reading the DTO and a consumer reading the JSON would disagree about the
    // same binding — so they are asserted against each other, not assumed.
    let candidate = Candidate {
        catalog_name: "Alpha Supply".to_string(),
        rule: CandidateRule::ExactName,
    };
    for candidates in [
        Candidates::None,
        Candidates::Listed {
            listed: vec![candidate.clone()],
        },
        Candidates::Truncated {
            listed: vec![candidate],
            found: 9,
        },
        Candidates::Withheld { found: 9 },
    ] {
        let json = serde_json::to_value(&candidates).expect("candidates serialize");
        assert_eq!(
            json.get("listing").and_then(serde_json::Value::as_str),
            Some(candidates.listing()),
            "the accessor and the wire tag disagree about {candidates:?}"
        );
    }
}
