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
    self, BookVoucher, BookWindow, NumberingDeclaration, NumberingMethod, ObservedEntry,
    ObservedVoucher, ObservedWindow, PresenceError, PresenceReport, PresenceRequest,
    ProposedVoucher, ProposedVoucherInput, WindowRead,
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
        for proposal in &proposals {
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

            // An empty window is only an empty window once the existing
            // corroboration says so. Anything less becomes `WindowIncomplete`
            // at the crate boundary rather than a report full of "absent".
            let mut read = WindowRead::Complete;
            let mut reason = None;
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
                }
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

            // The window is independent evidence about which ledgers exist,
            // and it is already in hand. A ledger the book posts to but the
            // catalogue never listed proves the catalogue short -- both reads
            // agreeing only proves they agree. Left unchecked, a proposal
            // naming that ledger binds `Unmatched`, every party rule declines
            // to run, and an `Absent` is authorised off a comparison that was
            // never possible. That is the failure this whole contract exists
            // to prevent, so it fails closed here rather than being reported.
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

            let observed = rows
                .iter()
                .map(book_voucher)
                .collect::<Result<Vec<_>, _>>()
                .map_err(presence_code)?;
            // The qualified `vouchers` profile does not FETCH REMOTEID, so an
            // absent value here means "never read", not "the voucher has
            // none". Declaring that keeps a proposal whose own REMOTEID was
            // never compared out of `absent`.
            let window = BookWindow::observed(ObservedWindow {
                from: &from,
                to: &to,
                read,
                // The qualified `vouchers` profile does not FETCH REMOTEID.
                remote_id_evidence: ColumnEvidence::NotRead,
                // It does FETCH NARRATION, which is what makes the marker
                // basis reachable with no change to a qualified read.
                narration_evidence: ColumnEvidence::Observed,
                vouchers: observed,
            })
            .map_err(presence_code)?;
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
fn book_voucher(row: &Value) -> Result<BookVoucher, PresenceError> {
    let entries = row["amounts"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|entry| ObservedEntry {
            ledger: entry["ledger"].as_str().unwrap_or_default(),
            amount: entry["amount"].as_str().unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    BookVoucher::observed(ObservedVoucher {
        // The GUID is the identity the window read already proved belongs to
        // this company, and the same field this tool's sibling already emits.
        key: row["guid"].as_str().unwrap_or_default(),
        date: row["date"].as_str().unwrap_or_default(),
        voucher_type: row["voucher_type"].as_str().unwrap_or_default(),
        voucher_number: row["voucher_number"].as_str(),
        remote_id: None,
        party: row["party"].as_str(),
        marker: observed_marker(row["narration"].as_str()),
        entries: &entries,
        cancelled: row["cancelled"].as_bool().unwrap_or_default(),
        optional: row["optional"].as_bool().unwrap_or_default(),
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
    let Some(narration) = narration else {
        return ObservedMarker::Absent;
    };
    let mut found = agent_import::narration_markers(narration);
    match (found.next(), found.next()) {
        (None, _) => ObservedMarker::Absent,
        (Some(Some(identity)), None) if is_batch_derived(identity) => {
            ObservedMarker::Identifying(identity)
        }
        _ => ObservedMarker::Unidentified,
    }
}

/// Whether a marker has the exact shape `import_identity` writes. Parsing
/// alone is not enough: `Uuid` accepts several spellings, and only the one the
/// writer emits can have come from a batch-derived identity.
fn is_batch_derived(identity: &str) -> bool {
    uuid::Uuid::parse_str(identity).is_ok_and(|parsed| parsed.to_string() == identity)
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

fn parse_proposals(args: &Value) -> Result<Vec<ProposedVoucher>, String> {
    let proposed = args
        .get("vouchers")
        .and_then(Value::as_array)
        .ok_or_else(|| "vouchers_required".to_string())?;
    let invalid = || "argument_invalid:vouchers".to_string();
    let mut parsed = Vec::with_capacity(proposed.len());
    for (position, voucher) in proposed.iter().enumerate() {
        let date = normalized_date(voucher["date"].as_str().ok_or_else(invalid)?)?;
        // Both or neither. Supplying one alone is a caller error, and silently
        // ignoring it would skip the strongest key this proposal has.
        let marker = match (
            voucher["batch_id"].as_str(),
            voucher["bridge_txn_id"].as_str(),
        ) {
            (Some(batch_id), Some(txn_id)) => {
                Some(agent_import::import_identity(batch_id, txn_id).to_string())
            }
            (None, None) => None,
            _ => return Err("presence_import_identity_incomplete".to_string()),
        };
        let rows = voucher["entries"].as_array().ok_or_else(invalid)?;
        let entries = rows
            .iter()
            .map(|entry| {
                Ok(ObservedEntry {
                    ledger: entry["ledger"].as_str().ok_or_else(invalid)?,
                    amount: entry["amount"].as_str().ok_or_else(invalid)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        parsed.push(
            ProposedVoucher::new(ProposedVoucherInput {
                position,
                date: &date,
                voucher_type: voucher["voucher_type"].as_str().ok_or_else(invalid)?,
                voucher_number: voucher["voucher_number"].as_str(),
                // Not an accepted input: the shipped read cannot fetch
                // REMOTEID, so a supplied one could only ever withhold a
                // verdict. The crate keeps the basis for callers that can.
                remote_id: None,
                party: voucher["party"].as_str(),
                // Derived, never accepted: a caller handing over a marker
                // string could name a legacy label and match an unrelated
                // import. See ADR 0018 §1 -- the input shape is the safety
                // argument, not a validation rule someone has to remember.
                narration_marker: marker.as_deref(),
                entries: &entries,
            })
            .map_err(|error| error.safe_reason_code().to_string())?,
        );
    }
    Ok(parsed)
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
    let truncated = offset.saturating_add(items.len()) < total;
    let result = json!({
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
