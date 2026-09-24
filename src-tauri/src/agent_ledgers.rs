//! Ledgers for the local MCP adapter.
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

/// At most this many excluded ledgers are named; `count` covers them all.
/// The list is not paged with the rows, so it is bounded where it is built.
const EXCLUDED_LEDGERS_NAMED: usize = 20;

/// The foreign-currency ledgers a read left out: how many, and the first
/// [`EXCLUDED_LEDGERS_NAMED`] with their currency. A ledger name is party
/// data and is marked for redaction like every row's name.
fn excluded_ledgers_json(
    foreign: &[bridge_tally_protocol::native_outstandings::ForeignCurrencyLedger],
) -> Value {
    json!({
        "count": foreign.len(),
        "ledgers": foreign
            .iter()
            .take(EXCLUDED_LEDGERS_NAMED)
            .map(|ledger| json!({
                "ledger": party_name(ledger.ledger.clone()),
                "currency": ledger.currency,
            }))
            .collect::<Vec<_>>(),
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
            let (mut ledgers, ledger_evidence, foreign): (Vec<(Value, Vec<String>)>, _, _) = if compliance {
                let crate::tally::runtime::PartyLedgerMasterListing {
                    records,
                    groups,
                    foreign_currency_ledgers_excluded: foreign,
                    opening_as_of,
                    evidence,
                } = self
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
                        .map(|record| {
                            let parent = record.ledger.parent.returned_text().map(str::to_string);
                            let chain = group_index.ancestry_chain(parent.as_deref());
                            let hop_names =
                                chain.hops.iter().map(|hop| hop.name.clone()).collect::<Vec<_>>();
                            let row = json!({
                                "name": party_name(record.ledger.name),
                                "parent": parent,
                                "opening_balance": record.ledger.opening_balance,
                                "opening_balance_as_of": &opening_as_of,
                                "party_gstin": record.ledger.party_gstin.returned_text(),
                                "compliance": mark_compliance_party_names(
                                    serde_json::to_value(record.fields).unwrap_or_default(),
                                ),
                                "ancestry": ancestry_json(&chain),
                            });
                            (row, hop_names)
                        })
                        .collect::<Vec<_>>(),
                    evidence,
                    foreign,
                )
            } else {
                let (records, opening_as_of, evidence) = self
                    .runtime
                    .fetch_ledgers_with_opening_as_of_evidence(self.tally_config(), &identity)
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
                                "opening_balance_as_of": &opening_as_of,
                            });
                            (row, Vec::new())
                        })
                        .collect::<Vec<_>>(),
                    evidence,
                    Vec::new(),
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
            let mut result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
            // A book with several Currency masters: its foreign-currency
            // ledgers are left out and named, never read as rupees (bridge#551).
            if !foreign.is_empty() {
                result["ledgers_scope"] = json!("base_currency_ledgers_only");
                result["foreign_currency_ledgers_excluded"] = excluded_ledgers_json(&foreign);
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
