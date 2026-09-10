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
fn an_observed_master_name_is_retained_verbatim_while_a_source_name_is_trimmed() {
    // A caller writes the bound name back to Tally byte for byte. Trimming an
    // observed name here would report a spelling that does not exist and
    // refuse at the write gate with no explanation.
    let catalog = ledgers(&["  Alpha Traders  ", "Beta Supply"]);
    assert_eq!(catalog.names().next(), Some("  Alpha Traders  "));
    let binding = bind_one_name(&catalog, "Alpha Traders");
    assert_eq!(binding.bound_name(), Some("  Alpha Traders  "));
    assert_eq!(binding.source_name, "Alpha Traders");
}

#[test]
fn names_differing_only_in_surrounding_whitespace_are_an_ambiguity_not_a_refused_catalog() {
    let catalog = ledgers(&["Alpha Traders", "Alpha Traders "]);
    let binding = bind_one_name(&catalog, "Alpha Traders");
    // Byte equality still picks the exact one; the near-identical sibling is
    // not a reason to fail the whole read.
    assert_eq!(binding.bound_name(), Some("Alpha Traders"));
    let other = bind_one_name(&catalog, "alpha traders");
    assert_eq!(reason(&other), UnboundReason::NameAmbiguous);
    assert_eq!(candidate_names(&other), ["Alpha Traders", "Alpha Traders "]);
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
fn case_whitespace_and_dash_style_do_not_defeat_a_bind() {
    let catalog = ledgers(&["Alpha \u{2013} Traders", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "  alpha - TRADERS  ");
    assert_eq!(
        binding.status,
        BindingStatus::Bound {
            catalog_name: "Alpha \u{2013} Traders".to_string(),
            basis: BindingBasis::NormalizedName,
        }
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
            .iter()
            .map(|candidate| candidate.rule)
            .collect::<Vec<_>>(),
        [
            CandidateRule::CatalogPrefix,
            CandidateRule::CatalogPrefix,
            CandidateRule::SharedToken
        ]
    );
    assert_eq!(unresolved.candidate_count, 3);
    assert!(!unresolved.candidates_truncated);
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
    for label in ["FY25", "FY2025", "AY2026", "Q3", "H2", "PER2026"] {
        assert!(
            entity(&format!("Purchases {label}"))
                .identifiers()
                .is_empty(),
            "{label} was treated as a code identifier"
        );
    }
    let catalog = ledgers(&["Sales FY2025", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Purchases FY2025");
    assert_eq!(
        binding.bound_name(),
        None,
        "a shared period label must not bind two unrelated ledgers"
    );
    // A genuine identity-bearing code still is one.
    assert_eq!(entity("Item PH01AB00").identifiers().len(), 1);
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
    assert!(binding.unresolved().expect("unbound").candidate_count <= 1);
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
    assert!(unresolved.candidates.is_empty());
    assert_eq!(unresolved.candidate_count, MAX_PREFIX_FAMILY + 5);
    assert!(unresolved.candidates_truncated);
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
    assert_eq!(unresolved.candidates.len(), MAX_PREFIX_FAMILY);
    assert!(!unresolved.candidates_truncated);
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
    assert_eq!(unresolved.candidate_count, MAX_PREFIX_FAMILY + 6);
    assert!(unresolved.candidates_truncated);
}

#[test]
fn an_identifier_hint_is_bounded_before_anything_scans_it() {
    let huge = "5".repeat(MAX_NAME_CHARS + 1);
    assert_eq!(
        SourceEntity::with_identifier_hints(0, "Alpha Traders", [huge.as_str()]),
        Err(MasterBindingError::NameTooLong)
    );
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
    assert_eq!(
        MasterBindingError::ClassMismatch.safe_reason_code(),
        "master_class_mismatch"
    );
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
        // Case and spacing noise from the source system.
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
    assert_eq!(totals.requested, 12);
    assert_eq!(totals.bound, 7);
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
        unresolved.candidate_count <= MAX_CANDIDATES_PER_ENTITY,
        "a firm-wide word pulled in {} candidates",
        unresolved.candidate_count
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

    let mut checked = 0_usize;
    let mut self_bound = 0_usize;
    for name in &names {
        for mutation in source_mutations(name) {
            let key = comparison_key(&mutation);
            if keys.get(&key).is_some_and(|owner| owner != name) {
                continue; // the mutation spells a different real ledger
            }
            checked += 1;
            let binding = bind_one_name(&catalog, &mutation);
            match binding.bound_name() {
                None => {}
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
    // Most mutations are case and spacing noise, which must still bind.
    assert!(
        self_bound * 2 > checked,
        "only {self_bound} of {checked} mutations bound at all"
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
