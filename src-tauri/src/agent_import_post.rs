//! One user-approved Journal attempt; subsequent calls only reconcile its identity.
use super::*;
use crate::agent::evidence_from_runtime_read;
use crate::tally::approved_import::{ApprovedImport, ApprovedImportAdmissionError};
use bridge_tally_protocol::{parse_import_outcome, TallyImportApplicationStatus};

impl Server {
    /// Describes the durable cancellation boundary without changing the batch.
    /// Missing or unreadable history is unknown, never proof of no dispatch.
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
        let attempted = match self.latest_import_snapshot(batch_id) {
            Ok(snapshot) => snapshot
                .filter(|snapshot| batch_guid_matches(&snapshot.batch.company_guid, guid))
                .map(|snapshot| snapshot.dispatched),
            Err(error) if error == "import_admission_busy" => return Err(error.into()),
            Err(_) => None,
        };
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
        self.post_import_checked(args, None).await
    }

    pub(in crate::agent) async fn post_import_checked(
        &self,
        args: &Value,
        expected_sha256: Option<&str>,
    ) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        let batch_id = required_string(args, "batch_id")?;
        let snapshot = self
            .latest_import_snapshot(batch_id)?
            .ok_or_else(|| "import_batch_not_found".to_string())?;
        if expected_sha256.is_some_and(|expected| snapshot.batch.sha256 != expected) {
            return Err("import_batch_changed".to_string().into());
        }
        let line = snapshot.batch.clone();
        if !batch_guid_matches(&line.company_guid, guid) {
            return Err("import_batch_company_mismatch".to_string().into());
        }
        let mut accumulated =
            evidence_from_runtime_read(crate::tally::runtime::RuntimeReadEvidence::empty());
        let mut received_response = None;
        let operation: Result<ToolOutcome, ToolFailure> = async {
        let _xml = admit_saved_journal_integrity(&line, &self.settings.endpoint)?;
        if snapshot.dispatched {
            return self.verify_import(args).await;
        }
        let preview = admit_fresh_saved_journal(&line)?;
            // Number matching precedence is not qualified for native Create.
            // Previously dispatched numbered batches remain reconcilable above.
            require_native_numbering(&line.vouchers[0])?;
            let before = self.verify_import(args).await?;
            accumulated = combine_evidence(accumulated.clone(), before.evidence);
            require_absent_verification_result(&before.payload["result"])?;
            let payload = ImportPayload {
                company_guid: line.company_guid.clone(),
                vouchers: line.vouchers.clone(),
            };
            let voucher_date = bridge_tally_core::TallyDate::parse(line.vouchers[0].date.clone())
                .map_err(|_| "voucher_date_invalid".to_string())?;
            let xml = render_native_journal_xml(
                &line
                    .company
                    .as_ref()
                    .ok_or_else(|| "import_post_company_missing".to_string())?
                    .name,
                &line.vouchers[0],
                &line.batch_id,
            );
            let request_sha256 = sha256_hex(
                &bridge_tally_protocol::encode_tally_xml_request_utf16le(&xml),
            );
            let verification_request = crate::tally::agent_read_request::AgentReadRequest::parse(
                render_import_verification_read(
                    &line
                        .company
                        .as_ref()
                        .ok_or_else(|| "import_post_company_missing".to_string())?
                        .name,
                    &line.date_from,
                    &line.date_to,
                ),
            )
            .map_err(|error| error.to_string())?;
            let (company, identity, identity_evidence) = self.verified_company(guid).await?;
            accumulated = combine_evidence(accumulated.clone(), identity_evidence);
            if line.company.as_ref() != Some(&import_company_tuple(&company)?) {
                return Err("company_identity_mismatch".to_string().into());
            }
            validate_dates(&payload, company.books_from.as_deref())?;
            let (catalogue, catalogue_identities, ledger_catalogue_request, evidence) = self
                .read_import_ledger_catalogue(&identity, &company.name)
                .await?;
            accumulated = combine_evidence(accumulated.clone(), evidence);
            if masters_for_payload(&payload, &catalogue)
                .iter()
                .any(|item| item["match_state"] != "exact")
            {
                return Err("import_masters_changed".to_string().into());
            }
            let ledger_binding = catalogue_identities
                .bind_selected(requested_ledger_names(&payload))
                .map_err(|_| "import_masters_changed".to_string())?;
            let mode = self.qualified_import_profile().await?;
            validate_post_profile_with_evidence(&payload, &mode, &mut accumulated)?;
            let request = ApprovedImport::confirm(
                xml,
                &preview,
                voucher_date,
                verification_request,
                ledger_catalogue_request,
                ledger_binding,
            )
            .await?;
            // The cross-process lease starts only after the independent native
            // approval. It covers intent, the one POST, its response append and
            // immediate readback; recovery remains the durable batch journal.
            let _endpoint_dispatch_lease = dispatch_lease::acquire(&self.settings.endpoint)?;
            let posted = self
                .runtime
                .post_approved_import(
                    self.tally_config(),
                    &identity,
                    request,
                    |first, second, catalogue, ledger_binding| {
                        recheck_import_admission(
                            &line,
                            identity.company_guid(),
                            &company.name,
                            first,
                            second,
                            catalogue,
                            ledger_binding,
                        )
                    },
                    || {
                        // The file lock covers only the admission+synced append. It is not
                        // held over approval or network I/O. A competing process loses here.
                        let _lock = self.lock_import_admission()?;
                        let current = self
                            .import_snapshot_while_admitted(Some(batch_id))?
                            .ok_or_else(|| "import_batch_not_found".to_string())?;
                        if current.dispatched {
                            return Err("import_already_attempted".into());
                        }
                        if current.batch.sha256 != line.sha256
                            || current.batch.endpoint_origin != line.endpoint_origin
                        {
                            return Err("import_batch_changed".into());
                        }
                        self.append_import_record_while_admitted(
                            &ledger::StatusRecord::dispatch_native(&line, request_sha256.clone()),
                        )
                    },
                )
                .await;
            let posted = posted.map_err(|error| {
                let code = if error.chain().any(|cause| {
                    cause.is::<crate::tally::approved_import::AmbiguousImportCompany>()
                }) {
                    "import_company_scope_ambiguous"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::EducationVoucherDateUnsupported)
                    )
                }) {
                    "education_voucher_date_unsupported"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::PreexistingIdentity)
                    )
                }) {
                    "import_preexisting_identity"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::LedgerIdentityChanged)
                    )
                }) {
                    "import_masters_changed"
                } else {
                    "import_dispatch_outcome_unknown"
                };
                ToolFailure::from_runtime(code, error)
            })?;
            accumulated = combine_evidence(
                accumulated.clone(),
                evidence_from_runtime_read(posted.admission_evidence.clone()),
            );
            accumulated = combine_evidence(
                accumulated.clone(),
                evidence_from_runtime_read(posted.response_evidence.clone()),
            );
            let parsed_outcome = parse_import_outcome(&posted.body).ok();
            let response = ledger::DispatchResponse {
                request_sha256: posted.response_evidence.request_sha256,
                response_sha256: posted.response_evidence.response_sha256,
                bytes: posted.response_evidence.bytes,
                outcome: parsed_outcome,
            };
            received_response = Some(response.clone());
            {
                let _lock = self.lock_import_admission()?;
                self.append_import_record_while_admitted(&ledger::StatusRecord::response(
                    &line, response,
                ))?;
            }
            // A valid counter response is evidence, never proof that Tally preserved
            // the requested ledger/amount/date semantics. Readback is mandatory.
            let mut proof = self.verify_import_after_current_dispatch(args).await?;
            accumulated = combine_evidence(accumulated.clone(), proof.evidence.clone());
            proof.evidence = accumulated.clone();
            Ok(proof)
        }
        .await;
        // Even failure after a lost response carries the saved identity. The next
        // call must reconcile that batch, never create a replacement business event.
        match operation {
            Ok(result) => Ok(result),
            Err(failure) => {
                let snapshot = self.latest_import_snapshot(batch_id).ok().flatten();
                Ok(post_failure_outcome(
                    batch_id,
                    guid,
                    failure,
                    accumulated,
                    snapshot.as_ref(),
                    received_response.as_ref(),
                ))
            }
        }
    }

    /// Reconciliation is read-only. Unlike `post_import`, it never reaches the
    /// approval dialog or dispatch path when a concurrent history change means
    /// there is no durable attempt to reconcile.
    pub(in crate::agent) async fn reconcile_import(
        &self,
        args: &Value,
        expected_sha256: &str,
    ) -> Result<ToolOutcome, ToolFailure> {
        let batch_id = required_string(args, "batch_id")?;
        let snapshot = self
            .latest_import_snapshot(batch_id)?
            .ok_or_else(|| "import_batch_not_found".to_string())?;
        if snapshot.batch.sha256 != expected_sha256 {
            return Err("import_batch_changed".to_string().into());
        }
        if !snapshot.dispatched {
            return Err("import_not_dispatched".to_string().into());
        }
        self.reconcile_dispatched_import(args, snapshot).await
    }

    async fn reconcile_dispatched_import(
        &self,
        args: &Value,
        snapshot: ledger::BatchSnapshot,
    ) -> Result<ToolOutcome, ToolFailure> {
        let guid = required_string(args, "company_guid")?;
        if !batch_guid_matches(&snapshot.batch.company_guid, guid) {
            return Err("import_batch_company_mismatch".to_string().into());
        }
        // Keep the exact same batch, endpoint and native-review admission as
        // the initial path, but do not construct an approval request here.
        let _ = admit_saved_journal(&snapshot.batch, &self.settings.endpoint)?;
        self.verify_import(args).await
    }
}

fn post_failure_outcome(
    batch_id: &str,
    guid: &str,
    failure: ToolFailure,
    accumulated: Evidence,
    snapshot: Option<&ledger::BatchSnapshot>,
    received_response: Option<&ledger::DispatchResponse>,
) -> ToolOutcome {
    let attempted = received_response
        .is_some()
        .then_some(true)
        .or_else(|| snapshot.map(|snapshot| snapshot.dispatched));
    let persisted_response = snapshot.and_then(|snapshot| snapshot.response.as_ref());
    let response = received_response.or(persisted_response);
    let mut evidence = failure
        .evidence
        .map(|item| combine_evidence(accumulated.clone(), *item))
        .unwrap_or(accumulated);
    evidence.state = "partial";
    evidence.reason_code = Some(failure.code.clone());
    ToolOutcome {
        payload: reconciliation_failure_payload(batch_id, attempted, response, &failure.code),
        evidence,
        company_guid: Some(guid.to_string()),
        truncated: false,
    }
}

fn reconciliation_failure_payload(
    batch_id: &str,
    attempted: Option<bool>,
    response: Option<&ledger::DispatchResponse>,
    code: &str,
) -> Value {
    json!({"result":{"batch_id":batch_id,"attempt_recorded":attempted,"dispatch_response":response,"error":{"code":code,
        "message":if attempted == Some(false) { "No posting attempt was recorded. Review the error before requesting approval again." }
        else { "The saved batch requires reconciliation. Use verify_import with this original batch; never rebuild it to retry." }}}})
}

fn mark_reconciliation_required(payload: &mut Value) {
    payload["result"]["error"] = json!({"code":"import_reconciliation_required", "message":"The saved attempt has not been confirmed as the intended new Journal. Reconcile this original batch without resending it."});
}

fn import_outcome_is_clean(outcome: Option<&bridge_tally_protocol::TallyImportOutcome>) -> bool {
    outcome.is_some_and(|outcome| {
        outcome.application_status() != TallyImportApplicationStatus::Failure
            && outcome.exceptions_were_reported()
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

pub(super) fn finalize_previous_attempt_reconciliation(
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

pub(super) fn finalize_current_dispatch(
    payload: &mut Value,
    response: Option<&ledger::DispatchResponse>,
) {
    let verified = verification_status(&payload["result"], 1) == "posted_verified";
    let clean = persisted_response_is_clean(response);
    payload["result"]["dispatch"] = json!({
        "state": if clean && verified { "posted_verified" } else { "reconciliation_required" },
        "counters":response.and_then(|response| response.outcome.as_ref().map(|outcome| outcome.counters())),
        "application_status":response.and_then(|response| response.outcome.as_ref().map(|outcome| outcome.application_status())),
        "response_state": persisted_response_state(response),
        "response": response,
        "resent":false, "automatic_retry":false
    });
    if !clean || !verified {
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

fn require_absent_verification_result(result: &Value) -> Result<(), String> {
    if result["counts"]["not_found"].as_u64() != Some(1)
        || result["vouchers"].as_array().map(Vec::len) != Some(1)
    {
        return Err("import_preexisting_identity".into());
    }
    Ok(())
}

fn recheck_import_admission(
    line: &ImportLedgerLine,
    company_guid: &str,
    company_name: &str,
    first: &str,
    second: &str,
    catalogue: &str,
    ledger_binding: &bridge_tally_protocol::StandardLedgerCatalogBinding,
) -> anyhow::Result<()> {
    let observed = parse_import_vouchers(first, company_guid).map_err(anyhow::Error::msg)?;
    let corroboration = parse_import_vouchers(second, company_guid).map_err(anyhow::Error::msg)?;
    corroborate_verification_window(&observed, &corroboration, &line.date_from, &line.date_to)
        .map_err(anyhow::Error::msg)?;
    let result = verify_batch(line, &observed).map_err(anyhow::Error::msg)?;
    require_absent_verification_result(&result).map_err(|code| match code.as_str() {
        "import_preexisting_identity" => ApprovedImportAdmissionError::PreexistingIdentity.into(),
        _ => anyhow::Error::msg(code),
    })?;
    if !ledger_binding
        .matches(catalogue, company_name, company_guid)
        .map_err(|_| anyhow::Error::msg("ledger_export_invalid"))?
    {
        return Err(ApprovedImportAdmissionError::LedgerIdentityChanged.into());
    }
    Ok(())
}

pub(super) fn require_native_numbering(voucher: &ImportVoucher) -> Result<(), String> {
    if voucher.voucher_number.is_some() {
        return Err("import_post_numbered_journal_unsupported".into());
    }
    Ok(())
}

pub(super) fn admit_saved_journal_integrity(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
) -> Result<String, String> {
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
    totals(&line.vouchers)?;
    let xml = render_import_xml(&company.name, &line.vouchers, &line.batch_id);
    if sha256_hex(xml.as_bytes()) != line.sha256 {
        return Err("import_batch_changed".into());
    }
    Ok(xml)
}

pub(super) fn admit_saved_journal(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
) -> Result<(String, String), String> {
    let xml = admit_saved_journal_integrity(line, endpoint)?;
    let preview = admit_fresh_saved_journal(line)?;
    Ok((xml, preview))
}

fn admit_fresh_saved_journal(line: &ImportLedgerLine) -> Result<String, String> {
    let company = line.company.as_ref().ok_or("import_post_company_missing")?;
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
    if review_text.any(has_unreviewable_format_character) {
        return Err("import_review_format_text".into());
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
    let preview = format!("Create ONE Journal in {}\nCompany GUID: {}\nCompany number: {}  Books from: {}\nTally: {origin}\nDate: {}  Voucher number: {}\nReference: {}\nNarration: {}\n\n{}\n\nTotal debit: {}  Total credit: {}\nBatch: {}\n\nBridge adds its batch reference for readback.\nDo not post a file already imported manually.\nCheck every ledger, date and amount. This changes your accounts.\nAfter a timeout, reconcile this batch; do not rebuild or resend it.",
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
    Ok(preview)
}

fn has_unsafe_review_layout_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
}

fn has_unreviewable_format_character(value: &str) -> bool {
    use icu_properties::{
        props::{DefaultIgnorableCodePoint, GeneralCategory},
        CodePointMapData, CodePointSetData,
    };
    // Unicode UAX #44: General_Category=Cf and Default_Ignorable_Code_Point.
    // Native approval must not hide characters that distinguish exact XML names.
    let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    let category = CodePointMapData::<GeneralCategory>::new();
    value.chars().any(|character| {
        ignorable.contains(character) || category.get(character) == GeneralCategory::Format
    })
}

#[cfg(test)]
#[path = "agent_import_post_tests.rs"]
mod tests;
