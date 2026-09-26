//! The CA grouping-decision store (#737, ADR 0020): an append-only event log
//! in the encrypted mirror, folded into one book's [`DecisionSet`].
//!
//! Every row is parsed here, at the boundary. A row this build cannot read
//! makes the whole set unreadable; it is never skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Deserializer};
use sqlx::Row;
use uuid::Uuid;

use super::tally_mirror::{MirrorError, TallyMirrorRepository};
use crate::reports::party_ledger_master::PartyLedgerMasterWorkbook;
use crate::reports::schedule_iii::{
    confirm_seen, BookKey, ConfirmedDerivation, Decision, DecisionId, DecisionSet,
    DecisionsUnavailable, Derivation, FinancialYear, LedgerGuid, ScheduleIIIError, ScheduleIIIHead,
    SeenMismatch,
};

const MAX_TEXT: usize = 2_000;
const MAX_NAME: usize = 256;

/// The person who records an event, as they declare it: Bridge has no user
/// accounts (ADR 0020). It has no `Serialize` and no `Display`, and its
/// `Debug` is redacted, so it cannot reach a log, a model or an egress
/// receipt by accident. Only the encrypted store and the CA's own screen see
/// it, through [`DeclaredPerson::declared`].
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DeclaredPerson(String);

impl DeclaredPerson {
    pub(crate) fn new(value: &str) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty() && value.len() <= MAX_NAME && !value.chars().any(char::is_control))
            .then(|| Self(value.to_string()))
    }

    /// For the store and the CA's own screen only.
    pub(crate) fn declared(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DeclaredPerson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeclaredPerson(<redacted>)")
    }
}

/// Parsed where it arrives from the CA's screen; there is no `Serialize`.
impl<'de> Deserialize<'de> for DeclaredPerson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).ok_or_else(|| {
            serde::de::Error::custom("a non-empty name or initials of at most 256 characters")
        })
    }
}

/// A reason or reference: trimmed, non-empty, bounded, no control characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Text(String);

impl Text {
    pub(crate) fn new(value: &str) -> Option<Self> {
        let value = value.trim();
        (!value.is_empty()
            && value.len() <= MAX_TEXT
            && !value.chars().any(|c| c.is_control() && c != '\n'))
        .then(|| Self(value.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct GroupingEventId(String);

impl GroupingEventId {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Uuid::parse_str(value)
            .ok()
            .map(|uuid| Self(uuid.to_string()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Who, when, and for which book and year a batch of events is recorded.
#[derive(Debug, Clone)]
pub(crate) struct EventContext {
    pub(crate) book: BookKey,
    pub(crate) company_display_name: String,
    pub(crate) year: FinancialYear,
    pub(crate) declared_by: DeclaredPerson,
    pub(crate) recorded_at_unix_ms: i64,
}

/// What a CA records. A decision needs a [`ConfirmedDerivation`], which only a
/// fresh read that still says what the CA saw can produce.
#[derive(Debug, Clone)]
pub(crate) enum NewGroupingEvent {
    Set {
        confirmed: ConfirmedDerivation,
        head: ScheduleIIIHead,
        reason: Text,
        reference: Option<Text>,
    },
    CarryForward {
        confirmed: ConfirmedDerivation,
        head: ScheduleIIIHead,
        reason: Text,
        reference: Option<Text>,
        from: GroupingEventId,
    },
    Withdraw {
        target: GroupingEventId,
        reason: Text,
    },
    Review {
        target: GroupingEventId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupingEventKind {
    Set,
    CarryForward,
    Withdraw,
    Review,
}

impl GroupingEventKind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::CarryForward => "carry_forward",
            Self::Withdraw => "withdraw",
            Self::Review => "review",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        [Self::Set, Self::CarryForward, Self::Withdraw, Self::Review]
            .into_iter()
            .find(|kind| kind.code() == code)
    }
}

/// One stored event, parsed. The register reads these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GroupingEventRecord {
    pub(crate) sequence: u64,
    pub(crate) id: GroupingEventId,
    pub(crate) event: RecordedEvent,
    pub(crate) company_display_name: String,
    pub(crate) year: FinancialYear,
    pub(crate) ledger: LedgerGuid,
    pub(crate) ledger_name: String,
    pub(crate) declared_by: DeclaredPerson,
    pub(crate) recorded_at_unix_ms: i64,
}

/// A decision's content: what the read said, the head, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecisionContent {
    pub(crate) made_against: Derivation,
    pub(crate) head: ScheduleIIIHead,
    pub(crate) reason: Text,
    pub(crate) reference: Option<Text>,
}

/// A stored event, each kind with exactly the fields it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecordedEvent {
    Set(DecisionContent),
    CarryForward {
        content: DecisionContent,
        from: GroupingEventId,
    },
    Withdraw {
        target: GroupingEventId,
        reason: Text,
    },
    Review {
        target: GroupingEventId,
    },
}

impl RecordedEvent {
    pub(crate) fn kind(&self) -> GroupingEventKind {
        match self {
            Self::Set(_) => GroupingEventKind::Set,
            Self::CarryForward { .. } => GroupingEventKind::CarryForward,
            Self::Withdraw { .. } => GroupingEventKind::Withdraw,
            Self::Review { .. } => GroupingEventKind::Review,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum GroupingStoreError {
    #[error(transparent)]
    Mirror(#[from] MirrorError),
    #[error("the store refused a grouping event ({0})")]
    Refused(&'static str),
    #[error("stored grouping decisions could not be read")]
    Unreadable,
    #[error("stored grouping decisions do not form one decision per ledger")]
    Invalid(#[from] ScheduleIIIError),
}

impl GroupingStoreError {
    /// How an export that could not read its decisions says so.
    pub(crate) fn unavailable(&self) -> DecisionsUnavailable {
        match self {
            Self::Unreadable | Self::Invalid(_) => DecisionsUnavailable::Unreadable,
            Self::Mirror(_) | Self::Refused(_) => DecisionsUnavailable::StoreUnavailable,
        }
    }
}

/// One event as the CA's screen sends it. A decision carries the derivation
/// the CA saw; it is confirmed against a fresh read before anything is stored.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GroupingEventRequest {
    Set {
        ledger_guid: String,
        seen: Derivation,
        head: String,
        reason: String,
        reference: Option<String>,
    },
    CarryForward {
        ledger_guid: String,
        seen: Derivation,
        head: String,
        reason: String,
        reference: Option<String>,
        from_event_id: String,
    },
    Withdraw {
        event_id: String,
        reason: String,
    },
    Review {
        event_id: String,
    },
}

/// Why a batch of requested events was not recorded. Nothing is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupingRequestRefusal {
    Invalid(&'static str),
    /// The read's financial year is not the one the CA saw, for example
    /// because the first voucher of a new year was posted in between.
    YearChanged {
        seen: FinancialYear,
        now: FinancialYear,
    },
    /// The read changed between what the CA saw and the save.
    Moved(Vec<SeenMismatch>),
}

/// Events ready to store, and the financial year they were confirmed for.
#[derive(Debug)]
pub(crate) struct PlannedEvents {
    pub(crate) year: FinancialYear,
    pub(crate) events: Vec<NewGroupingEvent>,
}

/// Turns what the CA sent into events, confirming the financial year and
/// every decision against the fresh read. Any invalid field, a different
/// year, or any ledger whose read moved refuses the whole batch.
pub(crate) fn plan_grouping_events(
    workbook: &PartyLedgerMasterWorkbook,
    seen_year: FinancialYear,
    requests: &[GroupingEventRequest],
) -> Result<PlannedEvents, GroupingRequestRefusal> {
    use GroupingRequestRefusal::Invalid;
    if requests.is_empty() {
        return Err(Invalid("grouping_events_empty"));
    }
    let year =
        FinancialYear::containing(&workbook.source().to).ok_or(Invalid("grouping_read_period"))?;
    if year != seen_year {
        return Err(GroupingRequestRefusal::YearChanged {
            seen: seen_year,
            now: year,
        });
    }
    let ledger = |guid: &str| LedgerGuid::new(guid).ok_or(Invalid("grouping_ledger_guid"));
    let mut seen = Vec::new();
    let mut decided = BTreeSet::new();
    for request in requests {
        if let GroupingEventRequest::Set {
            ledger_guid,
            seen: saw,
            ..
        }
        | GroupingEventRequest::CarryForward {
            ledger_guid,
            seen: saw,
            ..
        } = request
        {
            let ledger = ledger(ledger_guid)?;
            if !decided.insert(ledger.clone()) {
                return Err(Invalid("grouping_ledger_repeated"));
            }
            seen.push((ledger, saw.clone()));
        }
    }
    let mut confirmed = confirm_seen(workbook, &seen)
        .map_err(GroupingRequestRefusal::Moved)?
        .into_iter();
    let head = |code: &str| ScheduleIIIHead::from_code(code).ok_or(Invalid("grouping_head"));
    let reason = |text: &str| Text::new(text).ok_or(Invalid("grouping_reason"));
    let reference = |text: &Option<String>| {
        text.as_deref()
            .map(|value| Text::new(value).ok_or(Invalid("grouping_reference")))
            .transpose()
    };
    let event_id = |id: &str| GroupingEventId::parse(id).ok_or(Invalid("grouping_event_id"));
    let mut next_confirmed = || {
        confirmed
            .next()
            .ok_or(Invalid("grouping_events_unconfirmed"))
    };
    let events = requests
        .iter()
        .map(|request| {
            Ok(match request {
                GroupingEventRequest::Set {
                    head: code,
                    reason: why,
                    reference: note,
                    ..
                } => NewGroupingEvent::Set {
                    confirmed: next_confirmed()?,
                    head: head(code)?,
                    reason: reason(why)?,
                    reference: reference(note)?,
                },
                GroupingEventRequest::CarryForward {
                    head: code,
                    reason: why,
                    reference: note,
                    from_event_id,
                    ..
                } => NewGroupingEvent::CarryForward {
                    confirmed: next_confirmed()?,
                    head: head(code)?,
                    reason: reason(why)?,
                    reference: reference(note)?,
                    from: event_id(from_event_id)?,
                },
                GroupingEventRequest::Withdraw {
                    event_id: id,
                    reason: why,
                } => NewGroupingEvent::Withdraw {
                    target: event_id(id)?,
                    reason: reason(why)?,
                },
                GroupingEventRequest::Review { event_id: id } => NewGroupingEvent::Review {
                    target: event_id(id)?,
                },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PlannedEvents { year, events })
}

/// SQLite's extended result code for a `RAISE(ABORT)` in a trigger.
const SQLITE_CONSTRAINT_TRIGGER: &str = "1811";

impl From<sqlx::Error> for GroupingStoreError {
    fn from(error: sqlx::Error) -> Self {
        // A trigger or constraint refusal is the store saying no, not an
        // outage; tell them apart by typed code, never by message text.
        match &error {
            sqlx::Error::Database(database)
                if database.code().as_deref() == Some(SQLITE_CONSTRAINT_TRIGGER)
                    || database.is_check_violation()
                    || database.is_unique_violation()
                    || database.is_foreign_key_violation() =>
            {
                Self::Refused("grouping_event_constraint")
            }
            _ => Self::Mirror(MirrorError::Database(error)),
        }
    }
}

impl TallyMirrorRepository {
    /// Records a batch in one transaction: all of it, or none of it.
    pub(crate) async fn record_grouping_events(
        &self,
        context: &EventContext,
        events: &[NewGroupingEvent],
    ) -> Result<Vec<GroupingEventId>, GroupingStoreError> {
        if events.is_empty() {
            return Err(GroupingStoreError::Refused("grouping_events_empty"));
        }
        let display_name = context.company_display_name.trim();
        if display_name.is_empty() || display_name.len() > MAX_NAME {
            return Err(GroupingStoreError::Refused("grouping_company_display_name"));
        }
        if context.recorded_at_unix_ms <= 0 {
            return Err(GroupingStoreError::Refused("grouping_recorded_at"));
        }
        let mut transaction = self.pool.begin().await?;
        let mut ids = Vec::with_capacity(events.len());
        for event in events {
            let id = GroupingEventId(Uuid::new_v4().to_string());
            let (kind, content, target, reason) = match event {
                NewGroupingEvent::Set {
                    confirmed,
                    head,
                    reason,
                    reference,
                } => (
                    GroupingEventKind::Set,
                    Some((confirmed, *head, reference.as_ref())),
                    None,
                    Some(reason),
                ),
                NewGroupingEvent::CarryForward {
                    confirmed,
                    head,
                    reason,
                    reference,
                    from,
                } => (
                    GroupingEventKind::CarryForward,
                    Some((confirmed, *head, reference.as_ref())),
                    Some(from),
                    Some(reason),
                ),
                NewGroupingEvent::Withdraw { target, reason } => (
                    GroupingEventKind::Withdraw,
                    None,
                    Some(target),
                    Some(reason),
                ),
                NewGroupingEvent::Review { target } => {
                    (GroupingEventKind::Review, None, Some(target), None)
                }
            };
            let (ledger, ledger_name) = match (content, target) {
                (Some((confirmed, _, _)), _) => (
                    confirmed.ledger().as_str().to_string(),
                    confirmed.ledger_name().to_string(),
                ),
                // A withdrawal or review is about the decision it names.
                (None, Some(target)) => {
                    let row = sqlx::query(
                        "SELECT ledger_guid, ledger_name FROM schedule_iii_grouping_events WHERE id = ?1",
                    )
                    .bind(target.as_str())
                    .fetch_optional(&mut *transaction)
                    .await?
                    .ok_or(GroupingStoreError::Refused("grouping_event_target_unknown"))?;
                    (row.try_get("ledger_guid")?, row.try_get("ledger_name")?)
                }
                (None, None) => return Err(GroupingStoreError::Refused("grouping_event_shape")),
            };
            let made_against = content
                .map(|(confirmed, _, _)| serde_json::to_string(confirmed.derivation()))
                .transpose()
                .map_err(MirrorError::from)?;
            sqlx::query(
                "INSERT INTO schedule_iii_grouping_events(\
                   event_sequence, id, company_guid, company_number, books_from_yyyymmdd, \
                   company_display_name, financial_year, kind, ledger_guid, ledger_name, \
                   made_against, head, reason, reference, target_event_id, declared_by, \
                   recorded_at_unix_ms\
                 ) VALUES ((SELECT COALESCE(MAX(event_sequence), 0) + 1 FROM schedule_iii_grouping_events), \
                   ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            )
            .bind(id.as_str())
            .bind(context.book.company_guid())
            .bind(context.book.company_number())
            .bind(context.book.books_from_yyyymmdd())
            .bind(display_name)
            .bind(i64::from(context.year.first_year()))
            .bind(kind.code())
            .bind(ledger)
            .bind(ledger_name)
            .bind(made_against)
            .bind(content.map(|(_, head, _)| head.code()))
            .bind(reason.map(Text::as_str))
            .bind(content.and_then(|(_, _, reference)| reference.map(Text::as_str)))
            .bind(target.map(GroupingEventId::as_str))
            .bind(context.declared_by.declared())
            .bind(context.recorded_at_unix_ms)
            .execute(&mut *transaction)
            .await?;
            ids.push(id);
        }
        transaction.commit().await?;
        Ok(ids)
    }

    /// Every event of this book for `year`, and of any book with this GUID for
    /// the previous year (a year-end split keeps the GUID), in order.
    pub(crate) async fn grouping_history(
        &self,
        book: &BookKey,
        year: FinancialYear,
    ) -> Result<Vec<GroupingEventRecord>, GroupingStoreError> {
        let previous = year.first_year().checked_sub(1).map(i64::from);
        let rows = sqlx::query(
            "SELECT event_sequence, id, kind, company_display_name, financial_year, ledger_guid, \
                    ledger_name, made_against, head, reason, reference, target_event_id, \
                    declared_by, recorded_at_unix_ms \
             FROM schedule_iii_grouping_events \
             WHERE company_guid = ?1 AND ( \
                 (financial_year = ?2 AND company_number = ?3 AND books_from_yyyymmdd = ?4) \
                 OR financial_year = ?5) \
             ORDER BY event_sequence",
        )
        .bind(book.company_guid())
        .bind(i64::from(year.first_year()))
        .bind(book.company_number())
        .bind(book.books_from_yyyymmdd())
        .bind(previous)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(parse_row).collect()
    }

    /// The decisions a view of this book for `year` is built with.
    pub(crate) async fn grouping_decision_set(
        &self,
        book: &BookKey,
        year: FinancialYear,
    ) -> Result<DecisionSet, GroupingStoreError> {
        let history = self.grouping_history(book, year).await?;
        Ok(fold(book.clone(), year, &history)?)
    }
}

fn parse_row(row: &sqlx::sqlite::SqliteRow) -> Result<GroupingEventRecord, GroupingStoreError> {
    let unreadable = |_| GroupingStoreError::Unreadable;
    let text = |column: &str| -> Result<Option<Text>, GroupingStoreError> {
        row.try_get::<Option<String>, _>(column)
            .map_err(unreadable)?
            .map(|value| Text::new(&value).ok_or(GroupingStoreError::Unreadable))
            .transpose()
    };
    let sequence: i64 = row.try_get("event_sequence").map_err(unreadable)?;
    let year: i64 = row.try_get("financial_year").map_err(unreadable)?;
    let made_against = row
        .try_get::<Option<String>, _>("made_against")
        .map_err(unreadable)?
        .map(|json| serde_json::from_str::<Derivation>(&json))
        .transpose()
        .map_err(|_| GroupingStoreError::Unreadable)?;
    let head = row
        .try_get::<Option<String>, _>("head")
        .map_err(unreadable)?
        .map(|code| ScheduleIIIHead::from_code(&code).ok_or(GroupingStoreError::Unreadable))
        .transpose()?;
    let target = row
        .try_get::<Option<String>, _>("target_event_id")
        .map_err(unreadable)?
        .map(|id| GroupingEventId::parse(&id).ok_or(GroupingStoreError::Unreadable))
        .transpose()?;
    let kind = GroupingEventKind::from_code(&row.try_get::<String, _>("kind").map_err(unreadable)?)
        .ok_or(GroupingStoreError::Unreadable)?;
    let reason = text("reason")?;
    let reference = text("reference")?;
    // Each kind takes exactly its columns. The table's CHECK holds which
    // columns are present; the text rules (trimming, control characters,
    // lengths in bytes) are Rust's. A row failing either is unreadable, never
    // skipped.
    let event = match (kind, made_against, head, reason, reference, target) {
        (GroupingEventKind::Set, Some(made_against), Some(head), Some(reason), reference, None) => {
            RecordedEvent::Set(DecisionContent {
                made_against,
                head,
                reason,
                reference,
            })
        }
        (
            GroupingEventKind::CarryForward,
            Some(made_against),
            Some(head),
            Some(reason),
            reference,
            Some(from),
        ) => RecordedEvent::CarryForward {
            content: DecisionContent {
                made_against,
                head,
                reason,
                reference,
            },
            from,
        },
        (GroupingEventKind::Withdraw, None, None, Some(reason), None, Some(target)) => {
            RecordedEvent::Withdraw { target, reason }
        }
        (GroupingEventKind::Review, None, None, None, None, Some(target)) => {
            RecordedEvent::Review { target }
        }
        _ => return Err(GroupingStoreError::Unreadable),
    };
    Ok(GroupingEventRecord {
        sequence: u64::try_from(sequence).map_err(|_| GroupingStoreError::Unreadable)?,
        id: GroupingEventId::parse(&row.try_get::<String, _>("id").map_err(unreadable)?)
            .ok_or(GroupingStoreError::Unreadable)?,
        event,
        company_display_name: row.try_get("company_display_name").map_err(unreadable)?,
        year: FinancialYear::beginning_in(
            u16::try_from(year).map_err(|_| GroupingStoreError::Unreadable)?,
        ),
        ledger: LedgerGuid::new(
            &row.try_get::<String, _>("ledger_guid")
                .map_err(unreadable)?,
        )
        .ok_or(GroupingStoreError::Unreadable)?,
        ledger_name: row.try_get("ledger_name").map_err(unreadable)?,
        declared_by: DeclaredPerson::new(
            &row.try_get::<String, _>("declared_by")
                .map_err(unreadable)?,
        )
        .ok_or(GroupingStoreError::Unreadable)?,
        recorded_at_unix_ms: row.try_get("recorded_at_unix_ms").map_err(unreadable)?,
    })
}

/// The active decision per ledger and year: the latest set or carry-forward,
/// unless a later withdrawal names it. A previous-year decision is offered
/// only while the ledger has no event in the current year.
fn fold(
    book: BookKey,
    year: FinancialYear,
    history: &[GroupingEventRecord],
) -> Result<DecisionSet, ScheduleIIIError> {
    let mut active: BTreeMap<(FinancialYear, LedgerGuid), Option<(&GroupingEventId, Decision)>> =
        BTreeMap::new();
    for event in history {
        let key = (event.year, event.ledger.clone());
        match &event.event {
            RecordedEvent::Set(content) | RecordedEvent::CarryForward { content, .. } => {
                active.insert(
                    key,
                    Some((
                        &event.id,
                        Decision {
                            id: DecisionId(event.sequence),
                            ledger: event.ledger.clone(),
                            ledger_name_when_made: event.ledger_name.clone(),
                            made_against: content.made_against.clone(),
                            head: content.head,
                            year: event.year,
                        },
                    )),
                );
            }
            RecordedEvent::Withdraw { target, .. } => {
                let withdraws_active = matches!(
                    active.get(&key),
                    Some(Some((active_id, _))) if *active_id == target
                );
                if withdraws_active {
                    active.insert(key, None);
                }
            }
            RecordedEvent::Review { .. } => {}
        }
    }
    let decided_this_year: Vec<&LedgerGuid> = active
        .keys()
        .filter(|(event_year, _)| *event_year == year)
        .map(|(_, ledger)| ledger)
        .collect();
    let decisions = active
        .iter()
        .filter(|((event_year, ledger), _)| {
            *event_year == year || !decided_this_year.contains(&ledger)
        })
        .filter_map(|(_, decision)| decision.as_ref().map(|(_, decision)| decision.clone()))
        .collect();
    DecisionSet::new(book, year, decisions)
}

#[cfg(test)]
#[path = "grouping_decisions_tests.rs"]
mod tests;
