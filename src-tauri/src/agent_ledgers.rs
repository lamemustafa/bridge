//! Ledgers for the local MCP adapter.
use bridge_tally_protocol::group_ancestry::{AncestryChain, AncestryGap, GroupIndex};

use super::*;
use crate::reports::party_ledger_master::{
    PartyLedgerMasterJoinUnresolved, PartyLedgerMasterJoinUnresolvedReason as JoinReason,
    PartyLedgerMasterJoinUnresolvedSource as JoinSource,
};

fn compliance_row(
    record: bridge_tally_protocol::PartyLedgerMasterRecord,
    group_index: &GroupIndex,
) -> (Value, Vec<String>) {
    let parent = record.ledger.parent.returned_text().map(str::to_string);
    let chain = group_index.ancestry_chain(parent.as_deref());
    let hop_names = chain.hops.iter().map(|hop| hop.name.clone()).collect();
    let row = json!({
        "name": party_name(record.ledger.name),
        "parent": parent,
        "opening_balance": record.ledger.opening_balance,
        "party_gstin": record.ledger.party_gstin.returned_text(),
        "compliance": mark_compliance_party_names(
            serde_json::to_value(record.fields).unwrap_or_default(),
        ),
        "ancestry": ancestry_json(&chain),
    });
    (row, hop_names)
}

/// Renders an observed parent in diagnostics without turning returned-empty
/// into a party name. The marker on nonempty names lets the common egress
/// redactor mask it; `null` still means the source did not return `PARENT`.
fn diagnostic_parent_value(
    parent: &bridge_tally_protocol::PartyLedgerMasterFieldObservation,
) -> Value {
    match parent.returned_text() {
        None => Value::Null,
        Some("") => json!(""),
        Some(text) => json!(party_name(text.to_owned())),
    }
}

fn unresolved_row(observation: PartyLedgerMasterJoinUnresolved) -> Value {
    let source = match observation.source {
        JoinSource::Master => "master",
        JoinSource::Balance => "balance",
    };
    let reason = match observation.reason {
        JoinReason::MasterMissingBalance => "master_missing_balance",
        JoinReason::BalanceWithoutMaster => "balance_without_master",
        JoinReason::DuplicateMasterDisplayKey => "duplicate_master_display_key",
        JoinReason::DuplicateBalanceDisplayKey => "duplicate_balance_display_key",
    };
    let parent = diagnostic_parent_value(&observation.parent);
    json!({
        "join_state": "unresolved", "source": source,
        "source_ordinal": observation.source_ordinal,
        "name": party_name(observation.name), "parent": parent, "reason": reason,
    })
}

/// The `group` filter's match scope. `Immediate` is the tool's original,
/// still-default behaviour (match the ledger's own `PARENT` exactly); it is
/// unchanged for every existing caller. `Ancestry` is new and opt-in: it
/// matches when `group` names *any* group in the ledger's resolved ancestry
/// chain, not only the nearest one — e.g. a `group_scope: "ancestry"` filter
/// for `"Loans (Liability)"` also admits a ledger whose immediate parent is
/// `"Bank OD A/c"`, because that group's own ancestry reaches `"Loans
/// (Liability)"`. A gap in a ledger's chain never counts as a match: an
/// unresolved tail must never be silently guessed into agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupScope {
    Immediate,
    Ancestry,
}

fn group_scope(args: &Value) -> Result<GroupScope, String> {
    match optional_string(args, "group_scope")?.as_deref() {
        None | Some("immediate") => Ok(GroupScope::Immediate),
        Some("ancestry") => Ok(GroupScope::Ancestry),
        Some(_) => Err("argument_invalid:group_scope".to_string()),
    }
}

/// The gap variant as a stable, caller-facing string. Kept separate from
/// `Debug` so the wire vocabulary does not silently change if the enum's
/// variant names or ordering do.
fn ancestry_gap_code(gap: AncestryGap) -> &'static str {
    match gap {
        AncestryGap::NoParent => "no_parent",
        AncestryGap::ReachedRoot => "reached_root",
        AncestryGap::GroupAbsent => "group_absent",
        AncestryGap::GroupNameRepeated => "group_name_repeated",
        AncestryGap::ReservedNameMissing => "reserved_name_missing",
        AncestryGap::Cycle => "cycle",
        AncestryGap::Exhausted => "exhausted",
    }
}

/// Renders one ledger's resolved ancestry for the wire. `hops` is always the
/// true resolved prefix -- never padded past where resolution stopped -- and
/// `complete`/`gap` say plainly whether the caller is looking at the whole
/// chain or an incomplete one. A tax-classification caller must check
/// `complete` before treating `chain` as exhaustive.
fn ancestry_json(chain: &AncestryChain) -> Value {
    json!({
        "chain": chain.hops.iter().map(|hop| json!({
            "name": hop.name,
            "reserved_name": hop.reserved_name,
        })).collect::<Vec<_>>(),
        "complete": chain.is_complete(),
        "gap": chain.gap.map(ancestry_gap_code),
    })
}

/// Whether `group` matches this ledger under the requested scope. `parent` is
/// the ledger's own observed `PARENT`, matched exactly under either scope --
/// this is the tool's original behaviour and stays the whole answer under
/// `GroupScope::Immediate`. `hop_names` (every group name in the ledger's
/// resolved ancestry chain, nearest first) is consulted only under
/// `GroupScope::Ancestry`, and only past that immediate-parent check. A gap
/// in the chain simply means `hop_names` ends early: an unresolved tail is
/// never guessed into a match.
fn group_matches(
    scope: GroupScope,
    group: &str,
    parent: Option<&str>,
    hop_names: &[String],
) -> bool {
    parent == Some(group)
        || (scope == GroupScope::Ancestry && hop_names.iter().any(|name| name == group))
}

impl Server {
    pub(super) async fn ledger_masters(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, mut evidence) = self.verified_company(guid).await?;
        let result: Result<ToolOutcome, ToolFailure> = async {
            let fields = optional_string(args, "fields")?.unwrap_or_else(|| "basic".to_string());
            let mode = ledger_master_fields(&fields)?;
            if mode == LedgerMasterFields::ComplianceDiagnostics {
                if args.get("group").is_some() || args.get("group_scope").is_some() {
                    return Err(ToolFailure::from(
                        "compliance_diagnostics_does_not_support_group_filter".to_string(),
                    ));
                }
                let diagnostic = self.runtime
                    .fetch_agent_party_ledger_master_diagnostics_with_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("party_ledger_master_read_failed", error))?;
                evidence = combine_evidence(evidence.clone(), evidence_from_runtime_read(diagnostic.evidence));
                let partial = !diagnostic.unresolved.is_empty();
                let unresolved_masters = diagnostic.unresolved.iter()
                    .filter(|observation| observation.source == JoinSource::Master).count();
                let coverage = json!({
                    "master_observations": diagnostic.master_observation_count,
                    "balance_observations": diagnostic.balance_observation_count,
                    "matched_pairs": diagnostic.records.len(),
                    "unresolved_master_observations": unresolved_masters,
                    "unresolved_balance_observations": diagnostic.unresolved.len() - unresolved_masters,
                });
                let group_index = GroupIndex::build(diagnostic.groups);
                let matched = diagnostic.records.into_iter().map(|record| {
                    let parent = diagnostic_parent_value(&record.ledger.parent);
                    let (mut row, _) = compliance_row(record, &group_index);
                    // Diagnostic observations can contain user-created parent/group names.
                    row["parent"] = parent;
                    if let Some(hops) = row["ancestry"]["chain"].as_array_mut() {
                        for hop in hops {
                            if let Some(name) = hop["name"].as_str() {
                                hop["name"] = json!(party_name(name.to_owned()));
                            }
                        }
                    }
                    row["join_state"] = json!("matched");
                    row
                });
                let total = matched.len() + diagnostic.unresolved.len();
                let offset = arg_usize(args, "offset", 0)?;
                let limit = arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
                let page = matched.chain(diagnostic.unresolved.into_iter().map(unresolved_row))
                    .skip(offset).take(limit)
                    .map(|row| redact_value(row, self.settings.redaction)).collect::<Vec<_>>();
                let next_offset = offset.saturating_add(page.len());
                let truncated = next_offset < total;
                if partial {
                    evidence.state = "partial";
                    evidence.reason_code = Some("exact_join_unresolved".to_string());
                }
                return Ok(ToolOutcome {
                    payload: json!({
                        "company": company_json(&company, std::slice::from_ref(&company)),
                        "result": {"items": page, "offset": offset, "total": total,
                            "next_offset": if truncated {Some(next_offset)} else {None},
                            "total_basis": "diagnostic_observations", "fields": fields,
                            "state": if partial {"partial"} else {"complete"},
                            "partial_reason": if partial {Some("exact_join_unresolved")} else {None},
                            "coverage": coverage},
                    }),
                    evidence: evidence.clone(), company_guid: Some(guid.to_string()), truncated,
                });
            }
            let compliance = mode == LedgerMasterFields::Compliance;
            let scope = group_scope(args)?;
            if scope == GroupScope::Ancestry && !compliance {
                return Err(ToolFailure::from(
                    "group_scope_ancestry_requires_compliance_fields".to_string(),
                ));
            }
            // Each row carries its own resolved ancestry hop names alongside
            // its JSON so the `group` filter below can consult them without
            // re-parsing the rendered payload; `basic` rows never have any
            // (empty, not absent -- `fields=basic` never reads groups at
            // all, which `group_scope=ancestry` is refused for above).
            let (mut ledgers, ledger_evidence): (Vec<(Value, Vec<String>)>, _) = if compliance {
                let (records, groups, evidence) = self
                    .runtime
                    .fetch_agent_party_ledger_masters_with_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("party_ledger_master_read_failed", error))?;
                // Built once per call, not per ledger: the same group
                // collection classifies every row.
                let group_index = GroupIndex::build(groups);
                (
                    records
                        .into_iter()
                        .map(|record| compliance_row(record, &group_index))
                        .collect::<Vec<_>>(),
                    evidence,
                )
            } else {
                let (records, evidence) = self
                    .runtime
                    .fetch_ledgers_with_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("ledger_export_invalid", error))?;
                (
                    records
                        .into_iter()
                        .map(|ledger| {
                            let row = json!({
                                "name": party_name(ledger.name),
                                "parent": ledger.parent.returned_text(),
                                "opening_balance": ledger.opening_balance,
                            });
                            (row, Vec::new())
                        })
                        .collect::<Vec<_>>(),
                    evidence,
                )
            };
            evidence = combine_evidence(evidence.clone(), evidence_from_runtime_read(ledger_evidence));
            if let Some(group) = optional_string(args, "group")? {
                ledgers.retain(|(ledger, hop_names)| {
                    group_matches(scope, &group, ledger["parent"].as_str(), hop_names)
                });
            }
            let ledgers = ledgers.into_iter().map(|(ledger, _)| ledger).collect::<Vec<_>>();
            let offset = arg_usize(args, "offset", 0)?;
            let limit =
                arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
            let total = ledgers.len();
            let page = ledgers
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|ledger| redact_value(ledger, self.settings.redaction))
                .collect::<Vec<_>>();
            let truncated = offset.saturating_add(page.len()) < total;
            let result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
            Ok(ToolOutcome {
                payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
                evidence: evidence.clone(),
                company_guid: Some(guid.to_string()),
                truncated,
            })
        }
        .await;
        result.map_err(|failure| failure.with_prior_evidence(evidence))
    }
}

#[cfg(test)]
#[path = "agent_ledgers_tests.rs"]
mod tests;
