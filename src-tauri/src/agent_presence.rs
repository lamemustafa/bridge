//! "Which of these are already in the book?" for the local MCP adapter.
//!
//! The rules live in `bridge_tally_core::book_presence` so this tool and any
//! later desktop screen cannot drift apart; see
//! `docs/adr/0017-voucher-presence-authority.md`. This file owns only the two
//! qualified reads that produce the evidence, the typed parse of the caller's
//! proposals, and the response shape.
use super::*;
use std::collections::BTreeSet;

use bridge_tally_core::book_presence::{
    self, BookWindow, NumberingDeclaration, NumberingMethod, ObservedEntry, ObservedVoucher,
    ObservedWindow, PresenceError, PresenceReport, PresenceRequest, ProposedVoucher,
    ProposedVoucherInput, RawObservationBudget, WindowRead,
};
use bridge_tally_core::book_presence::{ColumnEvidence, ObservedMarker};
use bridge_tally_core::master_binding::{MasterCatalog, MasterClass, SourceEntity};

/// Most vouchers one presence request may propose. The window read is
/// unaffected by this: it always reads its whole range.
pub(super) const MAX_PRESENCE_VOUCHERS: usize = 500;
/// Most voucher types one numbering declaration may name.
pub(super) const MAX_PRESENCE_VOUCHER_TYPES: usize = 50;
/// Share of the byte cap the fixed observations may occupy.
///
/// `fit_response` can trim only `items`; `book` is a sibling it cannot reach,
/// and the final framing serializes the whole payload **twice** -- once as
/// `structuredContent` and again as text for clients that read only that. So a
/// maximal `book` (twenty-five duplicate-number groups of ten 128-character
/// keys, twenty-five unbalanced keys, labels at their bound) runs to six
/// figures on its own, and the doubled envelope clears the default cap without
/// a single large item. The report would then be discarded wholesale *after*
/// all three Tally reads were paid for.
///
/// An eighth leaves the doubled observations at a quarter of the cap.
const OBSERVATION_BUDGET_DIVISOR: usize = 8;
/// Most ledger entries one proposed voucher may carry.
pub(super) const MAX_PRESENCE_ENTRIES: usize = 200;
/// Enforces the published `inputSchema` on this tool's nested arrays.
///
/// The shared argument validator stops at the outer selectors, and the core
/// crate's own limits are far wider than this tool advertises, so the gap has
/// to be closed somewhere. Closing it by restating the bounds in this parser
/// would put two copies of every limit in the tree; driving it from the schema
/// itself keeps one.
fn enforce_published_schema(args: &Value) -> Result<(), String> {
    let definitions = catalog::registered_tool_definitions(true, true);
    let schema = definitions
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "voucher_presence"))
        .map(|tool| tool["inputSchema"].clone())
        .ok_or_else(|| "tool_not_found".to_string())?;
    for key in ["numbering", "vouchers"] {
        if let Some(value) = args.get(key) {
            catalog::validate_against_schema(value, &schema["properties"][key], key)?;
        }
    }
    Ok(())
}

impl Server {
    pub(super) async fn voucher_presence(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        if from > to {
            return Err("invalid_date_range".to_string().into());
        }
        // Parse the caller's own input before any Tally read: a malformed
        // proposal set should never cost a read.
        enforce_published_schema(args)?;
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let numbering = parse_numbering(args)?;
        let proposals = parse_proposals(args)?;
        // Both remaining cross-input refusals depend only on the arguments, so
        // they are settled here rather than after three Tally reads. The crate
        // enforces them again at its own boundary; this only stops a request
        // that was always going to be refused from exercising the endpoint.
        for proposal in proposals.iter() {
            // A party's *entity shape* -- how many identifiers its name
            // carries -- is decided entirely by the caller's text, and the
            // crate parses it inside `PresenceRequest::new`, three reads
            // later. Parsing it here keeps the promise the refusal path
            // already makes everywhere else: an input this tool was always
            // going to reject costs no Tally read.
            if let Some(party) = proposal.party() {
                SourceEntity::new(proposal.position(), party)
                    .map_err(|error| error.safe_reason_code().to_string())?;
            }
            if proposal.date() < from.as_str() || proposal.date() > to.as_str() {
                return Err(PresenceError::WindowDoesNotCover
                    .safe_reason_code()
                    .to_string()
                    .into());
            }
            if !numbering.declares(proposal.voucher_type()) {
                return Err(PresenceError::NumberingMethodUndeclared
                    .safe_reason_code()
                    .to_string()
                    .into());
            }
        }

        let (company, identity, accumulated) = self.verified_company(guid).await?;
        let mut accumulated = Some(accumulated);
        let outcome = async {
            let (catalogue, catalogue_evidence) =
                self.read_ledger_catalogue(&identity, &company.name).await?;
            accumulate(&mut accumulated, catalogue_evidence);
            let catalog = MasterCatalog::new(MasterClass::Ledger, &catalogue)
                .map_err(|error| error.safe_reason_code().to_string())?;

            let request = render_agent_vouchers(&company.name, &from, &to, None)?;
            let (xml, evidence) = self.post_read(&identity, request).await?;
            accumulate(&mut accumulated, evidence);
            let rows = validate_then_filter_voucher_rows(
                parse_agent_rows(&xml, identity.company_guid())?,
                &from,
                &to,
                None,
            )?;

            // The window is independent evidence about which ledgers exist.
            // A row posting to an unlisted ledger proves the first catalogue
            // short, regardless of whether a later window qualification could
            // have authorised a verdict. Refuse before the nonempty hold so
            // this distinct source defect remains visible without a redundant
            // paired catalogue read.
            for row in &rows {
                let entry_ledgers = row["amounts"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|entry| entry["ledger"].as_str());
                for ledger in row["party"].as_str().into_iter().chain(entry_ledgers) {
                    if catalog.exact(ledger).is_none() {
                        return Err("ledger_catalogue_incomplete".to_string().into());
                    }
                }
            }

            // A window can only license `Absent` when its cardinality is
            // independently established. The existing empty-window control
            // can establish that narrow case. A nonempty response has no
            // source-side count, so a well-formed bounded response cannot be
            // promoted to Complete merely because it contains rows.
            let mut read = WindowRead::Partial;
            let mut reason = Some("nonempty_window_unqualified");
            if rows.is_empty() {
                let (read_evidence, partial, corroboration) = self
                    .corroborate_empty_voucher_read(&identity, &company.name, &from, &to, None)
                    .await?;
                accumulate(&mut accumulated, read_evidence);
                reason = corroboration;
                if partial {
                    read = WindowRead::Partial;
                    if let Some(evidence) = accumulated.as_mut() {
                        evidence.state = "partial";
                        evidence.reason_code = corroboration.map(str::to_string);
                    }
                } else {
                    read = WindowRead::Complete;
                }
            } else if let Some(evidence) = accumulated.as_mut() {
                evidence.state = "partial";
                evidence.reason_code = reason.map(str::to_string);
                // The adapter has no source-side cardinality for nonempty
                // windows. A later catalogue reread cannot change the fixed
                // `Partial` state into a complete observation, so avoid the
                // extra endpoint load and fail with the evidence already in
                // hand. A future qualified nonempty path can continue to the
                // paired-snapshot checks below.
                return Err(PresenceError::WindowIncomplete
                    .safe_reason_code()
                    .to_string()
                    .into());
            }

            // The verdict is built from two independently timed observations,
            // so the catalogue must still be the one the parties bound
            // against. A ledger renamed between the reads would otherwise let
            // a proposal bind an old name while the rows carry the new one,
            // removing the only resemblance and manufacturing an `absent`.
            // Same paired-snapshot rule the selected-voucher read applies.
            let (corroboration, corroboration_evidence) =
                self.read_ledger_catalogue(&identity, &company.name).await?;
            accumulate(&mut accumulated, corroboration_evidence);
            let before = catalogue
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let after = corroboration
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            if before.len() != catalogue.len()
                || after.len() != corroboration.len()
                || before != after
            {
                return Err("ledger_snapshot_drifted".to_string().into());
            }

            // The qualified `vouchers` profile does not FETCH REMOTEID, so an
            // absent value here means "never read", not "the voucher has
            // none". Declaring that keeps a proposal whose own REMOTEID was
            // never compared out of `absent`.
            let window = book_window(&from, &to, read, &rows).map_err(presence_code)?;
            let request = PresenceRequest::new(&window, &catalog, &numbering, &proposals)
                .map_err(presence_code)?;
            let report = book_presence::assess(&request);
            let (result, truncated) = presence_result(
                &report,
                &catalogue,
                reason,
                offset,
                limit,
                self.settings.max_bytes,
            );

            Ok(ToolOutcome {
                payload: json!({
                    "company": company_json(&company, std::slice::from_ref(&company)),
                    "result": result,
                }),
                evidence: accumulated
                    .clone()
                    .expect("presence evidence is present after admitted reads"),
                company_guid: Some(guid.to_string()),
                truncated,
            })
        }
        .await;
        outcome.map_err(|failure: ToolFailure| match accumulated {
            Some(evidence) => failure.with_prior_evidence(evidence),
            None => failure,
        })
    }
}

fn accumulate(target: &mut Option<Evidence>, next: Evidence) {
    *target = Some(match target.take() {
        Some(current) => combine_evidence(current, next),
        None => next,
    });
}

fn presence_code(error: PresenceError) -> ToolFailure {
    error.safe_reason_code().to_string().into()
}

/// Turns one validated voucher row from the qualified window read into an
/// observed book voucher. `REMOTEID` is deliberately not read here: the
/// `vouchers` profile does not fetch it, and inventing an absent column would
/// be worse than reporting that it was never observed.
fn book_window(
    from: &str,
    to: &str,
    read: WindowRead,
    rows: &[Value],
) -> Result<BookWindow, PresenceError> {
    let mut budget = RawObservationBudget::default();
    let mut entries = Vec::with_capacity(rows.len().min(book_presence::MAX_WINDOW_VOUCHERS));
    let mut ambiguous = Vec::with_capacity(rows.len().min(book_presence::MAX_WINDOW_VOUCHERS));
    for row in rows {
        let raw = row["amounts"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let narration = row["narration"].as_str();
        match marker_kind(narration) {
            MarkerKind::Absent => budget.admit_fields(
                row["guid"].as_str().unwrap_or_default(),
                row["date"].as_str().unwrap_or_default(),
                row["voucher_type"].as_str().unwrap_or_default(),
                row["voucher_number"].as_str(),
                None,
                row["party"].as_str(),
                None,
                std::iter::empty(),
                raw.iter().map(|entry| {
                    (
                        entry["ledger"].as_str().unwrap_or_default(),
                        entry["amount"].as_str().unwrap_or_default(),
                    )
                }),
            )?,
            MarkerKind::Identifying(marker) => budget.admit_fields(
                row["guid"].as_str().unwrap_or_default(),
                row["date"].as_str().unwrap_or_default(),
                row["voucher_type"].as_str().unwrap_or_default(),
                row["voucher_number"].as_str(),
                None,
                row["party"].as_str(),
                Some(marker),
                std::iter::empty(),
                raw.iter().map(|entry| {
                    (
                        entry["ledger"].as_str().unwrap_or_default(),
                        entry["amount"].as_str().unwrap_or_default(),
                    )
                }),
            )?,
            MarkerKind::Unidentified => budget.admit_fields(
                row["guid"].as_str().unwrap_or_default(),
                row["date"].as_str().unwrap_or_default(),
                row["voucher_type"].as_str().unwrap_or_default(),
                row["voucher_number"].as_str(),
                None,
                row["party"].as_str(),
                None,
                ambiguous_markers_iter(narration),
                raw.iter().map(|entry| {
                    (
                        entry["ledger"].as_str().unwrap_or_default(),
                        entry["amount"].as_str().unwrap_or_default(),
                    )
                }),
            )?,
        }
        ambiguous.push(ambiguous_markers(narration));
        entries.push(
            row["amounts"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .map(|entry| ObservedEntry {
                    ledger: entry["ledger"].as_str().unwrap_or_default(),
                    amount: entry["amount"].as_str().unwrap_or_default(),
                })
                .collect::<Vec<_>>(),
        );
    }
    let observations =
        rows.iter()
            .zip(&entries)
            .zip(&ambiguous)
            .map(|((row, entries), ambiguous)| ObservedVoucher {
                // The GUID is the identity the window read already proved belongs to
                // this company, and the same field this tool's sibling already emits.
                key: row["guid"].as_str().unwrap_or_default(),
                date: row["date"].as_str().unwrap_or_default(),
                voucher_type: row["voucher_type"].as_str().unwrap_or_default(),
                voucher_number: row["voucher_number"].as_str(),
                remote_id: None,
                party: row["party"].as_str(),
                marker: match observed_marker(row["narration"].as_str()) {
                    ObservedMarker::Unidentified(_) => ObservedMarker::Unidentified(ambiguous),
                    settled => settled,
                },
                entries,
                cancelled: row["cancelled"].as_bool().unwrap_or_default(),
                optional: row["optional"].as_bool().unwrap_or_default(),
            });
    BookWindow::from_observations(ObservedWindow {
        from,
        to,
        read,
        remote_id_evidence: ColumnEvidence::NotRead,
        narration_evidence: ColumnEvidence::Observed,
        vouchers: observations,
    })
}

/// Applies the `[BRIDGE:...]` convention to one observed narration.
///
/// The convention belongs to the writer, so it is read here and the core crate
/// receives an opaque string. Two conditions must hold before a marker names
/// an import, and they fail closed for different reasons (ADR 0018 §3):
///
/// - **Exactly one occurrence.** Two mean the voucher claims two imports,
///   which is the middle case this contract never resolves; `verify_import`
///   already treats it as an error rather than taking the first.
/// - **The canonical form this writer produces.** A marker is
///   `import_identity`'s UUID over a random batch id. An older scheme wrote the
///   caller's transaction label instead, and those are, in this module's own
///   words, commonly reused -- matching one would pair a proposal with an
///   unrelated voucher from an unrelated batch and drop an invoice silently.
fn observed_marker(narration: Option<&str>) -> ObservedMarker<'_> {
    match marker_kind(narration) {
        MarkerKind::Absent => ObservedMarker::Absent,
        MarkerKind::Identifying(identity) => ObservedMarker::Identifying(identity),
        MarkerKind::Unidentified => ObservedMarker::Unidentified(&[]),
    }
}

#[derive(Clone, Copy)]
enum MarkerKind<'a> {
    Absent,
    Identifying(&'a str),
    Unidentified,
}

fn marker_kind(narration: Option<&str>) -> MarkerKind<'_> {
    let Some(narration) = narration else {
        return MarkerKind::Absent;
    };
    let mut found = agent_import::narration_markers(narration);
    match found.next() {
        None => MarkerKind::Absent,
        Some(Some(identity)) if is_batch_derived(identity) && found.next().is_none() => {
            MarkerKind::Identifying(identity)
        }
        _ => MarkerKind::Unidentified,
    }
}

/// The well-formed occurrences in a narration that could not identify one
/// import. They cannot decide, and they must not be thrown away: a proposal
/// whose own marker is among them is asking about this exact voucher.
fn ambiguous_markers(narration: Option<&str>) -> Vec<&str> {
    ambiguous_markers_iter(narration).collect()
}

fn ambiguous_markers_iter(narration: Option<&str>) -> impl Iterator<Item = &str> {
    narration.into_iter().flat_map(|narration| {
        agent_import::narration_markers(narration)
            .flatten()
            .filter(|identity| is_batch_derived(identity))
    })
}

/// Whether a marker has the exact shape `import_identity` writes.
///
/// Parsing alone is not enough, and neither is the canonical spelling. The
/// writer builds its identity with `Uuid::Builder::from_custom_bytes`, which
/// stamps **version 8** and the RFC 4122 variant into the bytes it is given,
/// so a value that carries any other version cannot have come from it.
///
/// That matters because a caller's transaction label may legally be
/// UUID-shaped: `valid_txn_id` admits hex and hyphens, so a legacy-scheme
/// write could put a canonical v4 UUID in a narration and this would have
/// called it batch-derived. Checking the version rejects that whole class
/// rather than the fraction of it that happens to look wrong.
fn is_batch_derived(identity: &str) -> bool {
    uuid::Uuid::parse_str(identity).is_ok_and(|parsed| {
        parsed.to_string() == identity
            && parsed.get_version() == Some(uuid::Version::Custom)
            && parsed.get_variant() == uuid::Variant::RFC4122
    })
}

fn parse_numbering(args: &Value) -> Result<NumberingDeclaration, String> {
    let declared = args
        .get("numbering")
        .and_then(Value::as_array)
        .ok_or_else(|| "numbering_required".to_string())?;
    let entries = declared
        .iter()
        .map(|entry| {
            let voucher_type = entry["voucher_type"]
                .as_str()
                .ok_or_else(|| "argument_invalid:numbering".to_string())?
                .to_string();
            let method = match entry["numbering_method"].as_str() {
                Some("manual") => NumberingMethod::Manual,
                Some("automatic") => NumberingMethod::Automatic,
                // The schema admits exactly these three, so anything else was
                // already refused above.
                _ => NumberingMethod::Unknown,
            };
            Ok((voucher_type, method))
        })
        .collect::<Result<Vec<_>, String>>()?;
    NumberingDeclaration::new(entries).map_err(|error| error.safe_reason_code().to_string())
}

fn parse_proposals(
    args: &Value,
) -> Result<bridge_tally_core::book_presence::ProposedBatch, String> {
    let proposed = args
        .get("vouchers")
        .and_then(Value::as_array)
        .ok_or_else(|| "vouchers_required".to_string())?;
    let invalid = || "argument_invalid:vouchers".to_string();
    struct RawProposal {
        date: String,
        voucher_type: String,
        voucher_number: Option<String>,
        party: Option<String>,
        entries: Vec<(String, String)>,
        marker: Option<String>,
    }
    let mut raw = Vec::with_capacity(proposed.len().min(book_presence::MAX_PROPOSED_VOUCHERS));
    let mut admission = bridge_tally_core::book_presence::RawProposalBudget::default();
    for (position, voucher) in proposed.iter().enumerate() {
        let raw_date = voucher["date"].as_str().ok_or_else(invalid)?;
        let raw_type = voucher["voucher_type"].as_str().ok_or_else(invalid)?;
        // Both or neither. Supplying one alone is a caller error, and silently
        // ignoring it would skip the strongest key this proposal has.
        let marker = match (
            voucher["batch_id"].as_str(),
            voucher["bridge_txn_id"].as_str(),
        ) {
            (Some(batch_id), Some(txn_id)) => {
                // The published pattern is documentation: the shared validator
                // enforces `minLength`, `maxLength` and the one `\S` special
                // case, and evaluates no other regular expression. So the
                // character rule is enforced here, with the writer's own
                // function -- a label `build_import_xml` would have refused
                // cannot have produced a marker, and hashing it anyway derives
                // an identity no book can hold and calls the result `absent`.
                if !agent_import::valid_txn_id(txn_id) {
                    return Err("argument_invalid:bridge_txn_id".to_string());
                }
                // Same rule, other half of the pair. A mistyped batch id is
                // not a harmless miss: under automatic numbering, with nothing
                // resembling the proposal, the derived-but-impossible marker
                // matches nothing and the window reports `absent` -- which
                // invites the duplicate import this contract exists to stop.
                // Bad input should say it is bad input.
                if !agent_import::valid_batch_id(batch_id) {
                    return Err("argument_invalid:batch_id".to_string());
                }
                Some(agent_import::import_identity(batch_id, txn_id).to_string())
            }
            (None, None) => None,
            _ => return Err("presence_import_identity_incomplete".to_string()),
        };
        let rows = voucher["entries"].as_array().ok_or_else(invalid)?;
        // Admit the complete borrowed shape before date/decimal parsing or
        // cloning any proposal metadata. The shared core repeats this check
        // for callers that do not use the JSON adapter.
        admission
            .admit_parts(
                position,
                raw_date,
                raw_type,
                voucher["voucher_number"].as_str(),
                marker.as_deref(),
                None,
                voucher["party"].as_str(),
                rows.iter().map(|entry| {
                    (
                        entry["ledger"].as_str().unwrap_or_default(),
                        entry["amount"].as_str().unwrap_or_default(),
                    )
                }),
            )
            .map_err(|error| error.safe_reason_code().to_string())?;
        let date = normalized_date(raw_date)?;
        let entries = rows
            .iter()
            .map(|entry| {
                Ok((
                    entry["ledger"].as_str().ok_or_else(invalid)?.to_string(),
                    entry["amount"].as_str().ok_or_else(invalid)?.to_string(),
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        raw.push(RawProposal {
            date,
            voucher_type: voucher["voucher_type"]
                .as_str()
                .ok_or_else(invalid)?
                .to_string(),
            voucher_number: voucher["voucher_number"].as_str().map(str::to_string),
            party: voucher["party"].as_str().map(str::to_string),
            entries,
            marker,
        });
    }
    // Materialize entry descriptors so their borrowed slices outlive the
    // batch conversion; admission still precedes decimal parsing and clones.
    let descriptors: Vec<Vec<ObservedEntry<'_>>> = raw
        .iter()
        .map(|voucher| {
            voucher
                .entries
                .iter()
                .map(|(ledger, amount)| ObservedEntry { ledger, amount })
                .collect()
        })
        .collect();
    let inputs = raw
        .iter()
        .enumerate()
        .map(|(position, voucher)| ProposedVoucherInput {
            position,
            date: &voucher.date,
            voucher_type: &voucher.voucher_type,
            voucher_number: voucher.voucher_number.as_deref(),
            remote_id: None,
            narration_marker: voucher.marker.as_deref(),
            party: voucher.party.as_deref(),
            entries: &descriptors[position],
        });
    ProposedVoucher::from_inputs(inputs).map_err(|error| error.safe_reason_code().to_string())
}

/// Bounds the fixed observations, so a diagnostic can never cost the answer.
///
/// The counts are what a person acts on; the listed keys are a convenience for
/// finding the rows again. When the listing will not fit, the listing goes and
/// every count stays -- and the report says so, because a list that is shorter
/// than it claims is the defect this contract keeps finding elsewhere.
fn bounded_observations(mut book: Value, budget: usize) -> Value {
    // Drop one listed row at a time rather than the whole listing. Twenty of
    // twenty-five duplicate groups is worth more to the person reading this
    // than none of them, and the counts beside them stay exact either way.
    let mut withheld = false;
    while book.to_string().len() > budget {
        let dropped = book["duplicate_numbers"]
            .as_array_mut()
            .and_then(Vec::pop)
            .inspect(|_| book["duplicate_numbers_truncated"] = json!(true))
            .or_else(|| {
                book["unbalanced_vouchers"]
                    .as_array_mut()
                    .and_then(Vec::pop)
            });
        if dropped.is_none() {
            // Only counts and flags are left; they are the part a reader
            // reconciles against, so they are never dropped.
            break;
        }
        withheld = true;
    }
    if withheld {
        book["listings_withheld_for_size"] = json!(true);
    }
    book
}

fn presence_result(
    report: &PresenceReport,
    catalogue: &[String],
    corroboration_reason: Option<&'static str>,
    offset: usize,
    limit: usize,
    max_bytes: usize,
) -> (Value, bool) {
    let (from, to) = report.window();
    let total = report.vouchers().len();
    // Paged like every other read in this adapter, for one reason beyond
    // consistency: this result shape is otherwise invisible to `page_shape`,
    // so an over-large report would be discarded wholesale *after* all three
    // Tally reads were paid for. An `items` array with an `offset` is the
    // shape the response machinery can trim with a resumable cursor.
    let items = report
        .vouchers()
        .iter()
        .skip(offset)
        .take(limit)
        .map(|entry| mark_presence_party_names(serde_json::to_value(entry).unwrap_or_default()))
        .collect::<Vec<_>>();
    let next_offset = offset.saturating_add(items.len());
    let truncated = next_offset < total;
    let mut result = json!({
        "profile": "agent_voucher_presence_v1",
        // Every verdict is relative to this window. `absent` means absent from
        // this range and never absent from the book.
        "window": {"from": from, "to": to, "read": "complete", "reason": corroboration_reason},
        "items": items,
        "offset": offset,
        "total": total,
        "totals": report.totals(),
        "book": bounded_observations(
            serde_json::to_value(report.observations()).unwrap_or_default(),
            max_bytes / OBSERVATION_BUDGET_DIVISOR,
        ),
        "catalogue_evidence_sha256": sha256_json(&catalogue.to_vec()),
    });
    if truncated {
        result["next_offset"] = json!(next_offset);
    }
    (result, truncated)
}

/// Marks the names an egress policy treats as party data. Voucher numbers and
/// dates are accounting selectors the sibling voucher read already emits
/// unmarked; the names are not.
pub(super) fn mark_presence_party_names(mut entry: Value) -> Value {
    if let Some(party) = entry.get_mut("party") {
        mark_party_field(party, "catalog_name");
    }
    if let Some(differences) = entry.get_mut("differences").and_then(Value::as_array_mut) {
        for difference in differences {
            if difference.get("field").and_then(Value::as_str) != Some("party") {
                continue;
            }
            for side in ["proposed", "observed"] {
                mark_party_field(difference, side);
            }
        }
    }
    entry
}

#[cfg(test)]
#[path = "agent_presence_tests.rs"]
mod tests;
