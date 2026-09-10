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
fn candidates_are_capped_with_the_true_count_retained() {
    let names = (0..MAX_CANDIDATES_PER_ENTITY + 5)
        .map(|index| format!("ALPHAGROUP UNIT {index:02}"))
        .collect::<Vec<_>>();
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("valid");
    let binding = bind_one_name(&catalog, "ALPHAGROUP");
    let unresolved = binding.unresolved().expect("unbound");
    assert_eq!(unresolved.candidates.len(), MAX_CANDIDATES_PER_ENTITY);
    assert_eq!(unresolved.candidate_count, MAX_CANDIDATES_PER_ENTITY + 5);
    assert!(unresolved.candidates_truncated);
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
    let binding = bind_one_name(&catalog, "PARTY 5550000001");
    let fallback = FallbackBinding::assign(&binding, &catalog, "Suspense Placeholder")
        .expect("an unbound entity may be parked");
    assert_eq!(fallback.fallback_name(), "Suspense Placeholder");
    assert_eq!(fallback.source_name(), "PARTY 5550000001");
    assert_eq!(fallback.reason(), UnboundReason::IdentifierConflict);
    assert_eq!(fallback.retained_tag(), "numeric:5550000001");
}

#[test]
fn a_bound_entity_cannot_be_parked() {
    let catalog = ledgers(&["Alpha Traders", "Suspense Placeholder"]);
    let binding = bind_one_name(&catalog, "Alpha Traders");
    assert_eq!(
        FallbackBinding::assign(&binding, &catalog, "Suspense Placeholder"),
        Err(MasterBindingError::FallbackNotInCatalog)
    );
}

#[test]
fn a_fallback_master_that_does_not_exist_is_refused() {
    // A suspense ledger that was never created is how one batch was lost.
    let catalog = ledgers(&["Alpha Traders", "Beta Supply"]);
    let binding = bind_one_name(&catalog, "Zeta Placeholder");
    assert_eq!(
        FallbackBinding::assign(&binding, &catalog, "Suspense Placeholder"),
        Err(MasterBindingError::FallbackNotInCatalog)
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
