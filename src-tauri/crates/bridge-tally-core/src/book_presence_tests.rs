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
    marker: ObservedMarker<'static>,
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
            marker: ObservedMarker::Absent,
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

    fn marker(mut self, marker: &'static str) -> Self {
        self.marker = ObservedMarker::Identifying(marker);
        self
    }

    fn unidentified_marker(mut self) -> Self {
        self.marker = ObservedMarker::Unidentified(&[]);
        self
    }

    /// A narration carrying more than one well-formed marker: it identifies
    /// nothing, and the occurrences are still evidence.
    fn ambiguous_markers(mut self, markers: &'static [&'static str]) -> Self {
        self.marker = ObservedMarker::Unidentified(markers);
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
            marker: self.marker,
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
    marker: Option<&'static str>,
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
            marker: None,
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

    fn marker(mut self, marker: &'static str) -> Self {
        self.marker = Some(marker);
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
            narration_marker: self.marker,
            party: self.party,
            entries: &entries,
        })
        .expect("proposed voucher")
    }
}

fn window(rows: &[BookRow]) -> BookWindow {
    BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: rows.iter().map(BookRow::build).collect(),
    })
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
    // These rule tests name only the ledgers relevant to their assertion. The
    // public boundary now requires a complete catalog, so complete that test
    // fixture from the already-observed window rather than weakening the
    // boundary every test reaches through this helper.
    let mut names = catalog.names().map(str::to_owned).collect::<Vec<_>>();
    names.extend(
        window
            .vouchers()
            .iter()
            .flat_map(|voucher| voucher.observed_ledgers.iter().cloned()),
    );
    names.sort();
    names.dedup();
    let complete_catalog =
        MasterCatalog::new(MasterClass::Ledger, &names).expect("complete catalog");
    let request =
        PresenceRequest::new(window, &complete_catalog, numbering, proposals).expect("request");
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
    let error = BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Partial,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: Vec::new(),
    })
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
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: vec![outside],
        })
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
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: rows,
        })
        .expect_err("duplicate"),
        PresenceError::WindowDuplicateVoucherKey
    );
}

#[test]
fn a_window_refuses_an_inverted_range() {
    assert_eq!(
        BookWindow::observed(ObservedWindow {
            from: "20260831",
            to: "20260801",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: Vec::new(),
        })
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
fn differently_spelled_voucher_types_have_independent_declarations() {
    let declaration = NumberingDeclaration::new([
        ("Sales", NumberingMethod::Manual),
        ("sales", NumberingMethod::Automatic),
    ])
    .expect("distinct exact type names");
    assert!(declaration.declares("Sales"));
    assert!(declaration.declares("sales"));
    assert!(!declaration.declares(" SALES "));
}

#[test]
fn conflicting_declarations_of_the_same_exact_voucher_type_are_refused() {
    assert_eq!(
        NumberingDeclaration::new([
            ("Sales", NumberingMethod::Manual),
            ("Sales", NumberingMethod::Automatic),
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

#[test]
fn numbering_declarations_bound_duplicate_iterator_work() {
    let entries = (0..=MAX_NUMBERING_DECLARATIONS).map(|_| ("Sales", NumberingMethod::Manual));
    assert_eq!(
        NumberingDeclaration::new(entries).expect_err("declaration count is bounded"),
        PresenceError::NumberingDeclarationsTooMany
    );
}

#[test]
fn numbering_declarations_bound_aggregate_bytes_while_consuming_duplicates() {
    let entries = (0..).map(|_| ("X".repeat(MAX_TEXT_CHARS), NumberingMethod::Manual));
    assert_eq!(
        NumberingDeclaration::new(entries).expect_err("declaration bytes are bounded"),
        PresenceError::NumberingDeclarationBytesTooLarge
    );
}

#[test]
fn aggregate_proposal_window_resemblance_work_is_refused() {
    let rows = (0..1_001)
        .map(|i| {
            let key = Box::leak(format!("book-{i}").into_boxed_str());
            BookRow::new(key, "20260812", "AA0118")
        })
        .collect::<Vec<_>>();
    let proposals = (0..1_001)
        .map(|i| {
            let number = Box::leak(format!("AA{i:04}").into_boxed_str());
            ProposalRow::new(i, "20260812", number).build()
        })
        .collect::<Vec<_>>();
    let observed = window(&rows);
    assert_eq!(
        PresenceRequest::new(
            &observed,
            &catalog(),
            &numbering(NumberingMethod::Manual),
            &proposals,
        )
        .expect_err("aggregate comparison work is bounded"),
        PresenceError::ComparisonWorkTooLarge
    );
}

#[test]
fn admitted_indexed_work_boundary_is_accepted() {
    let rows = (0..500)
        .map(|i| {
            BookRow::new(
                Box::leak(format!("book-{i}").into_boxed_str()),
                "20260812",
                Box::leak(format!("N{i}").into_boxed_str()),
            )
        })
        .collect::<Vec<_>>();
    let proposals = (0..500)
        .map(|i| {
            ProposalRow::new(i, "20260812", Box::leak(format!("P{i}").into_boxed_str())).build()
        })
        .collect::<Vec<_>>();
    let observed = window(&rows);
    assert!(PresenceRequest::new(
        &observed,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    )
    .is_ok());
}

#[test]
fn request_refuses_a_window_ledger_missing_from_its_catalog() {
    let window =
        window(&[BookRow::new("book-1", "20260812", "AA0118")
            .rows(vec![["Uncatalogued Ledger", "0.00"]])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    assert_eq!(
        PresenceRequest::new(
            &window,
            &catalog(),
            &numbering(NumberingMethod::Manual),
            &proposals
        )
        .expect_err("window ledger is absent from catalog"),
        PresenceError::CatalogWindowCoverageMissing
    );
}

#[test]
fn request_coverage_uses_the_exact_observed_spelling() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")
        .party_field("Café")
        .rows(vec![["Café", "0.00"]])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let exact = catalog_of(&["Café"]);
    assert!(PresenceRequest::new(
        &window,
        &exact,
        &numbering(NumberingMethod::Manual),
        &proposals
    )
    .is_ok());
    let folded_only = catalog_of(&["café"]);
    assert_eq!(
        PresenceRequest::new(
            &window,
            &folded_only,
            &numbering(NumberingMethod::Manual),
            &proposals
        )
        .expect_err("folded spelling is not exact coverage"),
        PresenceError::CatalogWindowCoverageMissing
    );
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

/// This asserted the opposite until a review asked what measured it.
///
/// A voucher number used the master-name key, so `aa-0118` matched `AA-0118`
/// and settled `Present`. The fold that key applies is not arbitrary -- §3.3b
/// measured Tally's own master-name matching and the key follows it -- but
/// nothing measured it for *numbers*, and borrowing the conclusion without the
/// measurement is how an assumption acquires a citation.
///
/// It also fails in the wrong direction. Folding produces more matches, a
/// wrong number match is a `Present`, and a `Present` tells a caller the
/// invoice is already filed. Two distinct invoices numbered `aa-0118` and
/// `AA-0118` would each have suppressed the other.
#[test]
fn a_voucher_number_is_not_folded_the_way_a_master_name_is() {
    let window = window(&[BookRow::new("book-1", "20260812", "aa-0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA-0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        only(&report).present_book_key(),
        None,
        "case is content in a number until a measurement says otherwise"
    );
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
fn internal_whitespace_in_a_voucher_number_is_content() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA  0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA 0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_ne!(only(&report).present_book_key(), Some("book-1"));
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
fn a_party_difference_echoes_the_source_spelling_not_its_catalog_binding() {
    let window =
        window(&[BookRow::new("book-1", "20260812", "AA0118").party_field("Bravo Industries")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .party("alpha traders")
        .build()];
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
    assert_eq!(party.proposed.as_deref(), Some("alpha traders"));
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
fn an_empty_proposal_entry_list_is_refused_at_the_core_boundary() {
    assert_eq!(
        ProposedVoucher::new(ProposedVoucherInput {
            position: 0,
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: Some("AA0118"),
            remote_id: None,
            narration_marker: None,
            party: None,
            entries: &[],
        })
        .expect_err("empty accounting data"),
        PresenceError::EntriesEmpty
    );
}

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

#[test]
fn unbalanced_observation_listing_is_sorted_before_its_cap() {
    let rows = [
        "book-z", "book-a", "book-b", "book-c", "book-d", "book-e", "book-f", "book-g", "book-h",
        "book-i", "book-j", "book-k", "book-l", "book-m", "book-n", "book-o", "book-p", "book-q",
        "book-r", "book-s", "book-t", "book-u", "book-v", "book-w", "book-x", "book-y",
    ]
    .map(|key| BookRow::new(key, "20260812", "AA0118").rows(vec![["Alpha Traders", "-1.00"]]));
    let window = window(&rows);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );

    assert_eq!(report.observations().unbalanced_voucher_count, 26);
    assert_eq!(
        report.observations().unbalanced_vouchers,
        (b'a'..=b'y')
            .map(|suffix| format!("book-{}", char::from(suffix)))
            .collect::<Vec<_>>()
    );
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
fn duplicate_observation_keys_are_sorted_before_the_listing_is_capped() {
    let window = window(&[
        BookRow::new("book-10", "20260812", "AA0118"),
        BookRow::new("book-09", "20260812", "AA0118"),
        BookRow::new("book-08", "20260812", "AA0118"),
        BookRow::new("book-07", "20260812", "AA0118"),
        BookRow::new("book-06", "20260812", "AA0118"),
        BookRow::new("book-05", "20260812", "AA0118"),
        BookRow::new("book-04", "20260812", "AA0118"),
        BookRow::new("book-03", "20260812", "AA0118"),
        BookRow::new("book-02", "20260812", "AA0118"),
        BookRow::new("book-01", "20260812", "AA0118"),
        BookRow::new("book-00", "20260812", "AA0118"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let group = &report.observations().duplicate_numbers[0];
    assert_eq!(group.book_voucher_count, MAX_KEYS_PER_DUPLICATE_GROUP + 1);
    assert_eq!(
        group.book_keys,
        (0..MAX_KEYS_PER_DUPLICATE_GROUP)
            .map(|index| format!("book-{index:02}"))
            .collect::<Vec<_>>(),
        "the capped diagnostic is stable even when the transport orders rows differently"
    );
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
fn weighted_party_fanout_is_bounded_below_the_pair_product_limit() {
    let names = (0..25)
        .map(|i| Box::leak(format!("Party Key {i}").into_boxed_str()) as &'static str)
        .collect::<Vec<_>>();
    let rows = (0..999)
        .map(|i| {
            BookRow::new(
                Box::leak(format!("book-{i}").into_boxed_str()),
                "20260812",
                "N",
            )
            .rows(names.iter().map(|name| [*name, "0.00"]).collect())
            .party_field(names[0])
        })
        .collect::<Vec<_>>();
    let observed = window(&rows);
    let proposals = (0..500)
        .map(|i| ProposalRow::new(i, "20260812", "P").party("Party").build())
        .collect::<Vec<_>>();
    assert!(proposals.len() * observed.vouchers().len() < MAX_PRESENCE_COMPARISONS);
    let catalog = catalog_of(&names);
    let parties = bind_parties(&catalog, &proposals).expect("actual party binding");
    assert_eq!(
        parties[0].compare_keys.len(),
        25,
        "fixture must retain every party fanout key"
    );
    let index = WindowIndex::build(&observed);
    let per_proposal =
        resemblance_work_units(&proposals[..1], &parties[..1], &index).expect("unit cost");
    let admitted_count = MAX_PRESENCE_WORK_UNITS / per_proposal;
    assert!(admitted_count > 0 && admitted_count < proposals.len());
    assert!(
        resemblance_work_units(
            &proposals[..admitted_count],
            &parties[..admitted_count],
            &index
        )
        .expect("admitted count")
            <= MAX_PRESENCE_WORK_UNITS
    );
    assert!(
        resemblance_work_units(
            &proposals[..admitted_count + 1],
            &parties[..admitted_count + 1],
            &index
        )
        .expect("refused count")
            > MAX_PRESENCE_WORK_UNITS
    );
    assert!(PresenceRequest::new(
        &observed,
        &catalog,
        &numbering(NumberingMethod::Manual),
        &proposals[..admitted_count]
    )
    .is_ok());
    assert_eq!(
        PresenceRequest::new(
            &observed,
            &catalog,
            &numbering(NumberingMethod::Manual),
            &proposals[..admitted_count + 1]
        )
        .expect_err("real admission refuses"),
        PresenceError::ComparisonWorkTooLarge
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
        marker: ObservedMarker::Absent,
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
        BookVoucher::observed(ObservedVoucher {
            marker: ObservedMarker::Absent,
            key: "  ",
            ..base
        })
        .expect_err("blank"),
        PresenceError::TextBlank
    );
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            marker: ObservedMarker::Absent,
            voucher_type: "Sales\u{0007}",
            ..base
        })
        .expect_err("unsafe"),
        PresenceError::TextUnsafe
    );
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            marker: ObservedMarker::Absent,
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
            marker: ObservedMarker::Absent,
            entries: &bad,
            ..base
        })
        .expect_err("amount"),
        PresenceError::AmountInvalid
    );
}

#[test]
fn a_window_bounds_aggregate_ledger_memberships_before_indexing() {
    let ledgers = (0..MAX_ENTRIES_PER_VOUCHER)
        .map(|position| Box::leak(format!("Ledger {position:04}").into_boxed_str()) as &'static str)
        .collect::<Vec<_>>();
    let entries = ledgers
        .iter()
        .map(|ledger| ObservedEntry {
            ledger,
            amount: "1.00",
        })
        .collect::<Vec<_>>();
    let vouchers = (0..(MAX_WINDOW_LEDGER_MEMBERSHIPS / MAX_ENTRIES_PER_VOUCHER + 1))
        .map(|position| {
            BookVoucher::observed(ObservedVoucher {
                key: Box::leak(format!("book-{position:03}").into_boxed_str()),
                date: "20260812",
                voucher_type: "Sales",
                voucher_number: None,
                remote_id: None,
                party: None,
                marker: ObservedMarker::Absent,
                entries: &entries,
                cancelled: false,
                optional: false,
            })
            .expect("voucher below its own entry limit")
        })
        .collect();
    assert_eq!(
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers,
        })
        .expect_err("derived index membership budget"),
        PresenceError::WindowLedgerMembershipsTooMany
    );
}

#[test]
fn raw_observations_are_bounded_before_voucher_conversion() {
    let entry = ObservedEntry {
        ledger: "Cash",
        amount: "1.00",
    };
    let rows = vec![entry; MAX_WINDOW_RAW_ENTRY_WORK + 1];
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::NotRead,
            vouchers: [ObservedVoucher {
                key: "book-1",
                date: "20260812",
                voucher_type: "Sales",
                voucher_number: None,
                remote_id: None,
                party: None,
                marker: ObservedMarker::Absent,
                entries: &rows,
                cancelled: false,
                optional: false
            }],
        })
        .expect_err("raw entries must be refused before parsing"),
        PresenceError::WindowRawEntryWorkTooLarge
    );
    let long = "x".repeat(MAX_WINDOW_RAW_ENTRY_BYTES + 1);
    let oversized = [ObservedEntry {
        ledger: &long,
        amount: "1.00",
    }];
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::NotRead,
            vouchers: [ObservedVoucher {
                key: "book-2",
                date: "20260812",
                voucher_type: "Sales",
                voucher_number: None,
                remote_id: None,
                party: None,
                marker: ObservedMarker::Absent,
                entries: &oversized,
                cancelled: false,
                optional: false
            }],
        })
        .expect_err("raw bytes must be refused before cloning"),
        PresenceError::WindowRawEntryBytesTooLarge
    );
    let admitted_entries = (0..(MAX_WINDOW_RAW_ENTRY_WORK / MAX_ENTRIES_PER_VOUCHER))
        .map(|_| vec![entry; MAX_ENTRIES_PER_VOUCHER])
        .collect::<Vec<_>>();
    let admitted = admitted_entries
        .iter()
        .enumerate()
        .map(|(position, entries)| ObservedVoucher {
            key: Box::leak(format!("admitted-{position}").into_boxed_str()),
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Absent,
            entries,
            cancelled: false,
            optional: false,
        });
    assert!(BookWindow::from_observations(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::NotRead,
        vouchers: admitted,
    })
    .is_ok());
}

#[test]
fn raw_entry_work_is_bounded_across_valid_voucher_sized_rows() {
    let entry = ObservedEntry {
        ledger: "Cash",
        amount: "1.00",
    };
    let full_voucher_entries = vec![entry; MAX_ENTRIES_PER_VOUCHER];
    let mut rows = (0..(MAX_WINDOW_RAW_ENTRY_WORK / MAX_ENTRIES_PER_VOUCHER))
        .map(|position| ObservedVoucher {
            key: Box::leak(format!("full-{position}").into_boxed_str()),
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Absent,
            entries: &full_voucher_entries,
            cancelled: false,
            optional: false,
        })
        .collect::<Vec<_>>();
    rows.push(ObservedVoucher {
        key: "one-over",
        date: "20260812",
        voucher_type: "Sales",
        voucher_number: None,
        remote_id: None,
        party: None,
        marker: ObservedMarker::Absent,
        entries: std::slice::from_ref(&entry),
        cancelled: false,
        optional: false,
    });
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::NotRead,
            vouchers: rows,
        })
        .expect_err("the aggregate raw-entry limit must span valid rows"),
        PresenceError::WindowRawEntryWorkTooLarge
    );
}

#[test]
fn raw_entry_bytes_admit_exact_limit_and_refuse_the_next_byte() {
    let amount = "1";
    let ledger_1023: &'static str = Box::leak("x".repeat(1_023).into_boxed_str());
    let ledger_1024: &'static str = Box::leak("y".repeat(1_024).into_boxed_str());
    let entry_1024 = ObservedEntry {
        ledger: ledger_1023,
        amount,
    };
    let exact_entries = vec![entry_1024; MAX_WINDOW_RAW_ENTRY_BYTES / 1_024];
    let exact_rows = [
        ObservedVoucher {
            key: "bytes-0",
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Absent,
            entries: &exact_entries[..MAX_ENTRIES_PER_VOUCHER],
            cancelled: false,
            optional: false,
        },
        ObservedVoucher {
            key: "bytes-1",
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Absent,
            entries: &exact_entries[MAX_ENTRIES_PER_VOUCHER..2 * MAX_ENTRIES_PER_VOUCHER],
            cancelled: false,
            optional: false,
        },
        ObservedVoucher {
            key: "bytes-2",
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Absent,
            entries: &exact_entries[2 * MAX_ENTRIES_PER_VOUCHER..],
            cancelled: false,
            optional: false,
        },
    ];
    assert!(BookWindow::from_observations(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::NotRead,
        vouchers: exact_rows,
    })
    .is_ok());
    let extra = ObservedEntry {
        ledger: ledger_1024,
        amount,
    };
    let mut over_tail = exact_entries[2 * MAX_ENTRIES_PER_VOUCHER..].to_vec();
    *over_tail.last_mut().expect("nonempty tail") = extra;
    let over_rows = [
        exact_rows[0],
        exact_rows[1],
        ObservedVoucher {
            entries: &over_tail,
            ..exact_rows[2]
        },
    ];
    assert_eq!(
        over_rows
            .iter()
            .flat_map(|row| row.entries)
            .map(|entry| entry.ledger.len() + entry.amount.len())
            .sum::<usize>(),
        MAX_WINDOW_RAW_ENTRY_BYTES + 1,
    );
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::NotRead,
            vouchers: over_rows,
        })
        .expect_err("one byte over the aggregate raw-byte limit must refuse"),
        PresenceError::WindowRawEntryBytesTooLarge
    );
}

#[test]
fn raw_observation_voucher_count_budget_counts_zero_entry_admissions() {
    let mut budget = RawObservationBudget::default();
    for _ in 0..MAX_WINDOW_VOUCHERS {
        assert!(budget.admit(std::iter::empty()).is_ok());
    }
    assert_eq!(
        budget
            .admit(std::iter::empty())
            .expect_err("next voucher exceeds the count budget"),
        PresenceError::WindowTooLarge
    );
}

#[test]
fn a_window_bounds_aggregate_ledger_key_bytes_before_indexing() {
    let ledgers = (0..(MAX_WINDOW_LEDGER_KEY_BYTES / MAX_TEXT_CHARS + 1))
        .map(|position| {
            Box::leak(format!("{position:04}{}", "x".repeat(MAX_TEXT_CHARS - 4)).into_boxed_str())
                as &'static str
        })
        .collect::<Vec<_>>();
    let entries = ledgers
        .iter()
        .map(|ledger| ObservedEntry {
            ledger,
            amount: "1.00",
        })
        .collect::<Vec<_>>();
    let voucher = BookVoucher::observed(ObservedVoucher {
        key: "book-1",
        date: "20260812",
        voucher_type: "Sales",
        voucher_number: None,
        remote_id: None,
        party: None,
        marker: ObservedMarker::Absent,
        entries: &entries,
        cancelled: false,
        optional: false,
    })
    .expect("voucher below its own bounds");
    assert_eq!(
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: vec![voucher],
        })
        .expect_err("derived index key-byte budget"),
        PresenceError::WindowLedgerKeyBytesTooLarge
    );
}

fn ambiguous_marker_voucher(
    voucher_position: usize,
    marker_count: usize,
    entries: &[ObservedEntry<'static>],
) -> BookVoucher {
    let markers = Box::leak(
        (0..marker_count)
            .map(|marker_position| {
                Box::leak(
                    format!("marker-{voucher_position:04}-{marker_position:02}").into_boxed_str(),
                ) as &'static str
            })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    ) as &'static [&'static str];
    BookVoucher::observed(ObservedVoucher {
        key: Box::leak(format!("book-{voucher_position:04}").into_boxed_str()),
        date: "20260812",
        voucher_type: "Sales",
        voucher_number: None,
        remote_id: None,
        party: None,
        marker: ObservedMarker::Unidentified(markers),
        entries,
        cancelled: false,
        optional: false,
    })
    .expect("voucher below its own ambiguous-marker bound")
}

#[test]
fn an_ambiguous_narration_bounds_raw_occurrences_before_cloning() {
    let entries = [
        ObservedEntry {
            ledger: "Alpha Traders",
            amount: "-1.00",
        },
        ObservedEntry {
            ledger: "Sales Account",
            amount: "1.00",
        },
    ];
    let at_limit = Box::leak(
        (0..MAX_AMBIGUOUS_MARKERS_PER_VOUCHER)
            .map(|_| "marker")
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    ) as &'static [&'static str];
    assert!(
        BookVoucher::observed(ObservedVoucher {
            key: "book-at-limit",
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Unidentified(at_limit),
            entries: &entries,
            cancelled: false,
            optional: false,
        })
        .is_ok(),
        "the declared raw marker limit remains admitted"
    );
    let over_limit = Box::leak(
        (0..=MAX_AMBIGUOUS_MARKERS_PER_VOUCHER)
            .map(|_| "marker")
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    ) as &'static [&'static str];
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            key: "book-over-limit",
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Unidentified(over_limit),
            entries: &entries,
            cancelled: false,
            optional: false,
        })
        .expect_err("raw marker count before a set clone"),
        PresenceError::TooManyAmbiguousMarkers
    );
}

#[test]
fn a_window_bounds_aggregate_ambiguous_marker_memberships_before_indexing() {
    let entries = [
        ObservedEntry {
            ledger: "Alpha Traders",
            amount: "-1.00",
        },
        ObservedEntry {
            ledger: "Sales Account",
            amount: "1.00",
        },
    ];
    let full_vouchers = MAX_WINDOW_AMBIGUOUS_MARKER_MEMBERSHIPS / MAX_AMBIGUOUS_MARKERS_PER_VOUCHER;
    let remainder = MAX_WINDOW_AMBIGUOUS_MARKER_MEMBERSHIPS % MAX_AMBIGUOUS_MARKERS_PER_VOUCHER;
    let mut vouchers = (0..full_vouchers)
        .map(|voucher_position| {
            ambiguous_marker_voucher(
                voucher_position,
                MAX_AMBIGUOUS_MARKERS_PER_VOUCHER,
                &entries,
            )
        })
        .collect::<Vec<_>>();
    if remainder > 0 {
        vouchers.push(ambiguous_marker_voucher(full_vouchers, remainder, &entries));
    }
    assert_eq!(
        vouchers
            .iter()
            .map(|voucher| voucher.ambiguous_markers.len())
            .sum::<usize>(),
        MAX_WINDOW_AMBIGUOUS_MARKER_MEMBERSHIPS,
        "fixture reaches the aggregate membership boundary"
    );
    assert!(BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: vouchers.clone(),
    })
    .is_ok());
    vouchers.push(ambiguous_marker_voucher(full_vouchers + 1, 1, &entries));
    assert_eq!(
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers,
        })
        .expect_err("derived marker membership budget"),
        PresenceError::WindowAmbiguousMarkerMembershipsTooMany
    );
}

#[test]
fn a_window_bounds_aggregate_ambiguous_marker_key_bytes_before_indexing() {
    let entries = [
        ObservedEntry {
            ledger: "Alpha Traders",
            amount: "-1.00",
        },
        ObservedEntry {
            ledger: "Sales Account",
            amount: "1.00",
        },
    ];
    let vouchers: Vec<_> = (0..(MAX_WINDOW_AMBIGUOUS_MARKER_KEY_BYTES / MAX_TEXT_CHARS + 1))
        .map(|position| {
            let marker = Box::leak(
                format!("{position:05}{}", "x".repeat(MAX_TEXT_CHARS - 5)).into_boxed_str(),
            ) as &'static str;
            let markers = Box::leak(vec![marker].into_boxed_slice()) as &'static [&'static str];
            BookVoucher::observed(ObservedVoucher {
                key: Box::leak(format!("book-{position:04}").into_boxed_str()),
                date: "20260812",
                voucher_type: "Sales",
                voucher_number: None,
                remote_id: None,
                party: None,
                marker: ObservedMarker::Unidentified(markers),
                entries: &entries,
                cancelled: false,
                optional: false,
            })
            .expect("voucher below its own marker bounds")
        })
        .collect();
    let admitted = MAX_WINDOW_AMBIGUOUS_MARKER_KEY_BYTES / MAX_TEXT_CHARS;
    assert_eq!(
        admitted * MAX_TEXT_CHARS,
        MAX_WINDOW_AMBIGUOUS_MARKER_KEY_BYTES
    );
    assert!(BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: vouchers[..admitted].to_vec(),
    })
    .is_ok());
    assert_eq!(
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers,
        })
        .expect_err("derived marker key-byte budget"),
        PresenceError::WindowAmbiguousMarkerKeyBytesTooLarge
    );
}

#[test]
fn every_error_carries_a_distinct_stable_reason_code() {
    let codes = [
        PresenceError::WindowIncomplete,
        PresenceError::WindowRangeInvalid,
        PresenceError::WindowTooLarge,
        PresenceError::WindowLedgerMembershipsTooMany,
        PresenceError::WindowLedgerKeyBytesTooLarge,
        PresenceError::WindowAmbiguousMarkerMembershipsTooMany,
        PresenceError::WindowAmbiguousMarkerKeyBytesTooLarge,
        PresenceError::WindowVoucherOutsideRange,
        PresenceError::WindowDuplicateVoucherKey,
        PresenceError::WindowDoesNotCover,
        PresenceError::ProposalsEmpty,
        PresenceError::TooManyProposals,
        PresenceError::ComparisonWorkTooLarge,
        PresenceError::TooManyEntries,
        PresenceError::TooManyAmbiguousMarkers,
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
    assert_eq!(codes.len(), 23);
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
                count_is_lower_bound: false,
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
    // The absent manual number contradicts the remote identity. The second
    // proposal has the sole exact manual number and remains present.
    assert_eq!(report.totals().present, 1);
    assert_eq!(
        reason(&report.vouchers()[0]),
        UndecidedReason::IdentityConflict
    );
    assert_eq!(report.vouchers()[1].present_book_key(), Some("book-1"));
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
fn a_number_match_without_the_proposed_observed_remote_id_does_not_settle() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .remote_id("previous-id")
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
    let unread = BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::NotRead,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: vec![BookRow::new("book-1", "20260819", "AA0130")
            .party("Bravo Industries")
            .build()],
    })
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
    let unread = BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::NotRead,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: vec![BookRow::new("book-1", "20260812", "AA0118").build()],
    })
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
    let unread = BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::NotRead,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: vec![BookRow::new("book-1", "20260812", "AA0118").build()],
    })
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
fn a_remote_id_with_an_absent_manual_number_is_an_identity_conflict() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .remote_id("tally-1")
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
fn a_marker_with_an_absent_manual_number_is_an_identity_conflict() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").marker("marker-1")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .marker("marker-1")
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
    let candidates = &entry.undecided().expect("undecided").candidates;
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].book_key, "book-1");
    assert_eq!(candidates[0].rule, CandidateRule::SharedNarrationMarker);
}

#[test]
fn an_absent_manual_number_conflict_retains_both_strong_identities() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").remote_id("remote-1"),
        BookRow::new("book-2", "20260813", "AA0119").marker("marker-1"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .remote_id("remote-1")
        .marker("marker-1")
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
    let candidates = &entry.undecided().expect("undecided").candidates;
    assert_eq!(candidates.len(), 2);
    assert!(candidates
        .iter()
        .any(|c| c.book_key == "book-1" && c.rule == CandidateRule::SharedRemoteId));
    assert!(candidates
        .iter()
        .any(|c| c.book_key == "book-2" && c.rule == CandidateRule::SharedNarrationMarker));
    assert_eq!(report.observations().unmatched_book_vouchers, 0);
}

#[test]
fn a_remote_id_with_an_absent_manual_number_on_an_unobserved_type_is_a_conflict() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .voucher_type("Other")
        .remote_id("tally-1")
        .build()];
    let numbering = NumberingDeclaration::new([
        ("Sales", NumberingMethod::Manual),
        ("Other", NumberingMethod::Manual),
    ])
    .expect("numbering");
    let report = run(&window, &catalog(), &numbering, &proposals);
    let entry = only(&report);
    assert!(entry.present_book_key().is_none());
    assert_eq!(reason(entry), UndecidedReason::IdentityConflict);
}

#[test]
fn a_nonunique_proposal_number_cannot_contradict_a_unique_remote_id() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").remote_id("tally-1"),
        BookRow::new("book-2", "20260813", "AA0119"),
    ]);
    let proposals = [
        ProposalRow::new(0, "20260813", "AA0119")
            .remote_id("tally-1")
            .build(),
        ProposalRow::new(1, "20260814", "AA0119").build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(report.vouchers()[0].present_book_key(), Some("book-1"));
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
        BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::NotRead,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: carrying,
        })
        .expect_err("contradiction"),
        PresenceError::WindowRemoteIdContradiction
    );
    // The same vouchers are fine once the window admits it read the column.
    assert!(BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: vec![BookRow::new("book-1", "20260812", "AA0118")
            .remote_id("tally-1")
            .build()],
    })
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

/// A voucher number is content, not a name, and the two are folded
/// differently on purpose.
///
/// `comparison_key` lowercases and unifies dash and quote variants because
/// §3.3b measured Tally doing that to master *names*. Nothing measured it for
/// numbers, and the fold fails in the silent direction: it produces more
/// matches, a wrong number match is a `Present`, and `Present` tells a caller
/// an invoice is already filed. Two distinct invoices differing only in case
/// would have suppressed one another.
#[test]
fn two_numbers_differing_only_in_case_are_two_numbers() {
    let cased = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "aa0118")
        .party("Bravo Industries")
        .rows(vec![
            ["Bravo Industries", "-4200.00"],
            ["Sales Account", "4200.00"],
        ])
        .build()];
    let report = run(
        &cased,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        only(&report).present_book_key(),
        None,
        "a case variant is a different number until something measures otherwise"
    );

    // Outer padding is normalized, but internal whitespace is content until
    // an observed source contract proves otherwise. Folding it could turn two
    // distinct invoice numbers into an unsafe `Present` verdict.
    let padded_rows = [BookRow::new("book-1", "20260812", "AA 0118")];
    let padded = window(&padded_rows);
    let spaced = [ProposalRow::new(0, "20260812", "AA  0118").build()];
    let report = run(
        &padded,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &spaced,
    );
    assert_eq!(only(&report).present_book_key(), None);

    let composed = window(&[BookRow::new("book-1", "20260812", "Caf\u{00e9}-0118")]);
    let decomposed = [ProposalRow::new(0, "20260812", "Cafe\u{0301}-0118")
        .party("Bravo Industries")
        .rows(vec![
            ["Bravo Industries", "-4200.00"],
            ["Sales Account", "4200.00"],
        ])
        .build()];
    let report = run(
        &composed,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &decomposed,
    );
    assert_eq!(
        only(&report).present_book_key(),
        None,
        "Unicode composition is part of a voucher number until Tally proves otherwise"
    );
}

#[test]
fn a_manual_number_does_not_decide_across_differently_spelled_voucher_types() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .voucher_type("sales")
        .build()];
    let declaration =
        NumberingDeclaration::new([("sales", NumberingMethod::Manual)]).expect("numbering");
    let report = run(&window, &catalog(), &declaration, &proposals);
    assert!(!only(&report).voucher_type_observed);
    assert_eq!(only(&report).present_book_key(), None);
    assert_eq!(
        reason(only(&report)),
        UndecidedReason::VoucherTypeNotObserved
    );
}

/// Under a `Manual` declaration the number is the one key that can decide, so
/// a proposal supplying none has offered nothing decisive — an absence would
/// rest on date, party and amount, which this contract does not let decide.
#[test]
fn a_manual_type_without_a_number_cannot_be_reported_absent() {
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let mut proposal = ProposalRow::new(0, "20260812", "AA0999");
    proposal.number = None;
    proposal = proposal.party("Charlie Minerals").rows(vec![
        ["Charlie Minerals", "-77.00"],
        ["Sales Account", "77.00"],
    ]);
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &[proposal.build()],
    );
    let entry = only(&report);
    assert!(!entry.is_absent());
    assert_eq!(reason(entry), UndecidedReason::ManualNumberNotSupplied);
}

#[test]
fn an_automatic_type_without_a_number_is_still_answerable() {
    // Under automatic numbering the number was never decisive, so omitting it
    // skips nothing and the absence stands on the rules that could run.
    let window = window(&[BookRow::new("book-1", "20260819", "AA0130").party("Bravo Industries")]);
    let mut proposal = ProposalRow::new(0, "20260812", "AA0999");
    proposal.number = None;
    proposal = proposal.party("Charlie Minerals").rows(vec![
        ["Charlie Minerals", "-77.00"],
        ["Sales Account", "77.00"],
    ]);
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &[proposal.build()],
    );
    assert!(only(&report).is_absent());
}

/// The observations sit outside the paged rows, so a consumer cannot trim
/// them. An unbounded echo there can push a complete report past a byte budget
/// that trimming rows could no longer rescue.
#[test]
fn book_observation_labels_are_bounded() {
    let long: &'static str = Box::leak(
        "N".repeat(MAX_OBSERVATION_LABEL_CHARS + 50)
            .into_boxed_str(),
    );
    let window = window(&[
        BookRow::new("book-1", "20260812", long),
        BookRow::new("book-2", "20260813", long).party("Bravo Industries"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let group = &report.observations().duplicate_numbers[0];
    assert_eq!(
        group.voucher_number.chars().count(),
        MAX_OBSERVATION_LABEL_CHARS + 1,
        "the whole bound of content, plus the marker that says it was applied"
    );
    assert!(group.voucher_number.ends_with(SHORTENED));
    // The group's identity is its keys, which are bounded by count, not by the
    // label that helps a human recognise it.
    assert_eq!(group.book_keys, vec!["book-1", "book-2"]);
    assert_eq!(group.book_voucher_count, 2);
}

/// This contract's two production files are pinned in the compatibility
/// surface, and there is a way for that to stop being true **silently**.
///
/// Resolving a surface conflict by taking the base side — which is the only
/// correct way to resolve a generated artifact — drops the entries a branch
/// *adds*, because `rehash-surface` updates hashes and never adds paths. The
/// compatibility gate does not catch it: its bound is
/// `MAX_SURFACE_FILES - files.len() <= RESERVED_SURFACE_FILES`, which asserts
/// there is no unreviewed *headroom* rather than that the cap matches the
/// surface. A guard on the slack cannot catch a claim made too early, or a pin
/// quietly lost.
///
/// So the claim is asserted here instead, in a file that is not itself pinned.
/// If a rebase ever drops these two, this fails loudly rather than the seal
/// passing over a surface that no longer covers the engine it was raised for.
#[test]
fn this_contracts_files_are_still_pinned_in_the_compatibility_surface() {
    const SURFACE: &str =
        include_str!("../../../../docs/tally/compatibility/compatibility-surface.json");
    let surface: serde_json::Value = serde_json::from_str(SURFACE).expect("surface json");
    let pinned = surface["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect::<BTreeSet<_>>();
    for path in [
        "src-tauri/crates/bridge-tally-core/src/book_presence.rs",
        "src-tauri/src/agent_presence.rs",
        // The adapter reads its bounds from the published schema rather than
        // restating them, so the only independent statement of the admission
        // contract is the assertion in this file. Unpinned, a loosened schema
        // and its matching test update leave the digest untouched.
        "src-tauri/src/agent_presence_tests.rs",
        // A narration marker is whatever this derives (ADR 0018). Presence
        // calls the same function the import writer calls so that a reader and
        // a writer cannot disagree about one voucher's identity -- which makes
        // an edit confined to it a silent change to what is reported present.
        "src-tauri/src/agent_import_identity.rs",
    ] {
        assert!(
            pinned.contains(path),
            "{path} is no longer pinned: a conflict resolution dropped it and the gate cannot see that"
        );
    }
}

/// `unmatched_book_vouchers` promises to count rows no proposal matched **or
/// even resembled**. A collision returns before rule three, so without help it
/// would report a row this proposal plainly resembles as one nothing came
/// near — the diagnostic contradicting itself.
#[test]
fn a_collision_still_counts_what_the_proposal_resembled() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let money = vec![
        ["Alpha Traders", "-11800.00"],
        ["Sales Account", "10000.00"],
        ["Output CGST 9%", "900.00"],
        ["Output SGST 9%", "900.00"],
    ];
    // Two proposals share a REMOTEID the book does not carry, so the collision
    // decides — but both plainly resemble book-1 on date, party and amount.
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0901")
            .remote_id("tally-9")
            .rows(money.clone())
            .build(),
        ProposalRow::new(1, "20260812", "AA0902")
            .remote_id("tally-9")
            .rows(money)
            .build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    for entry in report.vouchers() {
        assert_eq!(reason(entry), UndecidedReason::RemoteIdCollision);
    }
    assert_eq!(
        report.observations().unmatched_book_vouchers,
        0,
        "book-1 was resembled by both proposals, whatever decided them"
    );
}

/// A key is an identity, so a pathological one is refused rather than cut —
/// half a key joins to nothing. It is also echoed in every candidate, and a
/// consumer's framing can drop whole rows but cannot shrink one.
#[test]
fn a_pathological_book_key_is_refused_rather_than_truncated() {
    let rows = [["Alpha Traders", "-1.00"], ["Sales Account", "1.00"]];
    let entries = entries(&rows);
    let long: String = "g".repeat(MAX_BOOK_KEY_CHARS + 1);
    assert_eq!(
        BookVoucher::observed(ObservedVoucher {
            marker: ObservedMarker::Absent,
            key: &long,
            date: "20260812",
            voucher_type: "Sales",
            voucher_number: Some("AA0118"),
            remote_id: None,
            party: Some("Alpha Traders"),
            entries: &entries,
            cancelled: false,
            optional: false,
        })
        .expect_err("pathological key"),
        PresenceError::VoucherKeyTooLong
    );
    // A real Tally GUID — company prefix plus master id — is far inside it.
    assert!(BookVoucher::observed(ObservedVoucher {
        marker: ObservedMarker::Absent,
        key: "61c6de69-1748-461c-ad3f-162cb949df9f-00000001",
        date: "20260812",
        voucher_type: "Sales",
        voucher_number: Some("AA0118"),
        remote_id: None,
        party: Some("Alpha Traders"),
        entries: &entries,
        cancelled: false,
        optional: false,
    })
    .is_ok());
}

/// Candidate order is part of the contract, so the same book must yield the
/// same twenty-five whatever order Tally happened to return its rows in. The
/// cap is applied after ranking, never to an arbitrary source prefix.
#[test]
fn a_capped_collision_list_does_not_depend_on_the_rows_arriving_order() {
    let keys: Vec<&'static str> = (1..=40)
        .map(|index| Box::leak(format!("book-{index:03}").into_boxed_str()) as &'static str)
        .collect();
    let listed = |order: Vec<&'static str>| {
        let rows: Vec<BookRow> = order
            .into_iter()
            .enumerate()
            .map(|(offset, key)| {
                BookRow::new(
                    key,
                    if offset % 2 == 0 {
                        "20260812"
                    } else {
                        "20260813"
                    },
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
        let undecided = only(&report).undecided().expect("undecided").clone();
        assert_eq!(undecided.reason, UndecidedReason::BookNumberCollision);
        assert_eq!(undecided.candidate_count, 40);
        undecided
            .candidates
            .iter()
            .map(|candidate| candidate.book_key.clone())
            .collect::<Vec<_>>()
    };
    let ascending = listed(keys.clone());
    let reversed = listed(keys.into_iter().rev().collect());
    assert_eq!(ascending.len(), MAX_CANDIDATES_PER_PROPOSAL);
    assert_eq!(
        ascending, reversed,
        "the same book must expose the same candidates whatever order its rows arrive in"
    );
    // And the retained slice is the ordered prefix, not an arbitrary one.
    let mut sorted = ascending.clone();
    sorted.sort();
    assert_eq!(ascending, sorted);
}

/// A proposal that settles by identity still *reached* whatever else it
/// resembles. `unmatched_book_vouchers` counts only what no proposal came
/// near, so a resembled row must not appear there because another row
/// happened to carry the identity.
#[test]
fn an_identity_match_still_counts_what_it_resembled() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        // Same date, party and amount, different number: resembled, not matched.
        BookRow::new("book-2", "20260812", "AA0777"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(only(&report).present_book_key(), Some("book-1"));
    assert_eq!(
        report.observations().unmatched_book_vouchers,
        0,
        "book-2 was resembled even though book-1 carried the identity"
    );
}

/// The echoed party names are diagnostics a person reads, and a response can
/// drop whole rows but cannot shrink one. The comparison that produced the
/// difference used the full values; only the echo is bounded.
#[test]
fn an_echoed_party_difference_is_bounded() {
    let long: &'static str = Box::leak(
        format!("Bravo {}", "o".repeat(MAX_OBSERVATION_LABEL_CHARS + 40)).into_boxed_str(),
    );
    let names = [
        "Alpha Traders",
        long,
        "Sales Account",
        "Output CGST 9%",
        "Output SGST 9%",
    ];
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").party_field(long)]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog_of(&names),
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
    assert_eq!(
        party.observed.as_deref().map(|value| value.chars().count()),
        // The bound, plus the one character that says it was applied.
        Some(MAX_OBSERVATION_LABEL_CHARS + 1)
    );
}

/// The marker must not cost a character of content.
///
/// Spending one to stay inside the bound would make two values differing at
/// exactly the bound serialize identically — converting a difference that was
/// visible before the marker existed into one that is not. That is the failure
/// the marker exists to prevent, reintroduced one position earlier, and it
/// would be quieter than the bug it replaced: the report would still say the
/// two differ, and now also say it had shortened them, while showing one
/// string. Both are true statements and the reader still cannot see it.
#[test]
fn the_shortening_marker_does_not_cost_a_character_of_content() {
    let shared = "o".repeat(MAX_OBSERVATION_LABEL_CHARS - 1);
    // Identical for the whole bound but the final character inside it.
    let proposed: &'static str = Box::leak(format!("{shared}A tail").into_boxed_str());
    let observed: &'static str = Box::leak(format!("{shared}B tail").into_boxed_str());
    let names = [
        "Alpha Traders",
        proposed,
        "Sales Account",
        "Output CGST 9%",
        "Output SGST 9%",
    ];
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").party_field(observed)]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .party(proposed)
        .build()];
    let report = run(
        &window,
        &catalog_of(&names),
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
    assert_ne!(
        party.proposed, party.observed,
        "a difference inside the bound must still be visible in the echo"
    );
    for shown in [&party.proposed, &party.observed] {
        assert!(shown
            .as_deref()
            .is_some_and(|value| value.ends_with(SHORTENED)));
    }
}

/// Bounding must not quietly turn a true difference into a false display.
///
/// Two accepted names can agree for the whole bounded prefix and differ after
/// it -- the adapter admits names eight times longer than this bound. The
/// comparison sees the difference, so a difference is reported; without a
/// marker both sides then serialize to the same string and the report asserts
/// that two identical values differ. The marker cannot recover the missing
/// tail, but it stops the report from lying about what it is showing.
#[test]
fn a_difference_bounded_on_both_sides_says_the_values_were_shortened() {
    let shared = "Bravo ".to_string() + &"o".repeat(MAX_OBSERVATION_LABEL_CHARS);
    let proposed: &'static str = Box::leak(format!("{shared} Northern Division").into_boxed_str());
    let observed: &'static str = Box::leak(format!("{shared} Southern Division").into_boxed_str());
    let names = [
        "Alpha Traders",
        proposed,
        "Sales Account",
        "Output CGST 9%",
        "Output SGST 9%",
    ];
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").party_field(observed)]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .party(proposed)
        .build()];
    let report = run(
        &window,
        &catalog_of(&names),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present { differences, .. } = &only(&report).status else {
        panic!("expected Present");
    };
    let party = differences
        .iter()
        .find(|difference| difference.field == DifferenceField::Party)
        .expect("the full values differ, so a difference is reported");
    let shown_proposed = party.proposed.as_deref().expect("proposed");
    let shown_observed = party.observed.as_deref().expect("observed");
    // The premise: bounding really does collapse these two onto one string.
    assert_eq!(
        shown_proposed, shown_observed,
        "the values agree across the whole bounded prefix"
    );
    for shown in [shown_proposed, shown_observed] {
        assert!(
            shown.ends_with('\u{2026}'),
            "a shortened value must say it was shortened"
        );
        assert_eq!(
            shown.chars().count(),
            MAX_OBSERVATION_LABEL_CHARS + 1,
            "the whole bound of content, plus the marker"
        );
    }
}

/// A collision returns before resemblance can decide anything, but the
/// proposal still *reached* what it resembles. Every earlier collision test
/// had the whole window sharing the number, so the colliding set and the
/// resembled set were the same rows and a bare set looked correct.
#[test]
fn a_number_collision_still_reaches_what_it_only_resembled() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118"),
        // Shares the number: a collision, and the reason this returns early.
        BookRow::new("book-2", "20260812", "AA0118"),
        // Shares date, party and amount but not the number: resembled only,
        // and reachable solely through the resemblance scan the early return
        // used to skip.
        BookRow::new("book-3", "20260812", "AA0777"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(reason(only(&report)), UndecidedReason::BookNumberCollision);
    assert_eq!(
        report.observations().unmatched_book_vouchers,
        0,
        "book-3 was plainly resembled; the collision must not hide that"
    );
}

// ---------------------------------------------------------------------------
// ADR 0018 — the narration marker as an identity basis.
//
// Markers here are canonical-looking opaque strings. This crate never parses
// one: the `[BRIDGE:...]` convention is applied above it, which is the whole
// reason the crate can treat them as hashable keys.
// ---------------------------------------------------------------------------

const MARKER_A: &str = "8f14e45f-ceea-467a-9c1b-7b2f4c8a0001";
const MARKER_B: &str = "8f14e45f-ceea-467a-9c1b-7b2f4c8a0002";

/// A window whose read did not fetch `NARRATION` at all.
fn window_without_narration(rows: &[BookRow]) -> BookWindow {
    BookWindow::observed(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::Observed,
        narration_evidence: ColumnEvidence::NotRead,
        vouchers: rows.iter().map(BookRow::build).collect(),
    })
    .expect("window")
}

/// The case the basis exists for: Bridge wrote this voucher, and says so.
#[test]
fn a_marker_bridge_wrote_settles_a_present() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A)]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0999")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(entry.present_book_key(), Some("book-1"));
    let PresenceStatus::Present { basis, .. } = &entry.status else {
        panic!("expected Present");
    };
    assert_eq!(*basis, PresenceBasis::NarrationMarker);
    assert!(report.observations().narration_marker_observed);
}

/// Under `Automatic` numbering Tally discards the supplied number, so the
/// manual-number basis does not exist and `REMOTEID` is not fetched by the
/// shipped read. Without the marker this proposal has no identity at all --
/// which is exactly the gap ADR 0018 was written to close.
#[test]
fn a_marker_decides_where_automatic_numbering_leaves_nothing_else() {
    let rows = [BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A)];
    let unmarked = [ProposalRow::new(0, "20260812", "AA0118").build()];
    let without = run(
        &window(&rows),
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &unmarked,
    );
    assert_eq!(
        reason(only(&without)),
        UndecidedReason::NumberNotDecisive,
        "the number matches the book row exactly and still decides nothing"
    );

    let marked = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
    let with = run(
        &window(&rows),
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &marked,
    );
    assert_eq!(only(&with).present_book_key(), Some("book-1"));
}

/// Bridge writes a distinct identity per imported voucher, so one marker on two
/// book rows is a book anomaly. It is still never resolved by picking one.
#[test]
fn one_marker_on_two_book_vouchers_decides_nothing() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A),
        BookRow::new("book-2", "20260813", "AA0119").marker(MARKER_A),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(reason(entry), UndecidedReason::NarrationMarkerCollision);
    assert_eq!(entry.undecided().expect("undecided").candidate_count, 2);
}

/// A marker that would otherwise identify one voucher uniquely is not unique
/// when a *second* book voucher also carries it, even if that second voucher
/// only carries it ambiguously. Counting solely `by_marker` let the ambiguous
/// occurrence hide: the identifying voucher looked like the marker's only
/// home, and `Present` went out for it while the marker actually named two
/// book vouchers -- the exact middle case ambiguous-marker handling exists to
/// preserve.
#[test]
fn a_marker_shared_with_an_ambiguous_voucher_decides_nothing() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A),
        BookRow::new("book-2", "20260813", "AA0119").ambiguous_markers(&[MARKER_A, MARKER_B]),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(reason(entry), UndecidedReason::NarrationMarkerCollision);
    assert_eq!(
        entry.undecided().expect("undecided").candidate_count,
        2,
        "the marker occurs on two book vouchers, so it is not unique"
    );
}

/// An occurrence carried by an ambiguous narration is not an identity by
/// itself, but it still contradicts a manual number that selected another row.
/// Both rows must remain candidates and reached evidence for the operator.
#[test]
fn an_ambiguous_marker_on_another_voucher_blocks_a_manual_number_settlement() {
    let window = window(&[
        BookRow::new("book-a", "20260812", "AA0118"),
        BookRow::new("book-b", "20260813", "BB0229").ambiguous_markers(&[MARKER_A, MARKER_B]),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
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
    let undecided = entry.undecided().expect("identity conflict");
    assert_eq!(undecided.candidate_count, 2);
    assert_eq!(
        undecided
            .candidates
            .iter()
            .map(|candidate| candidate.book_key.as_str())
            .collect::<Vec<_>>(),
        vec!["book-b", "book-a"],
        "marker and number evidence both remain visible"
    );
    assert_eq!(
        undecided.candidates[0].rule,
        CandidateRule::SharedNarrationMarker
    );
    assert_eq!(
        undecided.candidates[1].rule,
        CandidateRule::SharedVoucherNumber
    );
    assert_eq!(report.observations().unmatched_book_vouchers, 0);
}

/// Proposal-side uniqueness is checked before the book lookup, the same way it
/// is for a `REMOTEID`: two source rows claiming one identity are undecidable
/// whether or not the book holds it.
#[test]
fn two_proposals_claiming_one_marker_decide_nothing() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A)]);
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0118")
            .marker(MARKER_A)
            .build(),
        ProposalRow::new(1, "20260813", "AA0119")
            .marker(MARKER_A)
            .build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    for entry in report.vouchers() {
        assert_eq!(reason(entry), UndecidedReason::NarrationMarkerCollision);
    }
}

#[test]
fn a_remote_id_collision_retains_each_narration_marker_match() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A),
        BookRow::new("book-2", "20260813", "AA0119").marker(MARKER_B),
    ]);
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0118")
            .remote_id("shared-source-id")
            .marker(MARKER_A)
            .build(),
        ProposalRow::new(1, "20260813", "AA0119")
            .remote_id("shared-source-id")
            .marker(MARKER_B)
            .build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Automatic),
        &proposals,
    );
    for (entry, expected_key) in report.vouchers().iter().zip(["book-1", "book-2"]) {
        assert_eq!(reason(entry), UndecidedReason::RemoteIdCollision);
        let undecided = entry.undecided().expect("collision");
        assert_eq!(undecided.candidate_count, 1);
        assert_eq!(undecided.candidates[0].book_key, expected_key);
        assert_eq!(
            undecided.candidates[0].rule,
            CandidateRule::SharedNarrationMarker
        );
    }
    assert_eq!(report.observations().unmatched_book_vouchers, 0);
}

/// Three identity signals mean three ways to disagree. A marker selecting one
/// voucher while the manual number selects another is reported, never ranked.
#[test]
fn a_marker_and_a_number_selecting_different_vouchers_conflict() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A),
        BookRow::new("book-2", "20260813", "AA0777"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0777")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(reason(entry), UndecidedReason::IdentityConflict);
    let keys = entry
        .undecided()
        .expect("undecided")
        .candidates
        .iter()
        .map(|candidate| candidate.book_key.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        keys.contains("book-1") && keys.contains("book-2"),
        "both sides of the disagreement are shown, not just the stronger one"
    );
}

/// `REMOTEID` and the marker can both select the *same* book voucher while the
/// manual number selects a different one -- three selections naming only two
/// book vouchers. Mapping every selection straight into a candidate lists the
/// shared voucher twice and reports one candidate more than there are book
/// vouchers to look at; the response must collapse to book position first.
#[test]
fn a_conflict_naming_one_voucher_twice_reports_it_once() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118")
            .remote_id("bridge-txn-1")
            .marker(MARKER_A),
        BookRow::new("book-2", "20260813", "AA0777"),
    ]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0777")
        .remote_id("bridge-txn-1")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_eq!(reason(entry), UndecidedReason::IdentityConflict);
    let undecided = entry.undecided().expect("undecided");
    assert_eq!(
        undecided.candidate_count, 2,
        "book-1 is named by two rules but is one book voucher"
    );
    let keys = undecided
        .candidates
        .iter()
        .map(|candidate| candidate.book_key.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec!["book-1", "book-2"],
        "each book voucher is listed exactly once"
    );
}

/// The other way two identities disagree: they agree on one voucher, and that
/// voucher names a different marker than the proposal does.
#[test]
fn a_number_match_naming_another_marker_is_a_conflict() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_B)]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(reason(only(&report)), UndecidedReason::IdentityConflict);
}

/// The same rule as `RemoteIdEvidenceUnavailable`, other column. A number that
/// is decisive on its own terms still cannot settle while the evidence that
/// could have contradicted it was never gathered.
#[test]
fn a_marker_against_an_unread_narration_withholds_both_verdicts() {
    let window = window_without_narration(&[BookRow::new("book-1", "20260812", "AA0118")]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        reason(only(&report)),
        UndecidedReason::MarkerEvidenceUnavailable
    );

    // And an absence is withheld too, on a window holding nothing like it.
    let empty = window_without_narration(&[]);
    let away = [ProposalRow::new(0, "20260820", "ZZ9999")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &empty,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &away,
    );
    assert_eq!(
        reason(only(&report)),
        UndecidedReason::MarkerEvidenceUnavailable
    );
}

/// A window cannot both say narration was never read and carry something read
/// out of a narration. The two statements contradict.
#[test]
fn a_window_that_did_not_read_narration_cannot_carry_a_marker() {
    for row in [
        BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A),
        BookRow::new("book-1", "20260812", "AA0118").unidentified_marker(),
    ] {
        let error = BookWindow::observed(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::NotRead,
            vouchers: vec![row.build()],
        })
        .expect_err("contradiction");
        assert_eq!(error, PresenceError::WindowNarrationContradiction);
    }
}

/// A legacy-scheme marker, two markers, or a malformed one cannot name a single
/// import, so it never matches. Losing the fact that Bridge wrote the row would
/// be its own defect, so it is counted for a person instead.
#[test]
fn an_unidentifiable_marker_is_counted_and_never_matched() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").unidentified_marker(),
        BookRow::new("book-2", "20260820", "AA0119").unidentified_marker(),
    ]);
    let proposals = [ProposalRow::new(0, "20260805", "AA0777")
        .marker(MARKER_A)
        .party("Bravo Industries")
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    assert_eq!(
        only(&report).status,
        PresenceStatus::Absent,
        "an unidentifiable marker matches nothing, and the window was read"
    );
    let observations = report.observations();
    assert_eq!(observations.unidentified_bridge_writes, 2);
    assert!(
        !observations.narration_marker_observed,
        "nothing identifying was observed, which is a different fact"
    );
}

/// A marker landing on a cancelled voucher is the same case as any other
/// identity landing on one: it occupies the row without being posted.
#[test]
fn a_marker_on_a_cancelled_voucher_is_not_present() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")
        .marker(MARKER_A)
        .cancelled()]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
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

/// One book voucher satisfies at most one proposal *across* bases. Adding a
/// third basis adds a third way for two proposals to reach one row.
#[test]
fn a_marker_and_a_number_cannot_claim_one_voucher_for_two_proposals() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118").marker(MARKER_A)]);
    let mut marker_only = ProposalRow::new(1, "20260813", "AA0119").marker(MARKER_A);
    // Omitted number evidence permits another identity to settle. A supplied
    // absent manual number would instead conflict before claiming this row.
    marker_only.number = None;
    let proposals = [
        ProposalRow::new(0, "20260812", "AA0118").build(),
        marker_only.build(),
    ];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    // A demotion is not a licence to misdescribe what matched: the proposal
    // that reached this voucher by its marker must still say so, and the one
    // that reached it by its number must say that.
    let rules = report
        .vouchers()
        .iter()
        .map(|entry| {
            assert_eq!(reason(entry), UndecidedReason::BookVoucherClaimedTwice);
            entry.undecided().expect("undecided").candidates[0].rule
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rules,
        vec![
            CandidateRule::SharedVoucherNumber,
            CandidateRule::SharedNarrationMarker
        ]
    );
}

/// A voucher whose narration carries this proposal's marker *and* another one
/// cannot be an identity -- it claims two imports, which this contract never
/// resolves. But the marker was observed, so `Absent` is not available either:
/// saying it invites the duplicate the whole contract exists to prevent.
///
/// The proposal here shares nothing else with the book row -- different date,
/// different number, different party, different amount -- so the marker is the
/// only thing that can surface it, and before this it surfaced nothing.
#[test]
fn an_ambiguous_narration_still_withholds_the_absence() {
    let window = window(&[
        BookRow::new("book-1", "20260812", "AA0118").ambiguous_markers(&[MARKER_A, MARKER_B])
    ]);
    let proposals = [ProposalRow::new(0, "20260820", "ZZ9999")
        .marker(MARKER_A)
        .party("Charlie Minerals")
        .rows(vec![
            ["Charlie Minerals", "-4200.00"],
            ["Sales Account", "4200.00"],
        ])
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let entry = only(&report);
    assert_ne!(
        entry.status,
        PresenceStatus::Absent,
        "the marker was observed in this book, so an absence is not available"
    );
    // And it is a candidate, named by the rule that found it -- never a
    // `Present`, because the voucher claims two imports.
    let undecided = entry.undecided().expect("undecided");
    assert_eq!(undecided.candidates[0].book_key, "book-1");
    assert_eq!(
        undecided.candidates[0].rule,
        CandidateRule::SharedNarrationMarker
    );
    assert_eq!(report.observations().unidentified_bridge_writes, 1);
}

/// A `Present` on the marker reports its differences like any other basis --
/// the ₹36.13 case reached through the channel Bridge actually has.
#[test]
fn a_marker_present_still_reports_what_disagrees() {
    let window = window(&[BookRow::new("book-1", "20260812", "AA0118")
        .marker(MARKER_A)
        .rows(vec![
            ["Alpha Traders", "-10900.00"],
            ["Sales Account", "10000.00"],
            ["Output CGST 9%", "900.00"],
        ])]);
    let proposals = [ProposalRow::new(0, "20260812", "AA0118")
        .marker(MARKER_A)
        .build()];
    let report = run(
        &window,
        &catalog(),
        &numbering(NumberingMethod::Manual),
        &proposals,
    );
    let PresenceStatus::Present {
        differences, basis, ..
    } = &only(&report).status
    else {
        panic!("expected Present");
    };
    assert_eq!(*basis, PresenceBasis::NarrationMarker);
    assert!(differences
        .iter()
        .any(|difference| difference.field == DifferenceField::Amount));
}

#[test]
fn raw_proposal_budget_counts_all_entry_work_before_conversion() {
    let entries = vec![
        ObservedEntry {
            ledger: "L",
            amount: "1"
        };
        2_000
    ];
    let inputs = (0..50)
        .map(|position| ProposedVoucherInput {
            position,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: Some("1"),
            remote_id: None,
            narration_marker: None,
            party: None,
            entries: &entries,
        })
        .collect::<Vec<_>>();
    let admitted =
        ProposedVoucher::from_inputs(inputs.iter().copied()).expect("exact 100,000 entries");
    assert_eq!(admitted.as_slice().len(), 50);
    let extra = [ObservedEntry {
        ledger: "L",
        amount: "not-an-amount",
    }];
    let next = ProposedVoucherInput {
        position: 50,
        entries: &extra,
        ..inputs[0]
    };
    assert_eq!(
        inputs.iter().map(|v| v.entries.len()).sum::<usize>() + next.entries.len(),
        MAX_PROPOSAL_RAW_ENTRY_WORK + 1
    );
    assert_eq!(
        ProposedVoucher::from_inputs(inputs.into_iter().chain([next])),
        Err(PresenceError::ProposalRawEntryWorkTooLarge)
    );
}

#[test]
fn raw_proposal_budget_counts_metadata_bytes_before_conversion() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let marker = "m".repeat(36);
    let metadata =
        "x".repeat(16_384 - "20260812".len() - "Receipt".len() - 1 - marker.len());
    let extra_byte = format!("{metadata}x");
    assert!(extra_byte.len() <= MAX_TEXT_CHARS);
    let inputs = (0..256)
        .map(|position| ProposedVoucherInput {
            position,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: Some("1"),
            remote_id: None,
            narration_marker: Some(marker.as_str()),
            party: Some(metadata.as_str()),
            entries: &rows,
        })
        .collect::<Vec<_>>();
    let total = inputs
        .iter()
        .map(|v| {
            v.date.len()
                + v.voucher_type.len()
                + v.voucher_number.unwrap().len()
                + v.narration_marker.unwrap().len()
                + v.party.unwrap().len()
        })
        .sum::<usize>();
    assert_eq!(total, MAX_PROPOSAL_RAW_BYTES);
    assert_eq!(
        ProposedVoucher::from_inputs(inputs.iter().copied())
            .expect("exact metadata limit")
            .as_slice()
            .len(),
        256
    );
    let mut over = inputs;
    over[255].party = Some(&extra_byte);
    assert_eq!(
        total + extra_byte.len() - metadata.len(),
        MAX_PROPOSAL_RAW_BYTES + 1
    );
    assert_eq!(
        ProposedVoucher::from_inputs(over),
        Err(PresenceError::ProposalRawBytesTooLarge)
    );
}

#[test]
fn raw_observation_budget_counts_retained_voucher_metadata() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let keys = (0..256)
        .map(|position| format!("K{position:07}"))
        .collect::<Vec<_>>();
    let marker = "m".repeat(36);
    let metadata =
        "x".repeat(16_384 - 8 - "20260812".len() - "Receipt".len() - marker.len());
    let extra_byte = format!("{metadata}x");
    assert!(extra_byte.len() <= MAX_TEXT_CHARS);
    let inputs = keys
        .iter()
        .map(|key| ObservedVoucher {
            key,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: None,
            remote_id: Some(metadata.as_str()),
            party: None,
            marker: ObservedMarker::Identifying(marker.as_str()),
            entries: &rows,
            cancelled: false,
            optional: false,
        })
        .collect::<Vec<_>>();
    let total = inputs
        .iter()
        .map(|v| {
            v.key.len()
                + v.date.len()
                + v.voucher_type.len()
                + v.remote_id.unwrap().len()
                + marker.len()
        })
        .sum::<usize>();
    assert_eq!(total, MAX_WINDOW_RAW_ENTRY_BYTES);
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: inputs.iter().copied(),
        })
        .expect("exact metadata limit")
        .vouchers()
        .len(),
        256
    );
    let mut over = inputs;
    over[255].remote_id = Some(&extra_byte);
    assert_eq!(
        total + extra_byte.len() - metadata.len(),
        MAX_WINDOW_RAW_ENTRY_BYTES + 1
    );
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: over,
        }),
        Err(PresenceError::WindowRawEntryBytesTooLarge)
    );
}

#[test]
fn raw_observation_budget_counts_ambiguous_marker_bytes_before_conversion() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let marker = "m".repeat(36);
    let next_marker = format!("{marker}x");
    let ambiguous = [marker.as_str()];
    let next_ambiguous = [next_marker.as_str()];
    let keys = (0..256)
        .map(|position| format!("K{position:07}"))
        .collect::<Vec<_>>();
    let metadata =
        "x".repeat(16_384 - 8 - "20260812".len() - "Receipt".len() - marker.len());
    let inputs = keys
        .iter()
        .map(|key| ObservedVoucher {
            key,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: None,
            remote_id: Some(metadata.as_str()),
            party: None,
            marker: ObservedMarker::Unidentified(&ambiguous),
            entries: &rows,
            cancelled: false,
            optional: false,
        })
        .collect::<Vec<_>>();
    let total = inputs
        .iter()
        .map(|voucher| {
            voucher.key.len()
                + voucher.date.len()
                + voucher.voucher_type.len()
                + voucher.remote_id.unwrap().len()
                + marker.len()
        })
        .sum::<usize>();
    assert_eq!(total, MAX_WINDOW_RAW_ENTRY_BYTES);
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: inputs.iter().copied(),
        })
        .expect("exact metadata limit")
        .vouchers()
        .len(),
        256
    );
    let mut over = inputs;
    over[255].marker = ObservedMarker::Unidentified(&next_ambiguous);
    assert_eq!(total + 1, MAX_WINDOW_RAW_ENTRY_BYTES + 1);
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::Observed,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: over,
        }),
        Err(PresenceError::WindowRawEntryBytesTooLarge)
    );
}

#[test]
fn raw_observation_budget_bounds_ambiguous_marker_occurrences_before_conversion() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let marker = "m";
    let at_limit = vec![marker; MAX_AMBIGUOUS_MARKERS_PER_VOUCHER];
    let over_limit = vec![marker; MAX_AMBIGUOUS_MARKERS_PER_VOUCHER + 1];
    let exact = ObservedVoucher {
        key: "exact",
        date: "20260812",
        voucher_type: "Receipt",
        voucher_number: None,
        remote_id: None,
        party: None,
        marker: ObservedMarker::Unidentified(&at_limit),
        entries: &rows,
        cancelled: false,
        optional: false,
    };
    assert!(BookWindow::from_observations(ObservedWindow {
        from: "20260801",
        to: "20260831",
        read: WindowRead::Complete,
        remote_id_evidence: ColumnEvidence::NotRead,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: [exact],
    })
    .is_ok());
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::NotRead,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: [ObservedVoucher {
                marker: ObservedMarker::Unidentified(&over_limit),
                ..exact
            }],
        }),
        Err(PresenceError::TooManyAmbiguousMarkers)
    );
}

#[test]
fn raw_observation_budget_bounds_aggregate_ambiguous_marker_work_before_conversion() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let markers = vec!["m"; MAX_AMBIGUOUS_MARKERS_PER_VOUCHER];
    let full_rows = MAX_WINDOW_AMBIGUOUS_MARKER_MEMBERSHIPS / markers.len();
    let remainder = MAX_WINDOW_AMBIGUOUS_MARKER_MEMBERSHIPS % markers.len();
    let final_markers = vec!["m"; remainder];
    let keys = (0..=full_rows + 1)
        .map(|position| format!("K{position:07}"))
        .collect::<Vec<_>>();
    let mut observations = keys[..full_rows]
        .iter()
        .map(|key| ObservedVoucher {
            key,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: None,
            remote_id: None,
            party: None,
            marker: ObservedMarker::Unidentified(&markers),
            entries: &rows,
            cancelled: false,
            optional: false,
        })
        .collect::<Vec<_>>();
    observations.push(ObservedVoucher {
        key: &keys[full_rows],
        date: "20260812",
        voucher_type: "Receipt",
        voucher_number: None,
        remote_id: None,
        party: None,
        marker: ObservedMarker::Unidentified(&final_markers),
        entries: &rows,
        cancelled: false,
        optional: false,
    });
    observations.push(ObservedVoucher {
        key: &keys[full_rows + 1],
        ..observations[0]
    });
    assert_eq!(
        full_rows * markers.len() + final_markers.len(),
        MAX_WINDOW_AMBIGUOUS_MARKER_MEMBERSHIPS
    );
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::NotRead,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: observations[..=full_rows].iter().copied(),
        })
        .expect("exact raw ambiguous marker work")
        .vouchers()
        .len(),
        full_rows + 1
    );
    assert_eq!(
        BookWindow::from_observations(ObservedWindow {
            from: "20260801",
            to: "20260831",
            read: WindowRead::Complete,
            remote_id_evidence: ColumnEvidence::NotRead,
            narration_evidence: ColumnEvidence::Observed,
            vouchers: observations,
        }),
        Err(PresenceError::WindowAmbiguousMarkerMembershipsTooMany)
    );
}

#[test]
fn raw_proposal_batch_stops_an_unbounded_iterator_at_the_count_limit() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let seen = std::cell::Cell::new(0);
    let inputs = std::iter::from_fn(|| {
        let position = seen.get();
        assert!(
            position <= MAX_PROPOSED_VOUCHERS,
            "must stop after the first excess input"
        );
        seen.set(position + 1);
        Some(ProposedVoucherInput {
            position,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: None,
            remote_id: None,
            narration_marker: None,
            party: None,
            entries: &rows,
        })
    });
    assert_eq!(
        ProposedVoucher::from_inputs(inputs),
        Err(PresenceError::TooManyProposals)
    );
    assert_eq!(seen.get(), MAX_PROPOSED_VOUCHERS + 1);
}

#[test]
fn proposal_batch_rejects_duplicate_source_positions_before_conversion() {
    let rows = [ObservedEntry {
        ledger: "L",
        amount: "1",
    }];
    let inputs = [
        ProposedVoucherInput {
            position: 7,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: Some("1"),
            remote_id: None,
            narration_marker: None,
            party: None,
            entries: &rows,
        },
        ProposedVoucherInput {
            position: 7,
            date: "20260812",
            voucher_type: "Receipt",
            voucher_number: Some("2"),
            remote_id: None,
            narration_marker: None,
            party: None,
            entries: &rows,
        },
    ];
    assert_eq!(
        ProposedVoucher::from_inputs(inputs),
        Err(PresenceError::DuplicateProposalPosition)
    );
}
