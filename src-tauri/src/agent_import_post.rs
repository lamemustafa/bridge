//! One user-approved Journal attempt; subsequent calls only reconcile its identity.
use super::*;
use crate::agent::evidence_from_runtime_read;
use crate::tally::approved_import::ApprovedImport;
use bridge_tally_protocol::{parse_import_outcome, TallyImportApplicationStatus};

impl Server {
    /// Called only after dropping a cancelled posting future. A missing or
    /// unreadable snapshot is unknown, never proof that dispatch did not occur.
    pub(in crate::agent) fn cancelled_import(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        let batch_id = required_string(args, "batch_id")?;
        let guid = required_string(args, "company_guid")?;
        let uuid = batch_id
            .strip_prefix("bridge-")
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .ok_or_else(|| "import_batch_identifier_invalid".to_string())?;
        if batch_id != format!("bridge-{uuid}") {
            return Err("import_batch_identifier_invalid".to_string().into());
        }
        let attempted = self
            .latest_import_snapshot(batch_id)
            .ok()
            .flatten()
            .filter(|snapshot| batch_guid_matches(&snapshot.batch.company_guid, guid))
            .map(|snapshot| snapshot.dispatched);
        let mut evidence =
            evidence_from_runtime_read(crate::tally::runtime::RuntimeReadEvidence::empty());
        evidence.state = "partial";
        evidence.reason_code = Some("request_cancelled".into());
        Ok(ToolOutcome {
            payload: json!({"result":{
                "batch_id":batch_id,"attempt_recorded":attempted,
                "dispatch":{"state":if attempted == Some(false) { "not_dispatched" } else { "reconciliation_required" },"resent":false},
                "error":{"code":"request_cancelled","message":if attempted == Some(false) {
                    "The request was cancelled. No posting attempt is recorded for this saved batch."
                } else {
                    "The request was cancelled with an uncertain outcome. Reconcile this original batch with verify_import; never rebuild or resend it."
                }}
            }}),
            evidence,
            company_guid: Some(guid.to_string()),
            truncated: false,
        })
    }

    pub(in crate::agent) async fn post_import(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let batch_id = required_string(args, "batch_id")?;
        let snapshot = self
            .latest_import_snapshot(batch_id)?
            .ok_or_else(|| "import_batch_not_found".to_string())?;
        let line = snapshot.batch;
        if !batch_guid_matches(&line.company_guid, guid) {
            return Err("import_batch_company_mismatch".to_string().into());
        }
        let mut accumulated =
            evidence_from_runtime_read(crate::tally::runtime::RuntimeReadEvidence::empty());
        let operation: Result<ToolOutcome, ToolFailure> = async {
            let (xml, preview) = admit_saved_journal(&line, &self.settings.endpoint)?;
            if snapshot.dispatched {
                let mut result = self.verify_import(args).await?;
                finalize_previous_attempt_reconciliation(&mut result.payload, snapshot.response.as_ref());
                return Ok(result);
            }
            let before = self.verify_import(args).await?;
            accumulated = combine_evidence(accumulated.clone(), before.evidence);
            require_absent(&before.payload)?;
            // No Tally mutation can occur while the separate approval dialog is open.
            let request = ApprovedImport::confirm(xml, &preview).await?;
            let mode = self.qualified_import_profile().await?;
            let payload = ImportPayload { company_guid: line.company_guid.clone(), vouchers: line.vouchers.clone() };
            validate_post_profile_with_evidence(&payload, &mode, &mut accumulated)?;
            let (company, identity, identity_evidence) = self.verified_company(guid).await?;
            accumulated = combine_evidence(accumulated.clone(), identity_evidence);
            if line.company.as_ref() != Some(&import_company_tuple(&company)?) {
                return Err("company_identity_mismatch".to_string().into());
            }
            validate_dates(&payload, company.books_from.as_deref())?;
            let (catalogue, evidence) = self.read_ledger_catalogue(&identity, &company.name).await?;
            accumulated = combine_evidence(accumulated.clone(), evidence);
            if masters_for_payload(&payload, &catalogue).iter().any(|item| item["match_state"] != "exact") {
                return Err("import_masters_changed".to_string().into());
            }
            let preflight = self.verify_import(args).await?;
            accumulated = combine_evidence(accumulated.clone(), preflight.evidence);
            require_absent(&preflight.payload)?;
            let posted = self.runtime.post_approved_import(self.tally_config(), &identity, request, || {
                // The file lock covers only the admission+synced append. It is not
                // held over approval or network I/O. A competing process loses here.
                let _lock = self.lock_import_admission()?;
                let current = self.import_snapshot_while_admitted(Some(batch_id))?
                    .ok_or_else(|| "import_batch_not_found".to_string())?;
                if current.dispatched { return Err("import_already_attempted".into()); }
                if current.batch.sha256 != line.sha256 || current.batch.endpoint_origin != line.endpoint_origin {
                    return Err("import_batch_changed".into());
                }
                self.append_import_record_while_admitted(&ledger::StatusRecord::dispatch(&line))
            }).await;
            let (body, wire) = posted.map_err(|error| {
                if error.downcast_ref::<crate::tally::approved_import::AmbiguousImportCompany>().is_some() {
                    "import_company_scope_ambiguous".to_string()
                } else { "import_dispatch_outcome_unknown".to_string() }
            })?;
            accumulated = combine_evidence(accumulated.clone(), evidence_from_runtime_read(wire.clone()));
            let parsed_outcome = parse_import_outcome(&body).ok();
            {
                let _lock = self.lock_import_admission()?;
                self.append_import_record_while_admitted(&ledger::StatusRecord::response(&line, ledger::DispatchResponse {
                    request_sha256:wire.request_sha256, response_sha256:wire.response_sha256, bytes:wire.bytes,
                    outcome:parsed_outcome.clone(),
                }))?;
            }
            // A valid counter response is evidence, never proof that Tally preserved
            // the requested ledger/amount/date semantics. Readback is mandatory.
            let clean = import_outcome_is_clean(parsed_outcome.as_ref());
            let mut proof = self.verify_import(args).await?;
            accumulated = combine_evidence(accumulated.clone(), proof.evidence.clone());
            let verified = verification_status(&proof.payload["result"], 1) == "posted_verified";
            proof.payload["result"]["dispatch"] = json!({
                "state": if clean && verified { "posted_verified" } else { "reconciliation_required" },
                "counters":parsed_outcome.as_ref().map(|outcome| outcome.counters()), "application_status":parsed_outcome.as_ref().map(|outcome| outcome.application_status()),
                "resent":false, "automatic_retry":false
            });
            if !clean || !verified {
                mark_reconciliation_required(&mut proof.payload);
            }
            proof.evidence = accumulated.clone();
            Ok(proof)
        }.await;
        // Even failure after a lost response carries the saved identity. The next
        // call must reconcile that batch, never create a replacement business event.
        match operation {
            Ok(result) => Ok(result),
            Err(failure) => {
                let attempted = self
                    .latest_import_snapshot(batch_id)
                    .ok()
                    .flatten()
                    .map(|s| s.dispatched);
                let mut evidence = failure
                    .evidence
                    .map(|item| combine_evidence(accumulated.clone(), *item))
                    .unwrap_or(accumulated);
                evidence.state = "partial";
                evidence.reason_code = Some(failure.code.clone());
                Ok(ToolOutcome {
                    payload: json!({"result":{"batch_id":batch_id,"attempt_recorded":attempted,"error":{"code":failure.code,
                        "message":if attempted == Some(false) { "No posting attempt was recorded. Review the error before requesting approval again." }
                        else { "The saved batch requires reconciliation. Use verify_import with this original batch; never rebuild it to retry." }}}}),
                    evidence,
                    company_guid: Some(guid.to_string()),
                    truncated: false,
                })
            }
        }
    }
}

fn mark_reconciliation_required(payload: &mut Value) {
    payload["result"]["error"] = json!({"code":"import_reconciliation_required", "message":"The saved attempt has not been confirmed as the intended new Journal. Reconcile this original batch without resending it."});
}

fn import_outcome_is_clean(outcome: Option<&bridge_tally_protocol::TallyImportOutcome>) -> bool {
    outcome.is_some_and(|outcome| {
        outcome.application_status() != TallyImportApplicationStatus::Failure
            && outcome.counters().is_clean_success_for(1, 0, 0)
    })
}

fn persisted_response_is_clean(response: Option<&ledger::DispatchResponse>) -> bool {
    response.is_some_and(|response| import_outcome_is_clean(response.outcome.as_ref()))
}

fn persisted_response_state(response: Option<&ledger::DispatchResponse>) -> &'static str {
    match response {
        None => "response_missing",
        Some(response) if import_outcome_is_clean(response.outcome.as_ref()) => "response_clean",
        Some(_) => "response_not_clean",
    }
}

fn finalize_previous_attempt_reconciliation(
    payload: &mut Value,
    response: Option<&ledger::DispatchResponse>,
) {
    let reconciled = verification_status(&payload["result"], 1) == "posted_verified"
        && persisted_response_is_clean(response);
    payload["result"]["dispatch"] = json!({
        "state": if reconciled { "previous_attempt_reconciled" } else { "reconciliation_required" },
        "resent": false,
        "response_state": persisted_response_state(response),
        "response": response,
    });
    if !reconciled {
        mark_reconciliation_required(payload);
    }
}

fn validate_post_profile_with_evidence(
    payload: &ImportPayload,
    profile: &ImportProfileObservation,
    accumulated: &mut Evidence,
) -> Result<(), String> {
    *accumulated = combine_evidence(accumulated.clone(), profile.evidence.clone());
    validate_import_dates_for_profile(payload, profile)
}

fn require_absent(payload: &Value) -> Result<(), String> {
    let result = &payload["result"];
    if result["counts"]["not_found"].as_u64() != Some(1)
        || result["vouchers"].as_array().map(Vec::len) != Some(1)
    {
        return Err("import_preexisting_identity".into());
    }
    Ok(())
}

fn admit_saved_journal(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
) -> Result<(String, String), String> {
    if line.vouchers.len() != 1
        || line.vouchers[0].voucher_type != VoucherType::Journal
        || line.identity_scheme != Some(ImportIdentityScheme::BatchV1)
    {
        return Err("import_post_requires_one_journal".into());
    }
    let origin =
        super::super::canonical_loopback_origin(endpoint).map_err(|_| "host_setting_invalid")?;
    if line.endpoint_origin.as_deref() != Some(origin.as_str()) {
        return Err("import_post_endpoint_mismatch".into());
    }
    let company = line.company.as_ref().ok_or("import_post_company_missing")?;
    let payload = ImportPayload {
        company_guid: line.company_guid.clone(),
        vouchers: line.vouchers.clone(),
    };
    validate_payload(&payload)?;
    let (debit, credit) = totals(&line.vouchers)?;
    let voucher = &line.vouchers[0];
    let mut review_text = std::iter::once(company.name.as_str())
        .chain(voucher.voucher_number.iter().map(String::as_str))
        .chain(voucher.reference.iter().map(String::as_str))
        .chain(voucher.narration.iter().map(String::as_str))
        .chain(voucher.entries.iter().map(|entry| entry.ledger.as_str()));
    if review_text.clone().any(has_unsafe_review_layout_character) {
        return Err("import_review_layout_text".into());
    }
    if review_text.any(has_directional_review_character) {
        return Err("import_review_directional_text".into());
    }
    let xml = render_import_xml(&company.name, &line.vouchers, &line.batch_id);
    if sha256_hex(xml.as_bytes()) != line.sha256 {
        return Err("import_batch_changed".into());
    }
    let quoted = |text: &str| serde_json::to_string(text).expect("string serialization");
    let optional = |value: &Option<String>| {
        value
            .as_deref()
            .map(quoted)
            .unwrap_or_else(|| "(none)".into())
    };
    let entries = voucher
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{} {}  {}",
                if entry.side == EntrySide::Dr {
                    "Dr"
                } else {
                    "Cr"
                },
                entry.amount,
                quoted(&entry.ledger)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let preview = format!("Create ONE Journal in {}\nCompany GUID: {}\nCompany number: {}  Books from: {}\nTally: {origin}\nDate: {}  Voucher number: {}\nReference: {}\nNarration: {}\n\n{}\n\nTotal debit: {}  Total credit: {}\nBatch: {}\n\nBridge adds its batch reference for readback.\nCheck every ledger, date and amount. This changes your accounts.\nAfter a timeout, reconcile this batch; do not rebuild or resend it.",
        quoted(&company.name), company.guid, company.company_number, company.books_from,
        voucher.date, voucher.voucher_number.as_deref().map(quoted).unwrap_or_else(|| "Tally assigns it".into()),
        optional(&voucher.reference), optional(&voucher.narration), entries, debit.as_str(), credit.as_str(), line.batch_id);
    // Native message boxes have no portable scrollable review surface. Keep this
    // first posting slice reviewable; longer batches retain the manual file path.
    if preview.chars().count() > 1_600
        || preview.lines().count() > 24
        || preview.lines().any(|line| line.chars().count() > 100)
    {
        return Err("import_review_too_large".into());
    }
    Ok((xml, preview))
}

fn has_unsafe_review_layout_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
}

fn has_directional_review_character(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
    })
}

#[cfg(test)]
#[path = "agent_import_post_tests.rs"]
mod tests;
