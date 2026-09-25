//! Ledgers for the local MCP adapter.
use bridge_tally_core::TallyDate;
use bridge_tally_protocol::group_ancestry::{AncestryChain, AncestryGap, GroupIndex};
use bridge_tally_protocol::gst_registration::GstRegistrationHistory;

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

/// A party's GSTIN on one date, where it came from, and both sources as read
/// (bridge#624). The dated registration history decides `gstin`; the flat
/// `PARTYGSTIN` decides it only when the history holds no entries, and never
/// beside an unreadable history. Which source Tally treats as authoritative
/// when they differ is unmeasured, so both are reported and a difference is
/// flagged rather than resolved.
#[derive(Debug, PartialEq, Eq)]
struct PartyGstin {
    gstin: Option<String>,
    status: &'static str,
    /// The in-force entry's `GSTREGISTRATIONTYPE`, so "Regular with no GSTIN"
    /// is not read as unregistered.
    registration_type: Option<String>,
    flat: Option<String>,
    /// The flat field names a GSTIN and a readable history says something
    /// else on that date: another GSTIN, or none.
    sources_disagree: bool,
}

/// The row keys a [`PartyGstin`] answer is reported under.
fn party_gstin_fields(gstin: PartyGstin, as_of: &str) -> serde_json::Map<String, Value> {
    let Value::Object(fields) = json!({
        "party_gstin": gstin.gstin,
        "party_gstin_status": gstin.status,
        "party_gstin_registration_type": gstin.registration_type,
        "party_gstin_as_of": as_of,
        "party_gstin_flat": gstin.flat,
        "gstin_sources_disagree": gstin.sources_disagree,
    }) else {
        unreachable!("a json object literal")
    };
    fields
}

fn party_gstin_on(flat: Option<&str>, history: &GstRegistrationHistory, as_of: &str) -> PartyGstin {
    // `flat` is the field as returned, so an explicit `<PARTYGSTIN/>` is
    // `Some("")`: reported as read, but it names no GSTIN.
    let named = flat.filter(|value| !value.is_empty()).map(str::to_string);
    let flat = flat.map(str::to_string);
    let from_flat = |named: Option<String>, flat: Option<String>| PartyGstin {
        status: if named.is_some() {
            "flat_field"
        } else {
            "not_reported"
        },
        gstin: named,
        registration_type: None,
        flat,
        sources_disagree: false,
    };
    match history {
        GstRegistrationHistory::Unreadable { .. } => PartyGstin {
            gstin: None,
            status: "history_unreadable",
            registration_type: None,
            flat,
            sources_disagree: false,
        },
        GstRegistrationHistory::Entries { entries } if !entries.is_empty() => {
            let in_force = history.in_force(as_of);
            let gstin = in_force.and_then(|entry| entry.gstin.clone());
            PartyGstin {
                status: if gstin.is_some() {
                    "in_force"
                } else {
                    "no_gstin_in_force"
                },
                registration_type: in_force.and_then(|entry| entry.registration_type.clone()),
                sources_disagree: named.is_some() && named != gstin,
                gstin,
                flat,
            }
        }
        GstRegistrationHistory::Entries { .. } | GstRegistrationHistory::NotObserved => {
            from_flat(named, flat)
        }
    }
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
///   it, so they are counted rather than treated as outside. The count is
///   book-wide: a gap in a subtree unrelated to `group` is counted too,
///   because Bridge cannot tell whether `group` lies above the point where
///   the walk stopped.
fn apply_group_filter<Row: std::borrow::Borrow<Value>>(
    rows: &mut Vec<Row>,
    scope: GroupScope,
    group: &str,
    index: &GroupIndex,
) -> Value {
    let mut excluded = 0usize;
    let mut excluded_groups = std::collections::BTreeSet::new();
    let mut unresolved = 0usize;
    rows.retain(|row| {
        let parent = row.borrow()["parent"].as_str();
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

/// How long a ledger listing snapshot may serve its continuation pages
/// (#630). A continuation page is served from the snapshot only while the
/// book's extent, including `ALTMSTID` and `ALTVCHID`, is unchanged. Whether a
/// regroup, an alteration made in Tally's own screens, or a deletion moves
/// those marks is unmeasured (ADR 0004), so a change of that kind can leave a
/// continuation page up to this old. A first page is always read fresh.
const LISTING_SNAPSHOT_TTL: std::time::Duration = std::time::Duration::from_secs(600);

/// The most bytes all listing snapshots may hold together (#630), counted as
/// their rows and frame serialized as JSON plus their groups' names, parents
/// and reserved names. That is a proxy for the memory they take, not a bound
/// on it: a parsed value takes more than its text. The oldest snapshot is
/// dropped first; a listing larger than this is not held.
const LISTING_SNAPSHOT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Which read a listing snapshot holds. A `basic` listing with a `group`
/// filter also holds the group collection, so it is a different read; a
/// trial balance is keyed by its period.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ListingKind {
    Basic,
    BasicWithGroups,
    Compliance,
    TrialBalance { from: TallyDate, to: TallyDate },
}

/// A group collection held with a snapshot, and the bytes it counts toward
/// the cap: its names, parents and reserved names.
pub(super) struct HeldGroups {
    index: GroupIndex,
    bytes: usize,
}

impl HeldGroups {
    pub(super) fn build(groups: Vec<bridge_tally_protocol::TallyNamedMaster>) -> Self {
        let bytes = groups
            .iter()
            .map(|group| {
                group.name.len()
                    + group.parent.returned_text().map_or(0, str::len)
                    + group.reserved_name.as_deref().map_or(0, str::len)
            })
            .sum();
        Self {
            index: GroupIndex::build(groups),
            bytes,
        }
    }
}

/// One logical ledger listing, read once by its first page (#630). The rows
/// are unfiltered and unredacted, held in process memory only, never
/// persisted, dropped after the TTL, when a newer first page replaces them,
/// when a write through this server touches the company, or when the byte cap
/// evicts them.
pub(super) struct ListingSnapshot {
    id: String,
    company_guid: String,
    kind: ListingKind,
    extent: bridge_tally_protocol::outstandings_shared::CompanyBookExtent,
    /// The rows, rendered but unfiltered, unpaged and unredacted.
    pub(super) rows: Arc<Vec<Value>>,
    groups: Option<HeldGroups>,
    /// What a page reports besides its rows (a trial balance's period,
    /// currency and totals); null for a ledger listing.
    pub(super) frame: Value,
    pub(super) evidence: Evidence,
    read_at: String,
    taken: std::time::Instant,
    bytes: usize,
}

impl ListingSnapshot {
    /// A fresh read's snapshot, sized by its rendered rows, frame and groups.
    pub(super) fn new(
        identity: &VerifiedCompanyIdentity,
        kind: ListingKind,
        extent: bridge_tally_protocol::outstandings_shared::CompanyBookExtent,
        rows: Vec<Value>,
        groups: Option<HeldGroups>,
        frame: Value,
        evidence: Evidence,
    ) -> Self {
        let bytes = rows.iter().map(|row| row.to_string().len()).sum::<usize>()
            + frame.to_string().len()
            + groups.as_ref().map_or(0, |groups| groups.bytes);
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            company_guid: identity.company_guid().to_string(),
            kind,
            extent,
            rows: Arc::new(rows),
            groups,
            frame,
            evidence,
            read_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            taken: std::time::Instant::now(),
            bytes,
        }
    }

    pub(super) fn describe(&self, reused: bool) -> Value {
        json!({
            "id": self.id,
            "master_alter_id": self.extent.master_alter_id_high_water().map(|mark| mark.get()),
            "voucher_alter_id": self.extent.voucher_alter_id_high_water().map(|mark| mark.get()),
            "read_at": self.read_at,
            "reused": reused,
        })
    }
}

pub(super) struct ListingSnapshots {
    held: Vec<Arc<ListingSnapshot>>,
    ttl: std::time::Duration,
    max_bytes: usize,
    /// Every company a write dropped, so a test can see the call was made.
    #[cfg(test)]
    dropped: Vec<String>,
}

impl Default for ListingSnapshots {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            ttl: LISTING_SNAPSHOT_TTL,
            max_bytes: LISTING_SNAPSHOT_MAX_BYTES,
            #[cfg(test)]
            dropped: Vec::new(),
        }
    }
}

impl ListingSnapshots {
    /// Drops every snapshot past the TTL, so an expired read is not only
    /// skipped but no longer held.
    fn purge_expired(&mut self) {
        let ttl = self.ttl;
        self.held.retain(|held| held.taken.elapsed() < ttl);
    }

    /// Holds a first page's read, replacing any earlier one for the same
    /// company and kind, and evicting the oldest until the cap holds.
    fn hold(&mut self, snapshot: Arc<ListingSnapshot>) {
        self.purge_expired();
        self.held.retain(|held| {
            !(held
                .company_guid
                .eq_ignore_ascii_case(&snapshot.company_guid)
                && held.kind == snapshot.kind)
        });
        if snapshot.bytes > self.max_bytes {
            return;
        }
        while self.held.iter().map(|held| held.bytes).sum::<usize>() + snapshot.bytes
            > self.max_bytes
        {
            let oldest = self
                .held
                .iter()
                .enumerate()
                .min_by_key(|(_, held)| held.taken)
                .map(|(index, _)| index)
                .expect("a held snapshot while over the cap");
            self.held.remove(oldest);
        }
        self.held.push(snapshot);
    }

    /// The unexpired snapshot for this company and kind, if one is held.
    fn current(&mut self, company_guid: &str, kind: &ListingKind) -> Option<Arc<ListingSnapshot>> {
        self.purge_expired();
        self.held
            .iter()
            .find(|held| held.company_guid.eq_ignore_ascii_case(company_guid) && held.kind == *kind)
            .cloned()
    }

    /// Drops every snapshot of a company, as a write through this server does
    /// before it returns.
    pub(super) fn drop_company(&mut self, company_guid: &str) {
        self.purge_expired();
        self.held
            .retain(|held| !held.company_guid.eq_ignore_ascii_case(company_guid));
        #[cfg(test)]
        self.dropped.push(company_guid.to_string());
    }

    #[cfg(test)]
    pub(super) fn dropped_companies(&self) -> &[String] {
        &self.dropped
    }
}

/// Why a continuation page that named its snapshot could not be served from
/// it: the book moved, or the snapshot is no longer held (expired, replaced
/// by a newer first page, evicted, or never held).
fn snapshot_refusal(cause: &'static str) -> ToolFailure {
    let mut failure = ToolFailure::from("listing_snapshot_changed".to_string());
    failure.cause = Some(cause);
    failure
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
            let offset = arg_usize(args, "offset", 0)?;
            let limit =
                arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
            let snapshot_id = optional_string(args, "snapshot_id")?;
            let kind = match (compliance, group.is_some()) {
                (true, _) => ListingKind::Compliance,
                (false, true) => ListingKind::BasicWithGroups,
                (false, false) => ListingKind::Basic,
            };
            let reused = self
                .continued_listing(&identity, &kind, offset, snapshot_id.as_deref(), &mut evidence)
                .await?;
            // A page served from a snapshot records only what it sent: the
            // identity and extent reads, not its first page's read again.
            let snapshot = match reused.clone() {
                Some(held) => held,
                None => {
                    let fresh = self.hold_listing(self.read_ledger_listing(&identity, kind).await?)?;
                    evidence = combine_evidence(evidence.clone(), fresh.evidence.clone());
                    fresh
                }
            };
            let mut ledgers = snapshot.rows.iter().collect::<Vec<_>>();
            let group_filter = match (group.as_deref(), snapshot.groups.as_ref().map(|groups| &groups.index)) {
                (Some(group), Some(index)) => Some(apply_group_filter(&mut ledgers, scope, group, index)),
                (Some(_), None) => return Err("listing_snapshot_groups_missing".to_string().into()),
                (None, _) => None,
            };
            let total = ledgers.len();
            let page = ledgers
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|ledger| redact_value(ledger.clone(), self.settings.redaction))
                .collect::<Vec<_>>();
            let truncated = offset.saturating_add(page.len()) < total;
            let mut result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
            if let Some(group_filter) = group_filter {
                result["group_filter"] = group_filter;
            }
            result["snapshot"] = snapshot.describe(reused.is_some());
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

    /// The held snapshot a continuation page may be served from (#630), or
    /// `None` when the page must read fresh. A first page always reads fresh.
    /// A later page sends one bracketed extent read, and is served from the
    /// snapshot only while the extent equals the one the snapshot was read
    /// under. When the caller names its snapshot, anything else refuses.
    pub(super) async fn continued_listing(
        &self,
        identity: &VerifiedCompanyIdentity,
        kind: &ListingKind,
        offset: usize,
        snapshot_id: Option<&str>,
        evidence: &mut Evidence,
    ) -> Result<Option<Arc<ListingSnapshot>>, ToolFailure> {
        if offset == 0 {
            return Ok(None);
        }
        let (extent, read) = self
            .runtime
            .fetch_listing_extent(self.tally_config(), identity)
            .await
            .map_err(|error| ToolFailure::from_runtime("listing_extent_read_failed", error))?;
        *evidence = combine_evidence(evidence.clone(), evidence_from_runtime_read(read));
        let held = self
            .listings
            .lock()
            .map_err(|_| "listing_snapshot_store_unavailable".to_string())?
            .current(identity.company_guid(), kind);
        match (held, snapshot_id) {
            (Some(held), Some(id)) if held.id == id => {
                if held.extent != extent {
                    return Err(snapshot_refusal("book_changed_since_first_page"));
                }
                Ok(Some(held))
            }
            (_, Some(_)) => Err(snapshot_refusal("snapshot_not_held")),
            (Some(held), None) if held.extent == extent => Ok(Some(held)),
            (_, None) => Ok(None),
        }
    }

    /// Holds a first page's fresh read as its listing's snapshot.
    pub(super) fn hold_listing(
        &self,
        snapshot: ListingSnapshot,
    ) -> Result<Arc<ListingSnapshot>, ToolFailure> {
        let snapshot = Arc::new(snapshot);
        self.listings
            .lock()
            .map_err(|_| "listing_snapshot_store_unavailable".to_string())?
            .hold(snapshot.clone());
        Ok(snapshot)
    }

    /// Drops every ledger listing snapshot of a company. Every write this
    /// server dispatches calls it before returning, whatever the outcome. A
    /// write from anywhere else (the desktop app's own server, another MCP
    /// process, Tally's screens) never reaches this store: a later page then
    /// relies on the extent check alone.
    pub(super) fn drop_listing_snapshots(&self, company_guid: &str) {
        // Recovered even from a poisoned store: a drop that silently did
        // nothing would let a snapshot outlive the write that made it stale.
        self.listings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drop_company(company_guid);
    }

    /// One fresh read of a ledger listing: the rows unfiltered and unredacted,
    /// the group collection when the kind needs it, and the extent the read
    /// was pinned under.
    async fn read_ledger_listing(
        &self,
        identity: &VerifiedCompanyIdentity,
        kind: ListingKind,
    ) -> Result<ListingSnapshot, ToolFailure> {
        let (rows, groups, extent, read_evidence) = match &kind {
            ListingKind::Compliance => {
                let listing = self
                    .runtime
                    .fetch_agent_party_ledger_masters_with_evidence(self.tally_config(), identity)
                    .await
                    .map_err(|error| {
                        ToolFailure::from_runtime("party_ledger_master_read_failed", error)
                    })?;
                // Built once per read, not per ledger: the same group
                // collection classifies every row.
                let groups = HeldGroups::build(listing.groups);
                let opening_as_of = listing.opening_as_of;
                let gstin_as_of = tally_host_today();
                let rows = listing
                    .records
                    .into_iter()
                    .map(|record| {
                        let parent = record.ledger.parent.returned_text().map(str::to_string);
                        let chain = groups.index.ancestry_chain(parent.as_deref());
                        let gstin = party_gstin_on(
                            record.ledger.party_gstin.returned_text(),
                            &record.fields.gst_registrations,
                            &gstin_as_of,
                        );
                        let mut row = json!({
                            "name": party_name(record.ledger.name),
                            "parent": parent,
                            "opening_balance": record.ledger.opening_balance,
                            "opening_balance_as_of": &opening_as_of,
                            "compliance": mark_compliance_party_names(
                                serde_json::to_value(record.fields).unwrap_or_default(),
                            ),
                            "ancestry": ancestry_json(&chain),
                        });
                        if let Value::Object(fields) = &mut row {
                            fields.extend(party_gstin_fields(gstin, &gstin_as_of));
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                (rows, Some(groups), listing.extent, listing.evidence)
            }
            ListingKind::BasicWithGroups => {
                let (listing, groups) = self
                    .runtime
                    .fetch_ledgers_and_groups_with_opening_as_of_evidence(
                        self.tally_config(),
                        identity,
                    )
                    .await
                    .map_err(|error| ToolFailure::from_runtime("ledger_export_invalid", error))?;
                let rows = listing
                    .ledgers
                    .into_iter()
                    .map(|ledger| basic_row(ledger, &listing.opening_as_of))
                    .collect::<Vec<_>>();
                (
                    rows,
                    Some(HeldGroups::build(groups)),
                    listing.extent,
                    listing.evidence,
                )
            }
            ListingKind::Basic => {
                let listing = self
                    .runtime
                    .fetch_ledgers_with_opening_as_of_evidence(self.tally_config(), identity)
                    .await
                    .map_err(|error| ToolFailure::from_runtime("ledger_export_invalid", error))?;
                let rows = listing
                    .ledgers
                    .into_iter()
                    .map(|ledger| basic_row(ledger, &listing.opening_as_of))
                    .collect::<Vec<_>>();
                (rows, None, listing.extent, listing.evidence)
            }
            ListingKind::TrialBalance { .. } => {
                return Err("listing_kind_not_a_ledger_listing".to_string().into());
            }
        };
        Ok(ListingSnapshot::new(
            identity,
            kind,
            extent,
            rows,
            groups,
            Value::Null,
            evidence_from_runtime_read(read_evidence),
        ))
    }
}

#[cfg(test)]
#[path = "agent_ledgers_tests.rs"]
mod tests;
