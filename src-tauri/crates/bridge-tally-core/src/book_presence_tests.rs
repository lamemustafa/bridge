//! Every name, number and amount here is fabricated from a placeholder
//! alphabet. These tests establish the behaviour of the rules; they are not,
//! and may not be presented as, evidence about any Tally instance.

use super::*;

const LEDGERS: [&str; 6] = [
    "Alpha Traders",
    "Bravo Industries",
    "Charlie Minerals",
    "Sales Account",
    "Output CGST 9%",
    "Output SGST 9%",
];

fn catalog() -> MasterCatalog {
    MasterCatalog::new(MasterClass::Ledger, LEDGERS).expect("catalog")
}

fn catalog_of(names: &[&str]) -> MasterCatalog {
    MasterCatalog::new(MasterClass::Ledger, names).expect("catalog")
}

fn entries<'a>(rows: &'a [[&'a str; 2]]) -> Vec<ObservedEntry<'a>> {
    rows.iter()
        .map(|row| ObservedEntry {
            ledger: row[0],
            amount: row[1],
        })
        .collect()
}

struct BookRow {
    key: &'static str,
    date: &'static str,
    voucher_type: &'static str,
    number: Option<&'static str>,
    remote_id: Option<&'static str>,
    party: Option<&'static str>,
    rows: Vec<[&'static str; 2]>,
    cancelled: bool,
    optional: bool,
}

impl BookRow {
    fn new(key: &'static str, date: &'static str, number: &'static str) -> Self {
        Self {
            key,
            date,
            voucher_type: "Sales",
            number: Some(number),
            remote_id: None,
            party: Some("Alpha Traders"),
            rows: vec![
                ["Alpha Traders", "-11800.00"],
                ["Sales Account", "10000.00"],
                ["Output CGST 9%", "900.00"],
                ["Output SGST 9%", "900.00"],
            ],
            cancelled: false,
            optional: false,
        }
    }

    fn party(mut self, party: &'static str) -> Self {
        self.rows[0][0] = party;
        self.party = Some(party);
        self
    }

    fn voucher_type(mut self, voucher_type: &'static str) -> Self {
        self.voucher_type = voucher_type;
        self
    }

    fn remote_id(mut self, remote_id: &'static str) -> Self {
        self.remote_id = Some(remote_id);
        self
    }

    fn rows(mut self, rows: Vec<[&'static str; 2]>) -> Self {
        self.rows = rows;
        self
    }

    /// Sets PARTYLEDGERNAME alone, leaving the entry ledgers untouched, so a
    /// voucher whose party field and entries name different ledgers can exist.
    fn party_field(mut self, party: &'static str) -> Self {
        self.party = Some(party);
        self
    }

    fn cancelled(mut self) -> Self {
        self.cancelled = true;
        self
    }

    fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    fn build(&self) -> BookVoucher {
        let entries = entries(&self.rows);
        BookVoucher::observed(ObservedVoucher {
            key: self.key,
            date: self.date,
            voucher_type: self.voucher_type,
            voucher_number: self.number,
            remote_id: self.remote_id,
            party: self.party,
            entries: &entries,
            cancelled: self.cancelled,
            optional: self.optional,
        })
        .expect("observed voucher")
    }
}

struct ProposalRow {
    position: usize,
    date: &'static str,
    voucher_type: &'static str,
    number: Option<&'static str>,
    remote_id: Option<&'static str>,
    party: Option<&'static str>,
    rows: Vec<[&'static str; 2]>,
}

impl ProposalRow {
    fn new(position: usize, date: &'static str, number: &'static str) -> Self {
        Self {
            position,
            date,
            voucher_type: "Sales",
            number: Some(number),
            remote_id: None,
            party: Some("Alpha Traders"),
            rows: vec![
                ["Alpha Traders", "-11800.00"],
                ["Sales Account", "10000.00"],
                ["Output CGST 9%", "900.00"],
                ["Output SGST 9%", "900.00"],
            ],
        }
    }

    fn party(mut self, party: &'static str) -> Self {
        self.rows[0][0] = party;
        self.party = Some(party);
        self
    }

    fn voucher_type(mut self, voucher_type: &'static str) -> Self {
        self.voucher_type = voucher_type;
        self
    }

    fn remote_id(mut self, remote_id: &'static str) -> Self {
        self.remote_id = Some(remote_id);
        self
    }

    fn rows(mut self, rows: Vec<[&'static str; 2]>) -> Self {
        self.rows = rows;
        self
    }

    fn build(&self) -> ProposedVoucher {
        let entries = entries(&self.rows);
        ProposedVoucher::new(ProposedVoucherInput {
            position: self.position,
            date: self.date,
            voucher_type: self.voucher_type,
            voucher_number: self.number,
            remote_id: self.remote_id,
            party: self.party,
            entries: &entries,
        })
        .expect("proposed voucher")
    }
}

fn window(rows: &[BookRow]) -> BookWindow {
    BookWindow::observed(
        "20260801",
        "20260831",
        WindowRead::Complete,
        RemoteIdEvidence::Observed,
        rows.iter().map(BookRow::build).collect(),
    )
    .expect("window")
}

fn numbering(method: NumberingMethod) -> NumberingDeclaration {
    NumberingDeclaration::new([("Sales", method)]).expect("numbering")
}

fn run(
    window: &BookWindow,
    catalog: &MasterCatalog,
    numbering: &NumberingDeclaration,
    proposals: &[ProposedVoucher],
) -> PresenceReport {
    let request = PresenceRequest::new(window, catalog, numbering, proposals).expect("request");
    assess(&request)
}

fn only(report: &PresenceReport) -> &VoucherPresence {
    assert_eq!(report.vouchers().len(), 1);
    &report.vouchers()[0]
}

fn reason(entry: &VoucherPresence) -> UndecidedReason {
    entry.undecided().expect("undecided").reason
}

// --- the window is a claim about a window ------------------------------

#[test]
fn a_partial_read_can_never_become_a_window() {
    let error = BookWindow::observed(
        "20260801",
        "20260831",
        WindowRead::Partial,
        RemoteIdEvidence::Observed,
        Vec::new(),
    )
    .expect_err("a partial read is not a window");
    assert_eq!(error, PresenceError::WindowIncomplete);
    assert_eq!(error.safe_reason_code(), "presence_window_incomplete");
}

#[test]
fn an_empty_complete_window_is_legal_and_reports_everything_absent() {
    let window = window(&[]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert!(only(&report).is_absent());
    assert_eq!(report.totals().absent, 1);
}

#[test]
fn a_window_refuses_a_voucher_dated_outside_its_own_range() {
    let outside = BookRow::new("book-1", "20260901", "AA0118").build();
    assert_eq!(
        BookWindow::observed(
            "20260801",
            "20260831",
            WindowRead::Complete,
            RemoteIdEvidence::Observed,
            vec![outside]
        )
        .expect_err("outside"),
        PresenceError::WindowVoucherOutsideRange
    );
}

#[test]
fn a_window_refuses_the_same_voucher_key_twice() {
    let rows = vec![
        BookRow::new("book-1", "20260812", "AA0118").build(),
        BookRow::new("book-1", "20260813", "AA0119").build(),
    ];
    assert_eq!(
        BookWindow::observed(
            "20260801",
            "20260831",
            WindowRead::Complete,
            RemoteIdEvidence::Observed,
            rows
        )
        .expect_err("duplicate"),
        PresenceError::WindowDuplicateVoucherKey
    );
}

#[test]
fn a_window_refuses_an_inverted_range() {
    assert_eq!(
        BookWindow::observed(
            "20260831",
            "20260801",
            WindowRead::Complete,
            RemoteIdEvidence::Observed,
            Vec::new()
        )
        .expect_err("inverted"),
        PresenceError::WindowRangeInvalid
    );
}

#[test]
fn a_proposal_outside_the_window_is_refused_rather_than_judged() {
    let window = window(&[]);
    let proposals = [ProposalRow::new(0, "20260902", "AA0118").build()];
    assert_eq!(
        PresenceRequest::new(
            &window,
            &catalog(),
            &numbering(NumberingMethod::Manual),
            &proposals
        )
        .expect_err("uncovered"),
        PresenceError::WindowDoesNotCover
    );
}

#[test]
fn every_verdict_is_scoped_to_the_window_it_names() {
    let window = window(&[]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(report.window(), ("20260801", "20260831"));
}

// --- the numbering method is declared ----------------------------------

#[test]
fn an_undeclared_numbering_method_is_an_error_not_a_default() {
    let window = window(&[]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .voucher_type("Part and Labour Sale")
        .build()];
    assert_eq!(
        PresenceRequest::new(
            &window,
            &catalog(),
            &numbering(NumberingMethod::Manual),
            &proposals
        )
        .expect_err("undeclared"),
        PresenceError::NumberingMethodUndeclared
    );
}

#[test]
fn declaring_one_voucher_type_two_ways_is_refused() {
    assert_eq!(
        NumberingDeclaration::new([
            ("Sales", NumberingMethod::Manual),
            ("sales", NumberingMethod::Automatic),
        ])
        .expect_err("conflict"),
        PresenceError::NumberingMethodConflict
    );
}

#[test]
fn a_repeated_identical_declaration_is_accepted() {
    assert!(NumberingDeclaration::new([
        ("Sales", NumberingMethod::Manual),
        ("Sales", NumberingMethod::Manual),
    ])
    .is_ok());
}

// --- only identity produces Present ------------------------------------

#[test]
fn a_manual_voucher_number_unique_on_both_sides_decides_presence() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(entry.present_book_key(), Some("book-1"));
    assert!(matches!(
        entry.status,
        PresenceStatus::Present {
            basis: PresenceBasis::ManualVoucherNumber,
            ..
        }
    ));
}

#[test]
fn a_voucher_number_decides_nothing_under_automatic_numbering() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &proposals,
    );
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::NumberNotDecisive);
}

#[test]
fn a_voucher_number_decides_nothing_under_unknown_numbering() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Unknown),
        &proposals,
    );
    assert_eq!(reason(only(&report)), UndecidedReason::NumberNotDecisive);
}

#[test]
fn a_voucher_number_is_compared_on_the_same_key_as_a_master_name() {
    let window = window(&[BookRow::new("book-1", "20260812", "aa-0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA-0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(only(&report).present_book_key(), Some("book-1"));
}

#[test]
fn a_number_carried_by_two_book_vouchers_decides_nothing() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        BookRow::new("book-2", "20260814", "AA0118").party("Bravo Industries"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(reason(entry), UndecidedReason::BookNumberCollision);
    assert_eq!(entry.undecided().expect("undecided").candidates.len(), 2);
}

#[test]
fn a_number_claimed_by_two_proposals_decides_nothing() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0118").build(),
        ProposalRow::new(1, "20260813", "AA0118")
            .party("Bravo Industries")
            .build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    for entry in report.vouchers() {
        assert_eq!(reason(entry), UndecidedReason::ProposalNumberCollision);
    }
}

#[test]
fn a_remote_id_unique_on_both_sides_decides_presence() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("bridge-txn-1")]);
    let proposals = [ProposalRow::new(0, "20260814", "AA9999")
        .remote_id("bridge-txn-1")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &proposals,
    );
    assert!(matches!(
        only(&report).status,
        PresenceStatus::Present {
            basis: PresenceBasis::RemoteId,
            ..
        }
    ));
}

#[test]
fn a_remote_id_on_two_book_vouchers_decides_nothing() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").remote_id("bridge-txn-1"),
        BookRow::new("book-2", "20260813", "AA0119").remote_id("bridge-txn-1"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .remote_id("bridge-txn-1")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(reason(only(&report)), UndecidedReason::RemoteIdCollision);
}

#[test]
fn an_absent_remote_id_column_is_reported_so_no_match_is_not_read_as_evidence() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert!(!report.observations().remote_id_observed);
}

#[test]
fn a_manual_number_never_decides_across_an_unobserved_voucher_type() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").voucher_type("Parts Sale")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .voucher_type("Part Sale")
        .build()];
    let declaration =
        NumberingDeclaration::new([("Part Sale", NumberingMethod::Manual)]).expect("numbering");
    let report = run(&window, &catalog(), &declaration, &proposals);
    let entry = only(&report);
    assert!(!entry.voucher_type_observed);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::VoucherTypeNotObserved);
    assert_eq!(entry.undecided().expect("undecided").candidates.len(), 1);
}

// --- cancelled and optional vouchers -----------------------------------

#[test]
fn an_identity_match_on_a_cancelled_voucher_is_never_present() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").cancelled()]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::MatchedVoucherNotPosted);
}

#[test]
fn an_identity_match_on_an_optional_voucher_is_never_present() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").optional()]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        reason(only(&report)),
        UndecidedReason::MatchedVoucherNotPosted
    );
}

// --- Present reports what disagrees ------------------------------------

#[test]
fn a_present_voucher_reports_an_amount_the_book_posted_short() {
    // One 9% head dropped when the voucher was keyed by hand.
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").rows(vec![
        ["Alpha Traders", "-10900.00"],
        ["Sales Account", "10000.00"],
        ["Output CGST 9%", "900.00"],
    ])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].field, DifferenceField::Amount);
    // Magnitudes are reported in canonical exact-decimal form, so a scale-only
    // difference between two readings of one amount is never a difference.
    assert_eq!(differences[0].proposed.as_deref(), Some("11800"));
    assert_eq!(differences[0].observed.as_deref(), Some("10900"));
}

#[test]
fn a_present_voucher_reports_a_date_the_book_disagrees_with() {
    let window = window(&[BookRow::new("book-1", "20260825", "AA0309")]);
    let proposals = [ProposalRow::new(0, "20260829", "AA0309").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].field, DifferenceField::Date);
}

#[test]
fn a_present_voucher_reports_a_party_the_book_disagrees_with() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").party("Bravo Industries")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    let party = differences
        .iter()
        .find(|difference| difference.field == DifferenceField::Party)
        .expect("party difference");
    assert_eq!(party.proposed.as_deref(), Some("Alpha Traders"));
    assert_eq!(party.observed.as_deref(), Some("Bravo Industries"));
}

#[test]
fn an_agreeing_present_voucher_reports_no_differences() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    assert!(differences.is_empty());
}

// --- resemblance never decides -----------------------------------------

#[test]
fn date_party_and_amount_together_still_only_resemble() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::ResemblesBookVoucher);
    assert_eq!(
        entry.undecided().expect("undecided").candidates[0].rule,
        CandidateRule::SameDatePartyAmount
    );
}

#[test]
fn an_amount_recurring_across_unrelated_parties_only_resembles() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").party("Bravo Industries")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(
        entry.undecided().expect("undecided").candidates[0].rule,
        CandidateRule::SameDateAmount
    );
}

#[test]
fn no_candidate_is_marked_best_and_no_score_is_emitted() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        BookRow::new("book-2", "20260812", "AA0119"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let serialized = serde_json::to_value(only(&report)).expect("serialize");
    let object = serialized.as_object().expect("object");
    // The refusal carries no field that names a match. A status does not
    // disarm a value printed beside it, so there must be no such value.
    assert_eq!(
        object.get("presence").and_then(serde_json::Value::as_str),
        Some("possibly_present")
    );
    for forbidden in [
        "book_key",
        "basis",
        "best",
        "score",
        "preferred",
        "suggested",
    ] {
        assert!(object.get(forbidden).is_none(), "{forbidden} leaked");
    }
    let text = serialized.to_string();
    assert!(!text.contains("score"));
}

#[test]
fn candidates_are_ordered_by_rule_then_key_and_never_by_similarity() {
    let window = window(&[
        BookRow::new("book-2", "20260812", "AA0118"),
        BookRow::new("book-1", "20260819", "AA0119"),
    ]);
    // book-2 shares date, party and amount; book-1 shares party and amount.
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let candidates = &only(&report).undecided().expect("undecided").candidates;
    assert_eq!(candidates[0].rule, CandidateRule::SameDatePartyAmount);
    assert_eq!(candidates[0].book_key, "book-2");
    assert_eq!(candidates[1].rule, CandidateRule::SamePartyAmount);
}

// --- party matching is master_binding -----------------------------------

#[test]
fn a_party_binds_on_an_embedded_identifier_before_any_name() {
    let names = [
        "Alpha (5550000001)",
        "Alpha Traders",
        "Sales Account",
        "Output CGST 9%",
        "Output SGST 9%",
    ];
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")
        .party("Alpha (5550000001)")
        .rows(vec![
            ["Alpha (5550000001)", "-11800.00"],
            ["Sales Account", "10000.00"],
            ["Output CGST 9%", "900.00"],
            ["Output SGST 9%", "900.00"],
        ])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .party("ALPHA. BRAVO 5550000001")
        .rows(vec![
            ["ALPHA. BRAVO 5550000001", "-11800.00"],
            ["Sales Account", "10000.00"],
            ["Output CGST 9%", "900.00"],
            ["Output SGST 9%", "900.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog_of(&names),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(
        entry.party,
        PartyOutcome::Bound {
            catalog_name: "Alpha (5550000001)".to_string()
        }
    );
    assert_eq!(
        entry.undecided().expect("undecided").candidates[0].rule,
        CandidateRule::SameDatePartyAmount
    );
}

#[test]
fn an_ambiguous_party_is_compared_against_every_candidate_never_one() {
    let names = [
        "Delta Trading Company",
        "Delta Trading Corporation",
        "Sales Account",
        "Output CGST 9%",
        "Output SGST 9%",
    ];
    let window = window(&[BookRow::new("book-1", "20260819", "AA0118")
        .party("Delta Trading Corporation")
        .rows(vec![
            ["Delta Trading Corporation", "-11800.00"],
            ["Sales Account", "10000.00"],
            ["Output CGST 9%", "900.00"],
            ["Output SGST 9%", "900.00"],
        ])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .party("Delta Trading")
        .rows(vec![
            ["Delta Trading", "-11800.00"],
            ["Sales Account", "10000.00"],
            ["Output CGST 9%", "900.00"],
            ["Output SGST 9%", "900.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog_of(&names),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(matches!(entry.party, PartyOutcome::Ambiguous { .. }));
    // The date differs, so only a party-and-amount rule can fire — and it only
    // fires because both candidate names were compared.
    assert_eq!(
        entry.undecided().expect("undecided").candidates[0].rule,
        CandidateRule::SamePartyAmount
    );
}

#[test]
fn a_party_with_nothing_resembling_it_still_permits_absent() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .party("Zulu Enterprises")
        .rows(vec![
            ["Zulu Enterprises", "-22500.00"],
            ["Sales Account", "22500.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(matches!(entry.party, PartyOutcome::Unmatched { .. }));
    assert!(entry.is_absent());
}

#[test]
fn a_party_name_family_withholds_absent_because_the_comparison_never_ran() {
    let mut names: Vec<String> = (1..=30)
        .map(|index| format!("Echo Party {index:03}"))
        .collect();
    names.extend(
        ["Sales Account", "Output CGST 9%", "Output SGST 9%"]
            .iter()
            .map(|name| (*name).to_string()),
    );
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("catalog");
    let window = window(&[]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .party("Echo Party 0")
        .rows(vec![
            ["Echo Party 0", "-22500.00"],
            ["Sales Account", "22500.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog,
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(!entry.is_absent(), "an uncompared family is not an absence");
    assert_eq!(reason(entry), UndecidedReason::PartyNotDecidable);
    assert!(entry.undecided().expect("undecided").candidates.is_empty());
}

#[test]
fn a_proposal_without_a_party_still_runs_the_party_independent_rules() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let mut proposal = ProposalRow::new(0, "20260812", "AA0999");
    proposal.party = None;
    let proposals = [proposal.build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(entry.party, PartyOutcome::NotSupplied);
    assert_eq!(
        entry.undecided().expect("undecided").candidates[0].rule,
        CandidateRule::SameDateAmount
    );
}

// --- magnitude ---------------------------------------------------------

#[test]
fn both_sides_derive_one_magnitude_from_the_same_entries() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").rows(vec![
        ["Alpha Traders", "-11800"],
        ["Sales Account", "10000.00"],
        ["Output CGST 9%", "900.000"],
        ["Output SGST 9%", "900"],
    ])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        only(&report).undecided().expect("undecided").candidates[0].rule,
        CandidateRule::SameDatePartyAmount
    );
}

#[test]
fn an_unbalanced_book_voucher_is_reported_and_still_matched() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").rows(vec![
        ["Alpha Traders", "-11800.00"],
        ["Sales Account", "10000.00"],
        ["Output CGST 9%", "900.00"],
    ])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(report.observations().unbalanced_voucher_count, 1);
    assert_eq!(report.observations().unbalanced_vouchers, vec!["book-1"]);
    assert_eq!(only(&report).present_book_key(), Some("book-1"));
}

// --- book observations --------------------------------------------------

#[test]
fn duplicate_voucher_numbers_in_the_book_are_reported_without_being_asked_for() {
    let window = window(&[
        BookRow::new("book-1", "20260803", "AA0118"),
        BookRow::new("book-2", "20260814", "AA0118").party("Bravo Industries"),
        BookRow::new("book-3", "20260815", "AA0120").party("Charlie Minerals"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let observations = report.observations();
    assert_eq!(observations.duplicate_number_group_count, 1);
    assert!(!observations.duplicate_numbers_truncated);
    let group = &observations.duplicate_numbers[0];
    assert_eq!(group.voucher_number, "AA0118");
    assert_eq!(group.book_voucher_count, 2);
    assert_eq!(group.book_keys, vec!["book-1", "book-2"]);
}

#[test]
fn book_vouchers_no_proposal_reached_are_counted() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        BookRow::new("book-2", "20260819", "AA0119").party("Charlie Minerals"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(report.observations().unmatched_book_vouchers, 1);
    assert_eq!(report.observations().window_voucher_count, 2);
}

// --- control totals -----------------------------------------------------

#[test]
fn totals_reconcile_a_mixed_run_and_reproduce_the_engagement_that_blocked() {
    // Twenty invoices proposed; fifteen already keyed by hand.
    let book: Vec<BookRow> = (1..=15)
        .map(|index| {
            BookRow::new(
                Box::leak(format!("book-{index}").into_boxed_str()),
                "20260812",
                Box::leak(format!("AA{index:04}").into_boxed_str()),
            )
        })
        .collect();
    let window = window(&book);
    let proposals: Vec<ProposedVoucher> = (1..=20)
        .map(|index| {
            ProposalRow::new(
                index - 1,
                "20260812",
                Box::leak(format!("AA{index:04}").into_boxed_str()),
            )
            .rows(vec![
                ["Alpha Traders", "-11800.00"],
                ["Sales Account", "10000.00"],
                ["Output CGST 9%", "900.00"],
                ["Output SGST 9%", "900.00"],
            ])
            .build()
        })
        .collect();
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let totals = report.totals();
    assert_eq!(totals.requested, 20);
    assert_eq!(totals.present, 15);
    // The five that are not in the book all resemble the fifteen that are —
    // same party, same date, same amount — so none is silently absent.
    assert_eq!(totals.present + totals.possibly_present + totals.absent, 20);
    assert_eq!(totals.possibly_present, 5);
    assert_eq!(totals.absent, 0);
}

#[test]
fn totals_always_partition_the_requested_set() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        BookRow::new("book-2", "20260812", "AA0119").party("Bravo Industries"),
    ]);
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0118").build(),
        ProposalRow::new(1, "20260813", "AA0125")
            .party("Charlie Minerals")
            .build(),
        ProposalRow::new(2, "20260812", "AA0126").build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let totals = report.totals();
    assert_eq!(totals.requested, 3);
    assert_eq!(
        totals.present + totals.possibly_present + totals.absent,
        totals.requested
    );
    assert_eq!(report.absent().count(), totals.absent);
    assert_eq!(report.present().count(), totals.present);
    assert_eq!(report.possibly_present().count(), totals.possibly_present);
}

// --- boundary refusals --------------------------------------------------

#[test]
fn an_empty_proposal_set_is_refused() {
    let window = window(&[]);
    assert_eq!(
        PresenceRequest::new(
            &window,
            &catalog(),
            &numbering(NumberingMethod::Manual),
            &[]
        )
        .expect_err("empty"),
        PresenceError::ProposalsEmpty
    );
}

#[test]
fn a_stock_item_catalog_cannot_be_used_to_compare_parties() {
    let catalog = MasterCatalog::new(MasterClass::StockItem, LEDGERS).expect("catalog");
    let window = window(&[]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    assert_eq!(
        PresenceRequest::new(
            &window,
            &catalog,
            &numbering(NumberingMethod::Manual),
            &proposals
        )
        .expect_err("class"),
        PresenceError::CatalogClassInvalid
    );
}

#[test]
fn observed_input_refuses_blank_unsafe_and_invalid_fields() {
    let rows = [["Alpha Traders", "-1.00"], ["Sales Account", "1.00"]];
    let good = entries(&rows);
    let base = ObservedVoucher {
        key: "book-1",
        date: "20260812",
        voucher_type: "Sales",
        voucher_number: Some("AA0118"),
        remote_id: None,
        party: Some("Alpha Traders"),
        entries: &good,
        cancelled: false,
        optional: false,
    };
    assert_eq!(
        BookVoucher::observed(ObservedVoucher { key: "  ", ..base }).expect_err("blank"),
        PresenceError::TextBlank
    );
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            voucher_type: "Sales\u{0007}",
            ..base
        })
        .expect_err("unsafe"),
        PresenceError::TextUnsafe
    );
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            date: "2026-08-12",
            ..base
        })
        .expect_err("date"),
        PresenceError::DateInvalid
    );
    let bad = [["Alpha Traders", "one thousand"]];
    let bad = entries(&bad);
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            entries: &bad,
            ..base
        })
        .expect_err("amount"),
        PresenceError::AmountInvalid
    );
}

#[test]
fn every_error_carries_a_distinct_stable_reason_code() {
    let codes = [
        PresenceError::WindowIncomplete,
        PresenceError::WindowRangeInvalid,
        PresenceError::WindowTooLarge,
        PresenceError::WindowVoucherOutsideRange,
        PresenceError::WindowDuplicateVoucherKey,
        PresenceError::WindowDoesNotCover,
        PresenceError::ProposalsEmpty,
        PresenceError::TooManyProposals,
        PresenceError::TooManyEntries,
        PresenceError::NumberingMethodUndeclared,
        PresenceError::NumberingMethodConflict,
        PresenceError::TextBlank,
        PresenceError::TextTooLong,
        PresenceError::TextUnsafe,
        PresenceError::DateInvalid,
        PresenceError::AmountInvalid,
        PresenceError::CatalogClassInvalid,
    ]
    .iter()
    .map(PresenceError::safe_reason_code)
    .collect::<BTreeSet<_>>();
    assert_eq!(codes.len(), 17);
    assert!(codes.iter().all(|code| code.starts_with("presence_")));
}

#[test]
fn every_undecided_reason_carries_a_distinct_stable_code() {
    let codes = [
        UndecidedReason::RemoteIdCollision,
        UndecidedReason::BookNumberCollision,
        UndecidedReason::ProposalNumberCollision,
        UndecidedReason::MatchedVoucherNotPosted,
        UndecidedReason::NumberNotDecisive,
        UndecidedReason::VoucherTypeNotObserved,
        UndecidedReason::ResemblesBookVoucher,
        UndecidedReason::PartyNotDecidable,
    ]
    .iter()
    .map(|reason| reason.safe_reason_code())
    .collect::<BTreeSet<_>>();
    assert_eq!(codes.len(), 8);
}

// --- the aggregate candidate budget is a second source of "incomplete" ------

/// `master_binding` spends an aggregate candidate-byte budget in entity order
/// while the report is built, so an entity's candidate list can arrive **empty
/// with `candidates_truncated`** for a reason that has nothing to do with its
/// own name — pressure from earlier entities in the same report. That is a new
/// source of a signal this contract already acts on, and the danger is reading
/// "no candidates" as "nothing in this book resembles this party".
#[test]
fn an_aggregately_truncated_candidate_list_withholds_absent() {
    let binding = master_binding::EntityBinding {
        position: 0,
        source_name: "Delta Trading".to_string(),
        status: BindingStatus::Ambiguous(master_binding::Unresolved {
            reason: master_binding::UnboundReason::NearMiss,
            unresolved_identity: Vec::new(),
            // Empty, yet seven candidates were found before the budget ran
            // out. Under the typed listing this is `Truncated` with nothing
            // listed, which is now a state the compiler makes me handle.
            candidates: master_binding::Candidates::Truncated {
                listed: Vec::new(),
                found: 7,
            },
        }),
    };
    let resolution = resolution_of(&binding);
    assert!(
        resolution.compare_keys.is_empty(),
        "no name survived to be compared"
    );
    assert!(
        resolution.incomplete,
        "names that were never compared cannot license an absence"
    );
}

/// The contrast that keeps the rule honest: a party with no candidates and no
/// truncation genuinely has nothing resembling it in the catalog, so no posted
/// voucher can be carrying it and `Absent` stays available.
#[test]
fn an_untruncated_empty_candidate_list_still_permits_absent() {
    let binding = master_binding::EntityBinding {
        position: 0,
        source_name: "Zulu Enterprises".to_string(),
        status: BindingStatus::Unmatched(master_binding::Unresolved {
            reason: master_binding::UnboundReason::NoCandidate,
            unresolved_identity: Vec::new(),
            candidates: master_binding::Candidates::None,
        }),
    };
    let resolution = resolution_of(&binding);
    assert!(resolution.compare_keys.is_empty());
    assert!(!resolution.incomplete);
}

/// The overloaded-empty-vector shape has been got wrong twice in two surfaces,
/// so this contract states what its own empty list means — and the honest
/// statement is not "one thing". An empty list is always a *proposal-side*
/// condition: the book was never consulted, or was consulted about something
/// undecidable before it could point anywhere. What it never means is
/// "nothing in the book resembles this" — only `Absent` means that, and
/// `Absent` carries no list at all.
///
/// An earlier version of this test asserted the stronger claim that empty
/// implies `PartyNotDecidable`. That was false the moment a second empty-list
/// reason existed, and it passed only because no case exercised one. The
/// allow-list below is the real rule and fails closed: a new reason that can
/// arrive empty must be added here deliberately.
const EMPTY_LIST_REASONS: [UndecidedReason; 4] = [
    UndecidedReason::PartyNotDecidable,
    UndecidedReason::RemoteIdEvidenceUnavailable,
    UndecidedReason::ProposalNumberCollision,
    UndecidedReason::RemoteIdCollision,
];

#[test]
fn an_empty_candidate_list_is_always_a_proposal_side_condition() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let mut names: Vec<String> = (1..=30)
        .map(|index| format!("Echo Party {index:03}"))
        .collect();
    names.extend(LEDGERS.iter().map(|name| (*name).to_string()));
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("catalog");
    let money = |ledger: &'static str| vec![[ledger, "-99.00"], ["Sales Account", "99.00"]];
    let proposals = [
        // Party family that cannot be distinguished.
        ProposalRow::new(0, "20260812", "AA0501")
            .party("Echo Party 0")
            .rows(money("Echo Party 0"))
            .build(),
        // Two proposals sharing a manual number the book does not hold.
        ProposalRow::new(1, "20260812", "AA0502")
            .party("Charlie Minerals")
            .rows(money("Charlie Minerals"))
            .build(),
        ProposalRow::new(2, "20260812", "AA0502")
            .party("Charlie Minerals")
            .rows(money("Charlie Minerals"))
            .build(),
        // Two proposals sharing a REMOTEID the book does not hold.
        ProposalRow::new(3, "20260812", "AA0503")
            .remote_id("tally-9")
            .party("Charlie Minerals")
            .rows(money("Charlie Minerals"))
            .build(),
        ProposalRow::new(4, "20260812", "AA0504")
            .remote_id("tally-9")
            .party("Charlie Minerals")
            .rows(money("Charlie Minerals"))
            .build(),
    ];
    let report = run(
        &window,
        &catalog,
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let mut empties = 0;
    for entry in report.vouchers() {
        let Some(undecided) = entry.undecided() else {
            continue;
        };
        if undecided.candidates.is_empty() {
            empties += 1;
            assert!(
                EMPTY_LIST_REASONS.contains(&undecided.reason),
                "{:?} may not arrive with an empty candidate list",
                undecided.reason
            );
            assert!(!undecided.candidates_truncated);
        } else {
            assert_eq!(
                undecided.candidates_truncated,
                undecided.candidates.len() < undecided.candidate_count
            );
        }
    }
    assert_eq!(empties, 5, "every empty-list reason must be exercised here");
    let totals = report.totals();
    assert_eq!(
        totals.present + totals.possibly_present + totals.absent,
        totals.requested
    );
}

/// Two source rows claiming one identity are undecidable whether or not the
/// book holds that identity. Consulting the book first let both fall through
/// to a resemblance verdict, or to `Absent` — reporting colliding rows as safe
/// to import.
#[test]
fn proposals_sharing_a_remote_id_collide_even_when_the_book_has_none() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0601")
            .remote_id("tally-9")
            .party("Charlie Minerals")
            .rows(vec![
                ["Charlie Minerals", "-42.00"],
                ["Sales Account", "42.00"],
            ])
            .build(),
        ProposalRow::new(1, "20260812", "AA0602")
            .remote_id("tally-9")
            .party("Charlie Minerals")
            .rows(vec![
                ["Charlie Minerals", "-43.00"],
                ["Sales Account", "43.00"],
            ])
            .build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        report.totals().absent,
        0,
        "colliding rows are not safe to import"
    );
    for entry in report.vouchers() {
        assert_eq!(reason(entry), UndecidedReason::RemoteIdCollision);
    }
}

/// The overloaded-empty-vector shape has now been got wrong twice in two
/// surfaces, so this contract's *own* output must not repeat it. Here an empty
/// candidate list is one fact and not three: it happens only when the party
/// comparison could not run, and truncation only ever cuts a list that is
/// otherwise full. Held by construction today; held by test from now on.
#[test]
fn an_empty_candidate_list_means_exactly_one_thing_in_this_contract() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        BookRow::new("book-2", "20260812", "AA0118").party("Bravo Industries"),
        BookRow::new("book-3", "20260819", "AA0130").party("Charlie Minerals"),
    ]);
    let mut names: Vec<String> = (1..=30)
        .map(|index| format!("Echo Party {index:03}"))
        .collect();
    names.extend(LEDGERS.iter().map(|name| (*name).to_string()));
    let catalog = MasterCatalog::new(MasterClass::Ledger, &names).expect("catalog");
    let proposals = [
        // Collides on a number carried by two book vouchers.
        ProposalRow::new(0, "20260812", "AA0118").build(),
        // Resembles on date, party and amount.
        ProposalRow::new(1, "20260812", "AA0777").build(),
        // Party is an undistinguishable family: withheld, not absent.
        ProposalRow::new(2, "20260812", "AA0778")
            .party("Echo Party 0")
            .rows(vec![["Echo Party 0", "-99.00"], ["Sales Account", "99.00"]])
            .build(),
        // Nothing resembles it at all.
        ProposalRow::new(3, "20260812", "AA0779")
            .party("Charlie Minerals")
            .rows(vec![
                ["Charlie Minerals", "-13.00"],
                ["Sales Account", "13.00"],
            ])
            .build(),
    ];
    let report = run(
        &window,
        &catalog,
        &numbering(NumberingMethod::Manual),
        &proposals,
    );

    let mut seen_empty = 0;
    for entry in report.vouchers() {
        let Some(undecided) = entry.undecided() else {
            continue;
        };
        if undecided.candidates.is_empty() {
            seen_empty += 1;
            assert_eq!(
                undecided.reason,
                UndecidedReason::PartyNotDecidable,
                "an empty candidate list may only mean the comparison did not run"
            );
            assert!(!undecided.candidates_truncated);
            assert_eq!(undecided.candidate_count, 0);
        } else {
            // A listed count and a true count that disagree must say so.
            assert_eq!(
                undecided.candidates_truncated,
                undecided.candidates.len() < undecided.candidate_count
            );
        }
    }
    assert_eq!(seen_empty, 1, "the withheld-family case must be exercised");
    // And the whole run still partitions.
    let totals = report.totals();
    assert_eq!(
        totals.present + totals.possibly_present + totals.absent,
        totals.requested
    );
}

// --- one book voucher satisfies at most one proposal --------------------

#[test]
fn two_proposals_reaching_one_book_voucher_are_both_demoted() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("bridge-txn-1")]);
    let proposals = [
        // Reaches book-1 by REMOTEID.
        ProposalRow::new(0, "20260812", "AA9999")
            .remote_id("bridge-txn-1")
            .build(),
        // Reaches the same voucher by its manual number.
        ProposalRow::new(1, "20260812", "AA0118").build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    // Neither may be excluded from an import: only one voucher exists.
    assert_eq!(report.totals().present, 0);
    for entry in report.vouchers() {
        assert_eq!(reason(entry), UndecidedReason::BookVoucherClaimedTwice);
        assert_eq!(
            entry.undecided().expect("undecided").candidates[0].book_key,
            "book-1"
        );
    }
}

#[test]
fn distinct_proposals_reaching_distinct_vouchers_both_stay_present() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").remote_id("bridge-txn-1"),
        BookRow::new("book-2", "20260813", "AA0119").party("Bravo Industries"),
    ]);
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0118").build(),
        ProposalRow::new(1, "20260813", "AA0119")
            .party("Bravo Industries")
            .build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(report.totals().present, 2);
}

// --- two identity signals that disagree ---------------------------------

#[test]
fn a_number_match_contradicted_by_a_different_remote_id_does_not_settle() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .remote_id("tally-2")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::IdentityConflict);
}

#[test]
fn a_number_match_agreeing_with_the_remote_id_still_settles() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .remote_id("tally-1")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    // The REMOTEID decides it first; either basis is an identity.
    assert!(only(&report).present_book_key().is_some());
}

// --- a key that was never read is not a key that found nothing ----------

#[test]
fn a_remote_id_the_window_never_read_withholds_absent() {
    let unread = BookWindow::observed(
        "20260801",
        "20260831",
        WindowRead::Complete,
        RemoteIdEvidence::NotRead,
        vec![BookRow::new("book-1", "20260819", "AA0130")
            .party("Bravo Industries")
            .build()],
    )
    .expect("window");
    let proposals = [ProposalRow::new(0, "20260812", "AA0777")
        .remote_id("tally-1")
        .party("Charlie Minerals")
        .rows(vec![
            ["Charlie Minerals", "-55.00"],
            ["Sales Account", "55.00"],
        ])
        .build()];
    let report = run(
        &unread,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(
        !entry.is_absent(),
        "the proposal's strongest key was never compared"
    );
    assert_eq!(reason(entry), UndecidedReason::RemoteIdEvidenceUnavailable);
}

#[test]
fn the_same_proposal_is_absent_when_the_window_did_read_remote_ids() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0777")
        .remote_id("tally-1")
        .party("Charlie Minerals")
        .rows(vec![
            ["Charlie Minerals", "-55.00"],
            ["Sales Account", "55.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert!(only(&report).is_absent());
}

// --- the response cap must not distort the observations -----------------

#[test]
fn candidates_dropped_by_the_response_cap_still_count_as_reached() {
    let rows: Vec<BookRow> = (1..=30)
        .map(|index| {
            BookRow::new(
                Box::leak(format!("book-{index:02}").into_boxed_str()),
                "20260812",
                Box::leak(format!("BB{index:04}").into_boxed_str()),
            )
        })
        .collect();
    let window = window(&rows);
    let proposals = [ProposalRow::new(0, "20260812", "AA0777").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let undecided = only(&report).undecided().expect("undecided");
    assert_eq!(undecided.candidate_count, 30);
    assert!(undecided.candidates_truncated);
    assert_eq!(undecided.candidates.len(), MAX_CANDIDATES_PER_PROPOSAL);
    // All thirty were reached; none may be reported as untouched merely
    // because the response could not carry it.
    assert_eq!(report.observations().unmatched_book_vouchers, 0);
}

// --- the party diagnostic reads the party field -------------------------

#[test]
fn a_party_difference_compares_the_observed_party_field_not_every_ledger() {
    // PARTYLEDGERNAME is Bravo while the entries still name Alpha.
    let window =
        window(&[BookRow::new("book-1", "20260812", "AA0118").party_field("Bravo Industries")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    let party = differences
        .iter()
        .find(|difference| difference.field == DifferenceField::Party)
        .expect("the party field disagrees and must be reported");
    assert_eq!(party.proposed.as_deref(), Some("Alpha Traders"));
    assert_eq!(party.observed.as_deref(), Some("Bravo Industries"));
}

#[test]
fn a_voucher_with_no_party_field_has_nothing_to_disagree_with() {
    let mut row = BookRow::new("book-1", "20260812", "AA0118");
    row.party = None;
    let window = window(&[row]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    assert!(differences.is_empty());
}

/// `Present` carries the higher bar, so unobserved evidence that could
/// *contradict* it must fail toward not-present. A number match while the
/// proposal's own `REMOTEID` was never compared settles on one identity while
/// the other is unknown — and a wrong `Present` suppresses a real invoice.
#[test]
fn a_number_match_cannot_settle_while_the_proposals_remote_id_is_unread() {
    let unread = BookWindow::observed(
        "20260801",
        "20260831",
        WindowRead::Complete,
        RemoteIdEvidence::NotRead,
        vec![BookRow::new("book-1", "20260812", "AA0118").build()],
    )
    .expect("window");
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .remote_id("tally-1")
        .build()];
    let report = run(
        &unread,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::RemoteIdEvidenceUnavailable);
    // The number match is still shown, so the operator sees what it resembles.
    assert_eq!(
        entry.undecided().expect("undecided").candidates[0].book_key,
        "book-1"
    );
}

#[test]
fn a_proposal_without_a_remote_id_still_settles_on_an_unread_window() {
    let unread = BookWindow::observed(
        "20260801",
        "20260831",
        WindowRead::Complete,
        RemoteIdEvidence::NotRead,
        vec![BookRow::new("book-1", "20260812", "AA0118").build()],
    )
    .expect("window");
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &unread,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    // Nothing was skipped: this proposal carries no REMOTEID to compare.
    assert_eq!(only(&report).present_book_key(), Some("book-1"));
}

/// Both identity lookups are resolved before either settles. A `REMOTEID`
/// selecting one voucher while the number selects another is a disagreement,
/// and ranking the basis that happened to be checked first is the move this
/// contract refuses everywhere else.
#[test]
fn a_remote_id_and_a_number_selecting_different_vouchers_do_not_settle() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1"),
        BookRow::new("book-2", "20260813", "AA0119").party("Bravo Industries"),
    ]);
    let proposals = [ProposalRow::new(0, "20260813", "AA0119")
        .remote_id("tally-1")
        .party("Bravo Industries")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::IdentityConflict);
    // Both contradicting vouchers are shown, each labelled by its own rule.
    let candidates = &entry.undecided().expect("undecided").candidates;
    assert_eq!(candidates.len(), 2);
    assert!(candidates
        .iter()
        .any(|c| c.book_key == "book-1" && c.rule == CandidateRule::SharedRemoteId));
    assert!(candidates
        .iter()
        .any(|c| c.book_key == "book-2" && c.rule == CandidateRule::SharedVoucherNumber));
}

#[test]
fn a_remote_id_and_a_number_agreeing_on_one_voucher_still_settle() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .remote_id("tally-1")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(only(&report).present_book_key(), Some("book-1"));
}

/// A source that names no party had no party rule run against it, so an
/// absence rests on date and amount alone — the pair this contract says
/// collides. `Present` by identity is unaffected; only the absence is
/// withheld, and supplying the party is what makes it available again.
#[test]
fn a_proposal_that_names_no_party_cannot_be_reported_absent() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let mut proposal = ProposalRow::new(0, "20260812", "AA0777");
    proposal.party = None;
    proposal.rows = vec![["Charlie Minerals", "-55.00"], ["Sales Account", "55.00"]];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &[proposal.build()],
    );
    let entry = only(&report);
    assert_eq!(entry.party, PartyOutcome::NotSupplied);
    assert!(!entry.is_absent());
    assert_eq!(reason(entry), UndecidedReason::PartyNotSupplied);
}

#[test]
fn naming_the_party_is_what_makes_absence_available() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0777")
        .party("Charlie Minerals")
        .rows(vec![
            ["Charlie Minerals", "-55.00"],
            ["Sales Account", "55.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert!(only(&report).is_absent());
}

#[test]
fn a_proposal_that_names_no_party_still_settles_by_identity() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let mut proposal = ProposalRow::new(0, "20260812", "AA0118");
    proposal.party = None;
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &[proposal.build()],
    );
    assert_eq!(only(&report).present_book_key(), Some("book-1"));
}

/// A collision between two proposals is a fact about the source. It does not
/// become less true because the book has never seen that voucher type.
#[test]
fn proposals_sharing_a_number_collide_even_for_an_unobserved_type() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let declaration =
        NumberingDeclaration::new([("Part Sale", NumberingMethod::Manual)]).expect("numbering");
    let money = vec![["Charlie Minerals", "-61.00"], ["Sales Account", "61.00"]];
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0801")
            .voucher_type("Part Sale")
            .party("Charlie Minerals")
            .rows(money.clone())
            .build(),
        ProposalRow::new(1, "20260812", "AA0801")
            .voucher_type("Part Sale")
            .party("Charlie Minerals")
            .rows(money)
            .build(),
    ];
    let report = run(&window, &catalog(), &declaration, &proposals);
    assert_eq!(
        report.totals().absent,
        0,
        "colliding rows are not safe to import"
    );
    for entry in report.vouchers() {
        assert!(!entry.voucher_type_observed);
        assert_eq!(reason(entry), UndecidedReason::ProposalNumberCollision);
    }
}

/// A window cannot say "REMOTEID was never read" while carrying one. The two
/// statements contradict, and the contradiction would let a verdict settle on
/// evidence the window itself says was not gathered.
#[test]
fn a_window_declaring_remote_ids_unread_refuses_to_carry_one() {
    let carrying = vec![BookRow::new("book-1", "20260812", "AA0118")
        .remote_id("tally-1")
        .build()];
    assert_eq!(
        BookWindow::observed(
            "20260801",
            "20260831",
            WindowRead::Complete,
            RemoteIdEvidence::NotRead,
            carrying,
        )
        .expect_err("contradiction"),
        PresenceError::WindowRemoteIdContradiction
    );
    // The same vouchers are fine once the window admits it read the column.
    assert!(BookWindow::observed(
        "20260801",
        "20260831",
        WindowRead::Complete,
        RemoteIdEvidence::Observed,
        vec![BookRow::new("book-1", "20260812", "AA0118")
            .remote_id("tally-1")
            .build()],
    )
    .is_ok());
}

/// A dense window can hold thousands of vouchers sharing one manual number.
/// The response keeps twenty-five of them, so twenty-five is what may be
/// cloned — the count is carried alongside rather than recovered from the
/// vector's length, which is what let the old code allocate the whole set and
/// then throw it away.
#[test]
fn a_large_number_collision_reports_its_true_size_without_listing_it() {
    let rows: Vec<BookRow> = (1..=200)
        .map(|index| {
            BookRow::new(
                Box::leak(format!("book-{index:03}").into_boxed_str()),
                "20260812",
                "AA0118",
            )
        })
        .collect();
    let window = window(&rows);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(reason(entry), UndecidedReason::BookNumberCollision);
    let undecided = entry.undecided().expect("undecided");
    assert_eq!(undecided.candidate_count, 200, "the true size is reported");
    assert_eq!(undecided.candidates.len(), MAX_CANDIDATES_PER_PROPOSAL);
    assert!(undecided.candidates_truncated);
    // Every one of them was still reached, so none is reported as a voucher no
    // proposal came near.
    assert_eq!(report.observations().unmatched_book_vouchers, 0);
    // And the book-side diagnostic sees the collision it is there to find.
    assert_eq!(report.observations().duplicate_number_group_count, 1);
}
