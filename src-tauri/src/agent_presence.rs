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
    ObservedVoucher, PresenceError, PresenceReport, PresenceRequest, ProposedVoucher,
    ProposedVoucherInput, RemoteIdEvidence, WindowRead,
};
use bridge_tally_core::master_binding::{MasterCatalog, MasterClass};

/// Most vouchers one presence request may propose. The window read is
/// unaffected by this: it always reads its whole range.
pub(super) const MAX_PRESENCE_VOUCHERS: usize = 500;
/// Most voucher types one numbering declaration may name.
pub(super) const MAX_PRESENCE_VOUCHER_TYPES: usize = 50;
/// Most ledger entries one proposed voucher may carry.
pub(super) const MAX_PRESENCE_ENTRIES: usize = 200;
/// Longest accepted amount lexeme, matching the published schema.
const MAX_PRESENCE_AMOUNT_CHARS: usize = 64;

/// The shared argument validator bounds only the outer arrays, and the core
/// crate's own limits are far wider than what this tool advertises. So every
/// nested string is bounded here against the published `inputSchema`, and an
/// unknown nested property is refused rather than ignored — a schema that
/// promises `additionalProperties: false` and then accepts them is a claim the
/// boundary does not keep.
fn nested_text(
    object: &Value,
    key: &str,
    argument: &str,
    max_chars: usize,
) -> Result<Option<String>, String> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    let text = value
        .as_str()
        .ok_or_else(|| format!("argument_invalid:{argument}"))?;
    if text.trim().is_empty() || text.chars().count() > max_chars {
        return Err(format!("argument_invalid:{argument}"));
    }
    Ok(Some(text.to_string()))
}

fn only_known_keys(object: &Value, known: &[&str], argument: &str) -> Result<(), String> {
    let map = object
        .as_object()
        .ok_or_else(|| format!("argument_invalid:{argument}"))?;
    if map.keys().any(|key| !known.contains(&key.as_str())) {
        return Err(format!("argument_invalid:{argument}"));
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
        let numbering = parse_numbering(args)?;
        let proposals = parse_proposals(args)?;
        // Both remaining cross-input refusals depend only on the arguments, so
        // they are settled here rather than after three Tally reads. The crate
        // enforces them again at its own boundary; this only stops a request
        // that was always going to be refused from exercising the endpoint.
        for proposal in &proposals {
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

            let observed = rows
                .iter()
                .map(book_voucher)
                .collect::<Result<Vec<_>, _>>()
                .map_err(presence_code)?;
            // The qualified `vouchers` profile does not FETCH REMOTEID, so an
            // absent value here means "never read", not "the voucher has
            // none". Declaring that keeps a proposal whose own REMOTEID was
            // never compared out of `absent`.
            let window =
                BookWindow::observed(&from, &to, read, RemoteIdEvidence::NotRead, observed)
                    .map_err(presence_code)?;
            let request = PresenceRequest::new(&window, &catalog, &numbering, &proposals)
                .map_err(presence_code)?;
            let report = book_presence::assess(&request);

            Ok(ToolOutcome {
                payload: json!({
                    "company": company_json(&company, std::slice::from_ref(&company)),
                    "result": presence_result(&report, &catalogue, reason),
                }),
                evidence: accumulated
                    .clone()
                    .expect("presence evidence is present after admitted reads"),
                company_guid: Some(guid.to_string()),
                truncated: false,
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
        entries: &entries,
        cancelled: row["cancelled"].as_bool().unwrap_or_default(),
        optional: row["optional"].as_bool().unwrap_or_default(),
    })
}

fn parse_numbering(args: &Value) -> Result<NumberingDeclaration, String> {
    let declared = args
        .get("numbering")
        .and_then(Value::as_array)
        .ok_or_else(|| "numbering_required".to_string())?;
    if declared.is_empty() || declared.len() > MAX_PRESENCE_VOUCHER_TYPES {
        return Err("argument_invalid:numbering".to_string());
    }
    let entries = declared
        .iter()
        .map(|entry| {
            only_known_keys(entry, &["voucher_type", "numbering_method"], "numbering")?;
            let voucher_type = nested_text(
                entry,
                "voucher_type",
                "numbering",
                agent_import::MAX_MASTER_NAME_CHARS,
            )?
            .ok_or_else(|| "argument_invalid:numbering".to_string())?;
            let method = match entry.get("numbering_method").and_then(Value::as_str) {
                Some("manual") => NumberingMethod::Manual,
                Some("automatic") => NumberingMethod::Automatic,
                Some("unknown") => NumberingMethod::Unknown,
                _ => return Err("argument_invalid:numbering".to_string()),
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
    if proposed.is_empty() || proposed.len() > MAX_PRESENCE_VOUCHERS {
        return Err("argument_invalid:vouchers".to_string());
    }
    let mut parsed = Vec::with_capacity(proposed.len());
    for (position, voucher) in proposed.iter().enumerate() {
        only_known_keys(
            voucher,
            &[
                "date",
                "voucher_type",
                "voucher_number",
                "remote_id",
                "party",
                "entries",
            ],
            "vouchers",
        )?;
        let date = normalized_date(
            voucher
                .get("date")
                .and_then(Value::as_str)
                .ok_or_else(|| "argument_invalid:vouchers".to_string())?,
        )?;
        let name_limit = agent_import::MAX_MASTER_NAME_CHARS;
        let voucher_type = nested_text(voucher, "voucher_type", "vouchers", name_limit)?
            .ok_or_else(|| "argument_invalid:vouchers".to_string())?;
        let voucher_number = nested_text(voucher, "voucher_number", "vouchers", name_limit)?;
        let remote_id = nested_text(voucher, "remote_id", "vouchers", name_limit)?;
        let party = nested_text(voucher, "party", "vouchers", name_limit)?;
        let rows = voucher
            .get("entries")
            .and_then(Value::as_array)
            .filter(|entries| !entries.is_empty() && entries.len() <= MAX_PRESENCE_ENTRIES)
            .ok_or_else(|| "argument_invalid:vouchers".to_string())?;
        let bounded = rows
            .iter()
            .map(|entry| {
                only_known_keys(entry, &["ledger", "amount"], "vouchers")?;
                let ledger = nested_text(entry, "ledger", "vouchers", name_limit)?
                    .ok_or_else(|| "argument_invalid:vouchers".to_string())?;
                let amount = nested_text(entry, "amount", "vouchers", MAX_PRESENCE_AMOUNT_CHARS)?
                    .ok_or_else(|| "argument_invalid:vouchers".to_string())?;
                Ok((ledger, amount))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let entries = bounded
            .iter()
            .map(|(ledger, amount)| ObservedEntry {
                ledger: ledger.as_str(),
                amount: amount.as_str(),
            })
            .collect::<Vec<_>>();
        parsed.push(
            ProposedVoucher::new(ProposedVoucherInput {
                position,
                date: &date,
                voucher_type: &voucher_type,
                voucher_number: voucher_number.as_deref(),
                remote_id: remote_id.as_deref(),
                party: party.as_deref(),
                entries: &entries,
            })
            .map_err(|error| error.safe_reason_code().to_string())?,
        );
    }
    Ok(parsed)
}

fn presence_result(
    report: &PresenceReport,
    catalogue: &[String],
    corroboration_reason: Option<&'static str>,
) -> Value {
    let (from, to) = report.window();
    let vouchers = report
        .vouchers()
        .iter()
        .map(|entry| mark_presence_party_names(serde_json::to_value(entry).unwrap_or_default()))
        .collect::<Vec<_>>();
    json!({
        "profile": "agent_voucher_presence_v1",
        // Every verdict is relative to this window. `absent` means absent from
        // this range and never absent from the book.
        "window": {"from": from, "to": to, "read": "complete", "reason": corroboration_reason},
        "vouchers": vouchers,
        "totals": report.totals(),
        "book": report.observations(),
        "catalogue_evidence_sha256": sha256_json(&catalogue.to_vec()),
    })
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
