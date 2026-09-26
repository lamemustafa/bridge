//! Desktop adapters for CA grouping decisions (#737, ADR 0020). Thin: the
//! rules live in `reports::schedule_iii` and `db::grouping_decisions`. No MCP
//! tool reaches them, so a decision never leaves the machine except in the
//! workbook the CA exports.
use super::*;
use crate::db::grouping_decisions::{
    plan_grouping_events, DeclaredPerson, EventContext, GroupingEventRecord, GroupingEventRequest,
    GroupingRequestRefusal, GroupingStoreError, RecordedEvent, Text,
};
use crate::reports::schedule_iii::{Derivation, SeenMismatch};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordGroupingDecisionsRequest {
    config: TallyConfig,
    selected_company: SelectedCompanyIdentity,
    declared_by: DeclaredPerson,
    /// The financial year of the export the CA decided from.
    financial_year: u16,
    events: Vec<GroupingEventRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupingDecisionHistoryRequest {
    config: TallyConfig,
    selected_company: SelectedCompanyIdentity,
    financial_year: u16,
}

/// One stored event for the CA's own screen. `declared_by` crosses to the
/// local webview here, explicitly, and nowhere else.
#[derive(Serialize)]
pub struct GroupingEventView {
    sequence: u64,
    id: String,
    kind: &'static str,
    company_display_name: String,
    financial_year: u16,
    ledger_guid: String,
    ledger_name: String,
    made_against: Option<Derivation>,
    head: Option<&'static str>,
    reason: Option<String>,
    reference: Option<String>,
    target_event_id: Option<String>,
    declared_by: String,
    recorded_at_unix_ms: i64,
}

/// Records a batch of grouping events. The save re-reads the book through the
/// export's verified path; if any ledger's read differs from what the CA saw,
/// nothing is stored and every such ledger is named.
#[tauri::command]
pub async fn record_grouping_decisions(
    request: RecordGroupingDecisionsRequest,
    runtime: State<'_, TallyRuntime>,
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<Vec<String>, TallyCommandError> {
    // Open the store first: its first open can wait on a keychain prompt, and
    // the read is confirmed as close to the write as possible.
    let repository = mirror
        .get()
        .await
        .map_err(mirror_unavailable_command_error)?;
    let (identity, workbook) =
        fetch_verified_party_ledger_master(&runtime, request.config, &request.selected_company)
            .await?;
    let planned = plan_grouping_events(
        &workbook,
        FinancialYear::beginning_in(request.financial_year),
        &request.events,
    )
    .map_err(grouping_refusal_error)?;
    let context = EventContext {
        book: BookKey::of(&identity),
        company_display_name: identity.display_name().to_string(),
        year: planned.year,
        declared_by: request.declared_by,
        recorded_at_unix_ms: chrono::Utc::now().timestamp_millis(),
    };
    let ids = repository
        .record_grouping_events(&context, &planned.events)
        .await
        .map_err(grouping_store_error)?;
    Ok(ids.iter().map(|id| id.as_str().to_string()).collect())
}

/// Every stored event for the verified book and year, and the previous year's
/// for its GUID, in order: the register's source.
#[tauri::command]
pub async fn grouping_decision_history(
    request: GroupingDecisionHistoryRequest,
    runtime: State<'_, TallyRuntime>,
    mirror: State<'_, crate::LazyTallyMirror>,
) -> Result<Vec<GroupingEventView>, TallyCommandError> {
    let identity =
        verify_observed_company_tuple(&runtime, &request.config, &request.selected_company).await?;
    let repository = mirror
        .get()
        .await
        .map_err(mirror_unavailable_command_error)?;
    let history = repository
        .grouping_history(
            &BookKey::of(&identity),
            FinancialYear::beginning_in(request.financial_year),
        )
        .await
        .map_err(grouping_store_error)?;
    Ok(history.into_iter().map(event_view).collect())
}

fn event_view(record: GroupingEventRecord) -> GroupingEventView {
    let text = |text: &Text| text.as_str().to_string();
    let (made_against, head, reason, reference, target) = match &record.event {
        RecordedEvent::Set(content) => (
            Some(content.made_against.clone()),
            Some(content.head.code()),
            Some(text(&content.reason)),
            content.reference.as_ref().map(text),
            None,
        ),
        RecordedEvent::CarryForward { content, from } => (
            Some(content.made_against.clone()),
            Some(content.head.code()),
            Some(text(&content.reason)),
            content.reference.as_ref().map(text),
            Some(from.as_str().to_string()),
        ),
        RecordedEvent::Withdraw { target, reason } => (
            None,
            None,
            Some(text(reason)),
            None,
            Some(target.as_str().to_string()),
        ),
        RecordedEvent::Review { target } => {
            (None, None, None, None, Some(target.as_str().to_string()))
        }
    };
    GroupingEventView {
        sequence: record.sequence,
        id: record.id.as_str().to_string(),
        kind: record.event.kind().code(),
        company_display_name: record.company_display_name,
        financial_year: record.year.first_year(),
        ledger_guid: record.ledger.as_str().to_string(),
        ledger_name: record.ledger_name,
        made_against,
        head,
        reason,
        reference,
        target_event_id: target,
        declared_by: record.declared_by.declared().to_string(),
        recorded_at_unix_ms: record.recorded_at_unix_ms,
    }
}

/// How many changed ledgers a refusal names; the count is always given.
const MISMATCHES_NAMED: usize = 10;

fn grouping_refusal_error(refusal: GroupingRequestRefusal) -> TallyCommandError {
    match refusal {
        GroupingRequestRefusal::Invalid(code) => tally_command_error(
            code,
            "Operation",
            "Bridge refused the grouping decisions: a field was missing or not valid. Nothing was saved.",
            "after_change",
            false,
            "Correct the decision and save again.",
        ),
        GroupingRequestRefusal::YearChanged { seen, now } => tally_command_error(
            "grouping_year_changed_since_export",
            "Operation",
            format!(
                "The books moved from the financial year beginning April {} to the one beginning April {} after the export, so nothing was saved.",
                seen.first_year(),
                now.first_year(),
            ),
            "after_change",
            false,
            "Export again and decide against the new year's export.",
        ),
        GroupingRequestRefusal::Moved(mismatches) => tally_command_error(
            "grouping_read_changed_since_export",
            "Operation",
            format!(
                "The books changed after the export, so nothing was saved. {} changed ledger(s): {}{}.",
                mismatches.len(),
                mismatches
                    .iter()
                    .take(MISMATCHES_NAMED)
                    .map(|mismatch| match mismatch {
                        SeenMismatch::LedgerMissing { ledger } => {
                            format!("{} (no longer in the books)", ledger.as_str())
                        }
                        SeenMismatch::Moved { ledger_name, .. } => ledger_name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
                if mismatches.len() > MISMATCHES_NAMED {
                    "; and more"
                } else {
                    ""
                }
            ),
            "after_change",
            false,
            "Export again, review the changed ledgers, and save your decisions against the new export.",
        ),
    }
}

fn grouping_store_error(error: GroupingStoreError) -> TallyCommandError {
    let (code, message) = match error {
        GroupingStoreError::Refused(code) => (
            code,
            "The encrypted store refused the grouping decisions. Nothing was saved.",
        ),
        GroupingStoreError::Unreadable | GroupingStoreError::Invalid(_) => (
            "grouping_decisions_unreadable",
            "The stored grouping decisions could not be read by this version of ComplyEaze Bridge.",
        ),
        GroupingStoreError::Mirror(_) => (
            "grouping_store_failed",
            "The encrypted store could not complete the grouping-decision operation.",
        ),
    };
    tally_command_error(
        code,
        "Operation",
        message,
        "after_change",
        false,
        "Retry; if it persists, check that this is the latest version of ComplyEaze Bridge.",
    )
}
