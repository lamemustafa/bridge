//! Ledgers for the local MCP adapter.
use bridge_tally_core::TallyDate;
use bridge_tally_protocol::group_ancestry::{AncestryChain, AncestryGap, GroupIndex};

use super::*;

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

/// At most this many sub-group names are listed in a filter report. The
/// report is not paged with the rows, so it is bounded where it is built;
/// `group_count` still says how many there were.
const EXCLUDED_SUBGROUPS_NAMED: usize = 20;

/// Applies the `group` filter to rendered rows and reports what it left out
/// (#631), so a narrower answer than "the ledgers in this group" is never
/// silent. Each row's `parent` is the exact returned text the ancestry walk
/// starts from.
///
/// * `excluded_subgroup_ledgers`: rows whose resolved chain reaches `group`
///   through a sub-group, which the immediate scope does not match. Always
///   zero under ancestry scope, which admits them.
/// * `unresolved_ancestry_ledgers`: rows not admitted whose chain stops (a
///   gap) before reaching `group`; Bridge cannot say whether they sit under
///   it, so they are counted rather than treated as outside.
fn apply_group_filter(
    rows: &mut Vec<Value>,
    scope: GroupScope,
    group: &str,
    index: &GroupIndex,
) -> Value {
    let mut excluded = 0usize;
    let mut excluded_groups = std::collections::BTreeSet::new();
    let mut unresolved = 0usize;
    rows.retain(|row| {
        let parent = row["parent"].as_str();
        let chain = index.ancestry_chain(parent);
        let hop_names = chain
            .hops
            .iter()
            .map(|hop| hop.name.clone())
            .collect::<Vec<_>>();
        if group_matches(scope, group, parent, &hop_names) {
            return true;
        }
        match parent {
            Some(parent) if hop_names.iter().any(|name| name == group) => {
                excluded += 1;
                excluded_groups.insert(parent.to_string());
            }
            _ if !chain.is_complete() => unresolved += 1,
            _ => {}
        }
        false
    });
    json!({
        "group": group,
        "scope": match scope {
            GroupScope::Immediate => "immediate",
            GroupScope::Ancestry => "ancestry",
        },
        "excluded_subgroup_ledgers": {
            "count": excluded,
            "group_count": excluded_groups.len(),
            "groups": excluded_groups.into_iter().take(EXCLUDED_SUBGROUPS_NAMED).collect::<Vec<_>>(),
        },
        "unresolved_ancestry_ledgers": unresolved,
    })
}

fn basic_row(ledger: TallyLedger, opening_as_of: &TallyDate) -> Value {
    json!({
        "name": party_name(ledger.name),
        "parent": ledger.parent.returned_text(),
        "opening_balance": ledger.opening_balance,
        "opening_balance_as_of": opening_as_of,
    })
}

impl Server {
    pub(super) async fn ledger_masters(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, mut evidence) = self.verified_company(guid).await?;
        let result: Result<ToolOutcome, ToolFailure> = async {
            let fields = optional_string(args, "fields")?.unwrap_or_else(|| "basic".to_string());
            let compliance = ledger_master_fields(&fields)?;
            let scope = group_scope(args)?;
            let group = optional_string(args, "group")?;
            // A `group` filter needs the group collection under either scope:
            // to admit sub-group ledgers (ancestry) or to report them
            // (immediate). `compliance` already reads it; `basic` reads it
            // only when a filter is given, so an unfiltered read is unchanged.
            let (ledgers, group_filter, ledger_evidence) = if compliance {
                let (records, groups, opening_as_of, evidence) = self
                    .runtime
                    .fetch_agent_party_ledger_masters_with_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("party_ledger_master_read_failed", error))?;
                // Built once per call, not per ledger: the same group
                // collection classifies every row.
                let group_index = GroupIndex::build(groups);
                let mut rows = records
                    .into_iter()
                    .map(|record| {
                        let parent = record.ledger.parent.returned_text().map(str::to_string);
                        let chain = group_index.ancestry_chain(parent.as_deref());
                        json!({
                            "name": party_name(record.ledger.name),
                            "parent": parent,
                            "opening_balance": record.ledger.opening_balance,
                            "opening_balance_as_of": &opening_as_of,
                            "party_gstin": record.ledger.party_gstin.returned_text(),
                            "compliance": mark_compliance_party_names(
                                serde_json::to_value(record.fields).unwrap_or_default(),
                            ),
                            "ancestry": ancestry_json(&chain),
                        })
                    })
                    .collect::<Vec<_>>();
                let report = group
                    .as_deref()
                    .map(|group| apply_group_filter(&mut rows, scope, group, &group_index));
                (rows, report, evidence)
            } else if let Some(group) = group.as_deref() {
                let (records, groups, opening_as_of, evidence) = self
                    .runtime
                    .fetch_ledgers_and_groups_with_opening_as_of_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("ledger_export_invalid", error))?;
                let mut rows = records
                    .into_iter()
                    .map(|ledger| basic_row(ledger, &opening_as_of))
                    .collect::<Vec<_>>();
                let report = apply_group_filter(&mut rows, scope, group, &GroupIndex::build(groups));
                (rows, Some(report), evidence)
            } else {
                let (records, opening_as_of, evidence) = self
                    .runtime
                    .fetch_ledgers_with_opening_as_of_evidence(self.tally_config(), &identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("ledger_export_invalid", error))?;
                let rows = records
                    .into_iter()
                    .map(|ledger| basic_row(ledger, &opening_as_of))
                    .collect::<Vec<_>>();
                (rows, None, evidence)
            };
            evidence = combine_evidence(evidence.clone(), evidence_from_runtime_read(ledger_evidence));
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
            let mut result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
            if let Some(group_filter) = group_filter {
                result["group_filter"] = group_filter;
            }
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
