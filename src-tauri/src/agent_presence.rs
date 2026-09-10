//! "Which of these are already in the book?" for the local MCP adapter.
//!
//! The rules live in `bridge_tally_core::book_presence` so this tool and any
//! later desktop screen cannot drift apart; see
//! `docs/adr/0017-voucher-presence-authority.md`. This file owns only the two
//! qualified reads that produce the evidence, the typed parse of the caller's
//! proposals, and the response shape.
use super::*;

use bridge_tally_core::book_presence::{
    self, BookVoucher, BookWindow, NumberingDeclaration, NumberingMethod, ObservedEntry,
    ObservedVoucher, PresenceError, PresenceReport, PresenceRequest, ProposedVoucher,
    ProposedVoucherInput, WindowRead,
};
use bridge_tally_core::master_binding::{MasterCatalog, MasterClass};

/// Most vouchers one presence request may propose. The window read is
/// unaffected by this: it always reads its whole range.
pub(super) const MAX_PRESENCE_VOUCHERS: usize = 500;
/// Most voucher types one numbering declaration may name.
pub(super) const MAX_PRESENCE_VOUCHER_TYPES: usize = 50;
/// Most ledger entries one proposed voucher may carry.
pub(super) const MAX_PRESENCE_ENTRIES: usize = 200;

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

            let observed = rows
                .iter()
                .map(book_voucher)
                .collect::<Result<Vec<_>, _>>()
                .map_err(presence_code)?;
            let window = BookWindow::observed(&from, &to, read, observed).map_err(presence_code)?;
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
            let voucher_type = entry
                .get("voucher_type")
                .and_then(Value::as_str)
                .ok_or_else(|| "argument_invalid:numbering".to_string())?;
            let method = match entry.get("numbering_method").and_then(Value::as_str) {
                Some("manual") => NumberingMethod::Manual,
                Some("automatic") => NumberingMethod::Automatic,
                Some("unknown") => NumberingMethod::Unknown,
                _ => return Err("argument_invalid:numbering".to_string()),
            };
            Ok((voucher_type.to_string(), method))
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
        let date = normalized_date(
            voucher
                .get("date")
                .and_then(Value::as_str)
                .ok_or_else(|| "argument_invalid:vouchers".to_string())?,
        )?;
        let voucher_type = voucher
            .get("voucher_type")
            .and_then(Value::as_str)
            .ok_or_else(|| "argument_invalid:vouchers".to_string())?;
        let rows = voucher
            .get("entries")
            .and_then(Value::as_array)
            .filter(|entries| !entries.is_empty() && entries.len() <= MAX_PRESENCE_ENTRIES)
            .ok_or_else(|| "argument_invalid:vouchers".to_string())?;
        let entries = rows
            .iter()
            .map(|entry| {
                Ok(ObservedEntry {
                    ledger: entry
                        .get("ledger")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "argument_invalid:vouchers".to_string())?,
                    amount: entry
                        .get("amount")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "argument_invalid:vouchers".to_string())?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        parsed.push(
            ProposedVoucher::new(ProposedVoucherInput {
                position,
                date: &date,
                voucher_type,
                voucher_number: voucher.get("voucher_number").and_then(Value::as_str),
                remote_id: voucher.get("remote_id").and_then(Value::as_str),
                party: voucher.get("party").and_then(Value::as_str),
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
