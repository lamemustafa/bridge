use std::path::Path;

use bridge_tally_core::{ExactDecimal, TallyDate};
use bridge_tally_protocol::{
    PartyLedgerMasterFieldObservation, PartyLedgerMasterFields, TallyNamedMaster,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use super::*;
use crate::reports::party_ledger_master::{
    build_party_ledger_master_workbook, PartyLedgerMasterRow, PartyLedgerMasterSource,
};
use crate::reports::schedule_iii::derivations;
use crate::tally::OutstandingsCurrencyAssertion;

const RESERVED_ROOT: &str = "\u{fffd}#4; Primary";
const WHO: &str = "Synthetic Preparer AB";
const FY_2026: FinancialYear = FinancialYear::beginning_in(2026);

async fn repository_at(options: SqliteConnectOptions) -> TallyMirrorRepository {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .expect("connect to SQLite");
    let repository = TallyMirrorRepository::new(pool);
    repository.migrate().await.expect("run mirror migration");
    repository
}

async fn repository() -> TallyMirrorRepository {
    repository_at("sqlite::memory:".parse().unwrap()).await
}

async fn file_repository(path: &Path) -> TallyMirrorRepository {
    repository_at(
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
}

fn row(name: &str, parent: &str, balance: &str) -> PartyLedgerMasterRow {
    PartyLedgerMasterRow {
        name: name.to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned(parent.to_string()),
        party_gstin: PartyLedgerMasterFieldObservation::NotObserved,
        fields: PartyLedgerMasterFields::default(),
        guid: format!("guid-{name}"),
        master_id: format!("id-{name}"),
        alter_id: "1".to_string(),
        opening_balance: ExactDecimal::zero(),
        closing_balance: Some(ExactDecimal::parse(balance).unwrap()),
    }
}

fn group(name: &str, reserved: &str) -> TallyNamedMaster {
    TallyNamedMaster {
        name: name.to_string(),
        parent: PartyLedgerMasterFieldObservation::Returned(RESERVED_ROOT.to_string()),
        reserved_name: Some(reserved.to_string()),
    }
}

/// A synthetic book with its balances as of `to`.
fn book(to: &str, rows: Vec<PartyLedgerMasterRow>) -> PartyLedgerMasterWorkbook {
    build_party_ledger_master_workbook(PartyLedgerMasterSource {
        company: "Synthetic Books".to_string(),
        company_guid: "company-guid".to_string(),
        currency_assertion: OutstandingsCurrencyAssertion::Inr,
        currency_decimal_places: 2,
        from: TallyDate::parse("20250401").unwrap(),
        to: TallyDate::parse(to).unwrap(),
        rows,
        request_sha256: "0".repeat(64),
        master_response_sha256: "a".repeat(64),
        balance_response_sha256: "b".repeat(64),
        group_response_sha256: "c".repeat(64),
        master_response_bytes: 1,
        balance_response_bytes: 1,
        group_response_bytes: 1,
        groups: vec![
            group("Sundry Debtors", "Sundry Debtors"),
            group("Loans (Liability)", "Loans (Liability)"),
            group("Current Liabilities", "Current Liabilities"),
        ],
    })
    .unwrap()
}

fn key(books_from: &str) -> BookKey {
    BookKey::for_tests("company-guid", "1", books_from)
}

fn context(book: BookKey, year: u16) -> EventContext {
    EventContext {
        book,
        company_display_name: "Synthetic Books".to_string(),
        year: FinancialYear::beginning_in(year),
        declared_by: DeclaredPerson::new(WHO).unwrap(),
        recorded_at_unix_ms: 1_790_000_000_000,
    }
}

fn confirmed(workbook: &PartyLedgerMasterWorkbook, ledger: &str) -> ConfirmedDerivation {
    let guid = LedgerGuid::new(&format!("guid-{ledger}")).unwrap();
    let index = workbook
        .source()
        .rows
        .iter()
        .position(|row| row.name == ledger)
        .unwrap();
    let seen = derivations(workbook).remove(index);
    confirm_seen(workbook, &[(guid, seen)]).unwrap().remove(0)
}

fn set(
    workbook: &PartyLedgerMasterWorkbook,
    ledger: &str,
    head: ScheduleIIIHead,
) -> NewGroupingEvent {
    NewGroupingEvent::Set {
        confirmed: confirmed(workbook, ledger),
        head,
        reason: Text::new("Repayable within twelve months").unwrap(),
        reference: Some(Text::new("WP 7.2").unwrap()),
    }
}

fn heads(set: &DecisionSet) -> Vec<(String, ScheduleIIIHead, u16)> {
    set.decisions()
        .iter()
        .map(|decision| {
            (
                decision.ledger_name_when_made.clone(),
                decision.head,
                decision.year.first_year(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_event_log_refuses_every_update_and_delete() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[set(
                &workbook,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    for statement in [
        "UPDATE schedule_iii_grouping_events SET head = 'trade_payables'",
        "UPDATE schedule_iii_grouping_events SET declared_by = 'someone else'",
        "DELETE FROM schedule_iii_grouping_events",
    ] {
        let error = sqlx::query(statement)
            .execute(&repository.pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("1811"),
            "{statement}"
        );
    }
}

#[tokio::test]
async fn a_recorded_decision_is_in_the_set_and_its_history_keeps_who_why_and_when() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let ids = repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[set(
                &workbook,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();

    let decisions = repository
        .grouping_decision_set(&key("20250401"), FinancialYear::beginning_in(2026))
        .await
        .unwrap();
    assert_eq!(
        heads(&decisions),
        vec![(
            "Term loan".to_string(),
            ScheduleIIIHead::OtherCurrentLiabilities,
            2026
        )]
    );
    let history = repository
        .grouping_history(&key("20250401"), FinancialYear::beginning_in(2026))
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, ids[0]);
    assert_eq!(history[0].event.kind(), GroupingEventKind::Set);
    let RecordedEvent::Set(content) = &history[0].event else {
        panic!("a set event");
    };
    assert_eq!(history[0].declared_by.declared(), WHO);
    assert_eq!(content.reason.as_str(), "Repayable within twelve months");
    assert_eq!(history[0].recorded_at_unix_ms, 1_790_000_000_000);
    assert_eq!(
        &content.made_against,
        confirmed(&workbook, "Term loan").derivation()
    );
}

#[tokio::test]
async fn a_newer_decision_supersedes_and_a_withdrawal_ends_it_but_history_keeps_all() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let context = context(key("20250401"), 2026);
    let first = repository
        .record_grouping_events(
            &context,
            &[set(
                &workbook,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    let second = repository
        .record_grouping_events(
            &context,
            &[set(&workbook, "Term loan", ScheduleIIIHead::TradePayables)],
        )
        .await
        .unwrap();
    let year = FinancialYear::beginning_in(2026);
    assert_eq!(
        heads(
            &repository
                .grouping_decision_set(&key("20250401"), year)
                .await
                .unwrap()
        ),
        vec![(
            "Term loan".to_string(),
            ScheduleIIIHead::TradePayables,
            2026
        )]
    );

    // Withdrawing the superseded decision changes nothing.
    repository
        .record_grouping_events(
            &context,
            &[NewGroupingEvent::Withdraw {
                target: first[0].clone(),
                reason: Text::new("Superseded").unwrap(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .grouping_decision_set(&key("20250401"), year)
            .await
            .unwrap()
            .decisions()
            .len(),
        1
    );

    repository
        .record_grouping_events(
            &context,
            &[NewGroupingEvent::Withdraw {
                target: second[0].clone(),
                reason: Text::new("Presented as the books show").unwrap(),
            }],
        )
        .await
        .unwrap();
    assert!(repository
        .grouping_decision_set(&key("20250401"), year)
        .await
        .unwrap()
        .decisions()
        .is_empty());
    assert_eq!(
        repository
            .grouping_history(&key("20250401"), year)
            .await
            .unwrap()
            .iter()
            .map(|event| event.event.kind())
            .collect::<Vec<_>>(),
        vec![
            GroupingEventKind::Set,
            GroupingEventKind::Set,
            GroupingEventKind::Withdraw,
            GroupingEventKind::Withdraw,
        ]
    );

    // A decision is withdrawn at most once.
    assert!(matches!(
        repository
            .record_grouping_events(
                &context,
                &[NewGroupingEvent::Withdraw {
                    target: second[0].clone(),
                    reason: Text::new("Again").unwrap(),
                }],
            )
            .await,
        Err(GroupingStoreError::Refused(_))
    ));
}

#[tokio::test]
async fn a_withdrawal_or_review_must_name_a_decision_of_the_same_book_and_year() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let ids = repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[set(
                &workbook,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    for other in [
        context(key("20260401"), 2026),
        context(key("20250401"), 2027),
    ] {
        assert!(matches!(
            repository
                .record_grouping_events(
                    &other,
                    &[NewGroupingEvent::Review {
                        target: ids[0].clone()
                    }],
                )
                .await,
            Err(GroupingStoreError::Refused(_))
        ));
    }
    assert!(matches!(
        repository
            .record_grouping_events(
                &context(key("20250401"), 2026),
                &[NewGroupingEvent::Review {
                    target: GroupingEventId::parse("00000000-0000-4000-8000-000000000000").unwrap()
                }],
            )
            .await,
        Err(GroupingStoreError::Refused("grouping_event_target_unknown"))
    ));
}

#[tokio::test]
async fn last_years_decision_is_offered_until_carried_forward_even_across_a_split() {
    let repository = repository().await;
    let last_year = book(
        "20260331",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let ids = repository
        .record_grouping_events(
            &context(key("20250401"), 2025),
            &[set(
                &last_year,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();

    // A year-end split: the new book keeps the GUID, with a new books-from.
    let split = key("20260401");
    let this_year = FinancialYear::beginning_in(2026);
    assert_eq!(
        heads(
            &repository
                .grouping_decision_set(&split, this_year)
                .await
                .unwrap()
        ),
        vec![(
            "Term loan".to_string(),
            ScheduleIIIHead::OtherCurrentLiabilities,
            2025
        )]
    );

    let now = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "450")],
    );
    repository
        .record_grouping_events(
            &context(split.clone(), 2026),
            &[NewGroupingEvent::CarryForward {
                confirmed: confirmed(&now, "Term loan"),
                head: ScheduleIIIHead::OtherCurrentLiabilities,
                reason: Text::new("Repayable within twelve months").unwrap(),
                reference: None,
                from: ids[0].clone(),
            }],
        )
        .await
        .unwrap();
    assert_eq!(
        heads(
            &repository
                .grouping_decision_set(&split, this_year)
                .await
                .unwrap()
        ),
        vec![(
            "Term loan".to_string(),
            ScheduleIIIHead::OtherCurrentLiabilities,
            2026
        )]
    );

    // Another book with the same GUID sees none of this year's decisions.
    assert!(repository
        .grouping_decision_set(&key("20250401"), this_year)
        .await
        .unwrap()
        .decisions()
        .iter()
        .all(|decision| decision.year.first_year() == 2025));
}

#[tokio::test]
async fn a_carry_forward_must_name_last_years_decision_on_the_same_ledger() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let ids = repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[set(
                &workbook,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    assert!(matches!(
        repository
            .record_grouping_events(
                &context(key("20250401"), 2026),
                &[NewGroupingEvent::CarryForward {
                    confirmed: confirmed(&workbook, "Term loan"),
                    head: ScheduleIIIHead::OtherCurrentLiabilities,
                    reason: Text::new("Same year is not a carry-forward").unwrap(),
                    reference: None,
                    from: ids[0].clone(),
                }],
            )
            .await,
        Err(GroupingStoreError::Refused(_))
    ));
}

#[tokio::test]
async fn a_batch_is_stored_whole_or_not_at_all() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![
            row("Term loan", "Loans (Liability)", "500"),
            row("Customer", "Sundry Debtors", "-100"),
        ],
    );
    let refused = repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[
                set(
                    &workbook,
                    "Term loan",
                    ScheduleIIIHead::OtherCurrentLiabilities,
                ),
                NewGroupingEvent::Review {
                    target: GroupingEventId::parse("00000000-0000-4000-8000-000000000000").unwrap(),
                },
            ],
        )
        .await;
    assert!(refused.is_err());
    assert!(repository
        .grouping_history(&key("20250401"), FinancialYear::beginning_in(2026))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_stored_row_this_build_cannot_read_makes_the_whole_set_unreadable() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let derivation = serde_json::to_string(confirmed(&workbook, "Term loan").derivation()).unwrap();
    // As a newer build might write it: a head this build does not know.
    sqlx::query(
        "INSERT INTO schedule_iii_grouping_events(event_sequence, id, company_guid, company_number, \
           books_from_yyyymmdd, company_display_name, financial_year, kind, ledger_guid, ledger_name, \
           made_against, head, reason, declared_by, recorded_at_unix_ms) \
         VALUES (1, '11111111-1111-4111-8111-111111111111', 'company-guid', '1', '20250401', \
           'Synthetic Books', 2026, 'set', 'guid-term loan', 'Term loan', ?1, \
           'reserves_and_surplus', 'A newer head', 'Synthetic Preparer AB', 1)",
    )
    .bind(derivation)
    .execute(&repository.pool)
    .await
    .unwrap();

    let error = repository
        .grouping_decision_set(&key("20250401"), FinancialYear::beginning_in(2026))
        .await
        .unwrap_err();
    assert!(matches!(error, GroupingStoreError::Unreadable));
    assert_eq!(error.unavailable(), DecisionsUnavailable::Unreadable);
}

#[tokio::test]
async fn who_never_appears_in_a_debug_rendering() {
    let repository = repository().await;
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let context = context(key("20250401"), 2026);
    repository
        .record_grouping_events(
            &context,
            &[set(
                &workbook,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    let history = repository
        .grouping_history(&key("20250401"), FinancialYear::beginning_in(2026))
        .await
        .unwrap();
    let person: DeclaredPerson = serde_json::from_str(&format!("\"{WHO}\"")).unwrap();
    for rendering in [
        format!("{context:?}"),
        format!("{history:?}"),
        format!("{person:?}"),
    ] {
        assert!(!rendering.contains(WHO), "{rendering}");
        assert!(rendering.contains("<redacted>"));
    }
    assert!(serde_json::from_str::<DeclaredPerson>("\"  \"").is_err());
}

#[tokio::test]
async fn reopening_keeps_every_event_and_the_new_table_touches_no_older_one() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mirror.db");
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    {
        let repository = file_repository(&path).await;
        repository
            .record_grouping_events(
                &context(key("20250401"), 2026),
                &[set(
                    &workbook,
                    "Term loan",
                    ScheduleIIIHead::OtherCurrentLiabilities,
                )],
            )
            .await
            .unwrap();
    }
    let reopened = file_repository(&path).await;
    assert_eq!(
        reopened
            .grouping_history(&key("20250401"), FinancialYear::beginning_in(2026))
            .await
            .unwrap()
            .len(),
        1
    );
    // An older binary runs only migrations 2-27, each gated on its own
    // marker. Nothing it runs names this table, and this table depends on no
    // older one, so its rows survive such a binary untouched.
    let foreign_tables: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT \"table\" FROM pragma_foreign_key_list('schedule_iii_grouping_events')",
    )
    .fetch_all(&reopened.pool)
    .await
    .unwrap();
    assert_eq!(
        foreign_tables,
        vec!["schedule_iii_grouping_events".to_string()]
    );
    let triggers_elsewhere: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' \
         AND name LIKE 'schedule_iii_grouping_events%' AND tbl_name <> 'schedule_iii_grouping_events'",
    )
    .fetch_one(&reopened.pool)
    .await
    .unwrap();
    assert_eq!(triggers_elsewhere, 0);
}

#[test]
fn a_save_is_refused_when_any_ledger_moved_since_the_export() {
    let seen_at_export = book(
        "20260731",
        vec![
            row("Term loan", "Loans (Liability)", "500"),
            row("Customer", "Sundry Debtors", "-100"),
        ],
    );
    let requests: Vec<GroupingEventRequest> = derivations(&seen_at_export)
        .into_iter()
        .zip(["guid-Term loan", "guid-Customer"])
        .map(|(seen, guid)| GroupingEventRequest::Set {
            ledger_guid: guid.to_string(),
            seen,
            head: "other_current_liabilities".to_string(),
            reason: "Reviewed".to_string(),
            reference: None,
        })
        .collect();
    assert_eq!(
        plan_grouping_events(&seen_at_export, FY_2026, &requests)
            .unwrap()
            .events
            .len(),
        2
    );

    let at_save = book(
        "20260731",
        vec![
            row("Term loan", "Current Liabilities", "500"),
            row("Customer", "Sundry Debtors", "-100"),
        ],
    );
    match plan_grouping_events(&at_save, FY_2026, &requests) {
        Err(GroupingRequestRefusal::Moved(moved)) => {
            assert_eq!(moved.len(), 1);
            assert!(matches!(
                &moved[0],
                SeenMismatch::Moved { ledger_name, .. } if ledger_name == "Term loan"
            ));
        }
        other => panic!("expected a refusal naming the moved ledger, got {other:?}"),
    }
}

#[test]
fn a_request_with_an_invalid_field_or_a_repeated_ledger_is_refused_whole() {
    let workbook = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let seen = derivations(&workbook).remove(0);
    let request = |head: &str, reason: &str| GroupingEventRequest::Set {
        ledger_guid: "guid-Term loan".to_string(),
        seen: seen.clone(),
        head: head.to_string(),
        reason: reason.to_string(),
        reference: None,
    };
    for (requests, code) in [
        (vec![], "grouping_events_empty"),
        (
            vec![request("reserves_and_surplus", "Why")],
            "grouping_head",
        ),
        (vec![request("trade_payables", "   ")], "grouping_reason"),
        (
            vec![
                request("trade_payables", "Why"),
                request("trade_payables", "Why"),
            ],
            "grouping_ledger_repeated",
        ),
        (
            vec![GroupingEventRequest::Withdraw {
                event_id: "not-an-id".to_string(),
                reason: "Why".to_string(),
            }],
            "grouping_event_id",
        ),
    ] {
        assert_eq!(
            plan_grouping_events(&workbook, FY_2026, &requests).err(),
            Some(GroupingRequestRefusal::Invalid(code))
        );
    }
}

#[tokio::test]
async fn a_row_whose_columns_do_not_fit_its_kind_is_unreadable() {
    let repository = repository().await;
    let derivation = r#"{"outcome":{"undetermined":"account_root"},"ancestry":[]}"#;
    let target = "11111111-1111-4111-8111-111111111111";
    for (kind, made_against, head, reason, target) in [
        ("set", None, Some("trade_payables"), Some("Why"), None),
        (
            "carry_forward",
            Some(derivation),
            Some("trade_payables"),
            Some("Why"),
            None,
        ),
        (
            "withdraw",
            None,
            Some("trade_payables"),
            Some("Why"),
            Some(target),
        ),
        ("review", None, None, Some("Why"), Some(target)),
    ] {
        let row = sqlx::query(
            "SELECT 1 AS event_sequence, '22222222-2222-4222-8222-222222222222' AS id, \
               ?1 AS kind, 'Synthetic Books' AS company_display_name, \
               2026 AS financial_year, 'guid-x' AS ledger_guid, 'X' AS ledger_name, \
               ?2 AS made_against, ?3 AS head, ?4 AS reason, NULL AS reference, \
               ?5 AS target_event_id, 'AB' AS declared_by, 1 AS recorded_at_unix_ms",
        )
        .bind(kind)
        .bind(made_against)
        .bind(head)
        .bind(reason)
        .bind(target)
        .fetch_one(&repository.pool)
        .await
        .unwrap();
        assert!(
            matches!(parse_row(&row), Err(GroupingStoreError::Unreadable)),
            "{kind}"
        );
    }
}

#[test]
fn a_save_is_refused_when_the_books_entered_a_new_year_since_the_export() {
    let workbook = book(
        "20270401",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let requests = [GroupingEventRequest::Set {
        ledger_guid: "guid-Term loan".to_string(),
        seen: derivations(&workbook).remove(0),
        head: "other_current_liabilities".to_string(),
        reason: "Reviewed".to_string(),
        reference: None,
    }];
    assert_eq!(
        plan_grouping_events(&workbook, FY_2026, &requests).err(),
        Some(GroupingRequestRefusal::YearChanged {
            seen: FY_2026,
            now: FinancialYear::beginning_in(2027),
        })
    );
}

#[tokio::test]
async fn the_table_refuses_a_row_its_kind_does_not_fit() {
    let repository = repository().await;
    let derivation = r#"{"outcome":{"undetermined":"account_root"},"ancestry":[]}"#;
    // (id, kind, made_against, head, reason, reference, declared_by)
    for (id, kind, made_against, head, reason, reference, declared_by) in [
        (
            Some("a1"),
            "set",
            Some(derivation),
            Some("trade_payables"),
            None,
            None,
            "AB",
        ),
        (
            None,
            "set",
            Some(derivation),
            Some("trade_payables"),
            Some("Why"),
            None,
            "AB",
        ),
        (
            Some("a2"),
            "set",
            Some(derivation),
            Some("trade_payables"),
            Some("Why"),
            Some("  "),
            "AB",
        ),
        (
            Some("a3"),
            "set",
            Some(derivation),
            Some("trade_payables"),
            Some("Why"),
            None,
            "   ",
        ),
        (
            Some("a4"),
            "set",
            None,
            Some("trade_payables"),
            Some("Why"),
            None,
            "AB",
        ),
    ] {
        let error = sqlx::query(
            "INSERT INTO schedule_iii_grouping_events(event_sequence, id, company_guid, \
               company_number, books_from_yyyymmdd, company_display_name, financial_year, kind, \
               ledger_guid, ledger_name, made_against, head, reason, reference, declared_by, \
               recorded_at_unix_ms) \
             VALUES (1, ?1, 'company-guid', '1', '20250401', 'Synthetic Books', 2026, ?2, \
               'guid-x', 'X', ?3, ?4, ?5, ?6, ?7, 1)",
        )
        .bind(id)
        .bind(kind)
        .bind(made_against)
        .bind(head)
        .bind(reason)
        .bind(reference)
        .bind(declared_by)
        .execute(&repository.pool)
        .await
        .unwrap_err();
        let database = error.as_database_error().unwrap();
        assert!(
            database.is_check_violation() || database.code().as_deref() == Some("1299"),
            "{id:?} {kind}: {database:?}"
        );
    }
}

#[tokio::test]
async fn a_withdrawal_this_year_also_ends_last_years_offer() {
    // A withdrawal records this year's choice: present the ledger as the
    // books show it. Last year's decision is then not offered again.
    let repository = repository().await;
    let last_year = book(
        "20260331",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    repository
        .record_grouping_events(
            &context(key("20250401"), 2025),
            &[set(
                &last_year,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    let now = book(
        "20260731",
        vec![row("Term loan", "Loans (Liability)", "500")],
    );
    let this_year = repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[set(
                &now,
                "Term loan",
                ScheduleIIIHead::OtherCurrentLiabilities,
            )],
        )
        .await
        .unwrap();
    repository
        .record_grouping_events(
            &context(key("20250401"), 2026),
            &[NewGroupingEvent::Withdraw {
                target: this_year[0].clone(),
                reason: Text::new("Presented as the books show").unwrap(),
            }],
        )
        .await
        .unwrap();
    assert!(repository
        .grouping_decision_set(&key("20250401"), FY_2026)
        .await
        .unwrap()
        .decisions()
        .is_empty());
}
