//! One user-approved voucher attempt (a Journal, Payment, Receipt or Contra);
//! subsequent calls only reconcile its identity.
use super::*;
use crate::agent::evidence_from_runtime_read;
use crate::tally::approved_import::{ApprovedImport, ApprovedImportAdmissionError};
use bridge_tally_protocol::native_outstandings::{parse_company_currency, BaseCurrencyName};
use bridge_tally_protocol::{parse_import_outcome, TallyImportApplicationStatus};

/// The batch's verification window is one the pre-flight bound (§11c) would
/// read in parts, but the pre-post check inside the dispatch lease sends it as
/// one request. Refused before approval; the batch can be posted over a
/// narrower window.
pub(super) const IMPORT_POST_WINDOW_NOT_BOUNDED: &str = "import_post_window_not_bounded";

/// Admit the whole-window pre-post request on what the verification read of
/// the same window measured: an undivided read was that request, and a divided
/// read's parts together are its size. A verification that reported nothing is
/// refused, not assumed small.
/// Which saved batches a caller may post. The agent's `post_import` posts one
/// Journal, Payment, Receipt or Contra (ADR 0004, amended 2026-09-22); the
/// desktop's review and approval were built for one Journal and stay so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::agent) enum PostScope {
    JournalOnly,
    Vouchers,
}

impl PostScope {
    fn admits(self, voucher_type: &VoucherType) -> bool {
        match self {
            Self::JournalOnly => *voucher_type == VoucherType::Journal,
            Self::Vouchers => matches!(
                voucher_type,
                VoucherType::Journal
                    | VoucherType::Payment
                    | VoucherType::Receipt
                    | VoucherType::Contra
            ),
        }
    }

    fn refusal(self) -> &'static str {
        match self {
            Self::JournalOnly => "import_post_requires_one_journal",
            Self::Vouchers => "import_post_requires_one_voucher",
        }
    }
}

/// How many bytes the in-queue classification recheck may spend describing a
/// refusal it never returns: only whether a leg failed is used.
const RECHECK_REFUSAL_BUDGET: usize = 4_096;

pub(super) fn admit_post_window(served: Option<crate::agent::WindowServed>) -> Result<(), String> {
    match served {
        Some(served) if served.fits_one_request() => Ok(()),
        _ => Err(IMPORT_POST_WINDOW_NOT_BOUNDED.to_string()),
    }
}

impl Server {
    /// The response path runs only after its interrupted post future is gone.
    /// It may therefore use the endpoint lease to distinguish a locally empty
    /// journal from another connector's in-flight pre-intent admission.
    pub(in crate::agent) fn cancelled_import_for_response(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        let snapshot = self.recorded_import_snapshot(args)?;
        let batch_id = required_string(args, "batch_id")?;
        let guid = required_string(args, "company_guid")?;
        let attempted =
            self.post_failure_attempt_observation(batch_id, guid, snapshot.as_ref(), None);
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

    pub(in crate::agent) fn recorded_import_attempt(
        &self,
        args: &Value,
    ) -> Result<Option<bool>, ToolFailure> {
        self.recorded_import_snapshot(args)
            .map(|snapshot| snapshot.map(|snapshot| snapshot.dispatched))
    }

    fn recorded_import_snapshot(
        &self,
        args: &Value,
    ) -> Result<Option<ledger::BatchSnapshot>, ToolFailure> {
        let batch_id = required_string(args, "batch_id")?;
        let guid = required_string(args, "company_guid")?;
        let uuid = batch_id
            .strip_prefix("bridge-")
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .ok_or_else(|| "import_batch_identifier_invalid".to_string())?;
        if batch_id != format!("bridge-{uuid}") {
            return Err("import_batch_identifier_invalid".to_string().into());
        }
        match self.latest_import_snapshot(batch_id) {
            Ok(snapshot) => {
                Ok(snapshot
                    .filter(|snapshot| batch_guid_matches(&snapshot.batch.company_guid, guid)))
            }
            Err(error) if error == "import_admission_busy" => Err(error.into()),
            Err(_) => Ok(None),
        }
    }

    /// The company's masters across the post (#239). Only when the snapshots
    /// either side of the POST prove the target's master mark unchanged is
    /// nothing read. Otherwise (the mark moved, or either snapshot could not
    /// be read or did not hold exactly one target row) the ledger catalogue is
    /// read again and each approved ledger's name resolved to its GUID: a
    /// posted voucher's lines carry names, not GUIDs (#239, lab comment of
    /// 2026-09-23).
    async fn masters_after_post(
        &self,
        marks_before: &str,
        marks_after: Option<&[location::LoadedCompanyMarks]>,
        identity: &super::super::VerifiedCompanyIdentity,
        company_name: &str,
        approved: &bridge_tally_protocol::StandardLedgerCatalogBinding,
        accumulated: &mut Evidence,
    ) -> Value {
        let unchanged = location::parse_all_company_marks(marks_before)
            .ok()
            .zip(marks_after)
            .and_then(|(before, after)| {
                location::target_masters_unchanged(
                    &before,
                    after,
                    identity.company_guid(),
                    company_name,
                )
            });
        let trigger = match unchanged {
            Some(true) => return json!({"state":"not_checked","reason":"masters_unmoved"}),
            Some(false) => "masters_moved",
            None => "masters_unconfirmed",
        };
        let approved = approved
            .pairs()
            .map(|(name, guid)| (name.to_string(), guid.to_string()))
            .collect::<Vec<_>>();
        self.ledgers_still_approved(identity, company_name, &approved, trigger, accumulated)
            .await
    }

    /// Whether each approved (name, GUID) still resolves by name to its GUID,
    /// as a masters check verdict with `trigger` saying why it ran (#239).
    pub(super) async fn ledgers_still_approved(
        &self,
        identity: &super::super::VerifiedCompanyIdentity,
        company_name: &str,
        approved: &[(String, String)],
        trigger: &str,
        accumulated: &mut Evidence,
    ) -> Value {
        match self
            .read_import_ledger_catalogue(identity, company_name)
            .await
        {
            Err(_) => json!({"state":"check_unavailable","trigger":trigger}),
            Ok((_, catalogue, _, evidence)) => {
                *accumulated = combine_evidence(accumulated.clone(), evidence);
                let changed = approved
                    .iter()
                    .filter(|(name, guid)| {
                        catalogue
                            .bind_selected([name.clone()])
                            .ok()
                            .and_then(|now| {
                                now.pairs()
                                    .next()
                                    .map(|(_, current)| current.eq_ignore_ascii_case(guid))
                            })
                            != Some(true)
                    })
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();
                if changed.is_empty() {
                    json!({"state":"unchanged","trigger":trigger})
                } else {
                    json!({"state":"posted_under_changed_masters","trigger":trigger,"ledgers":changed})
                }
            }
        }
    }

    pub(in crate::agent) async fn post_import(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        self.post_import_checked(args, None, PostScope::Vouchers)
            .await
    }

    pub(in crate::agent) async fn post_import_checked(
        &self,
        args: &Value,
        expected_sha256: Option<&str>,
        scope: PostScope,
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
        // Where the voucher went, once a POST has been sent (#574).
        let mut post_location: Option<Value> = None;
        // The Currency masters of a book refused for having several (#551).
        let mut currencies_seen: Option<Vec<String>> = None;
        // The masters check across the post, kept for a failed readback (#239).
        let mut masters_verdict: Option<Value> = None;
        // The ledgers whose GUID changed since the build (#239).
        let mut ledgers_changed: Option<Vec<String>> = None;
        let operation: Result<ToolOutcome, ToolFailure> = async {
            let xml = admit_saved_voucher_integrity(&line, &self.settings.endpoint, scope)?;
            if snapshot.dispatched {
                return self.verify_import(args).await;
            }
            // The record's own hash only proves the record agrees with itself.
            // The file Bridge built must hold exactly the XML this record
            // renders (bridge#575). The native post below re-renders the same
            // vouchers from the same record, differing only in a fresh private
            // REMOTEID, so every accounting field it sends is the one checked.
            if self.read_persisted_import_xml(batch_id)? != xml.as_bytes() {
                return Err("import_batch_changed".to_string().into());
            }
            // Decidable from the record alone, so refused after the file check
            // and before any Tally request (#239): a batch saved before Bridge
            // recorded its ledgers' GUIDs has nothing to check them against.
            if line.ledger_identities.is_none() {
                return Err(BuildBindingRefusal::Unbound.code().to_string().into());
            }
            // Tally may have seen any REMOTEID the journal records, and resending
            // one can undo a person's cancel or delete (protocol reference §9.3).
            // Refused before any Tally request; checked again as the intent is
            // written, under the exclusive lock.
            let remote_ids = RemoteIds::mint(line.vouchers.len())?;
            let recorded = {
                let _lock = self.lock_import_admission_shared()?;
                self.import_remote_ids_recorded_while_admitted(remote_ids.as_slice())?
            };
            if recorded {
                return Err("import_remote_id_reused".to_string().into());
            }
            let preview = admit_fresh_saved_voucher(&line, &self.settings.endpoint)?;
            // Number matching precedence is not qualified for native Create.
            // Previously dispatched numbered batches remain reconcilable above.
            // Also admits, on this read's measurement, the whole-window request
            // the lease sends before posting (§11c).
            let before = self.verify_import_for_post(args).await?;
            accumulated = combine_evidence(accumulated.clone(), before.evidence);
            require_absent_verification_result(&before.payload["result"], line.vouchers.len())?;
            let payload = ImportPayload {
                company_guid: line.company_guid.clone(),
                vouchers: line.vouchers.clone(),
                amends_batch_id: None,
            };
            // Every voucher's date, so the queue's Education recheck covers
            // them all, not only the first.
            let voucher_dates = line
                .vouchers
                .iter()
                .map(|voucher| bridge_tally_core::TallyDate::parse(voucher.date.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "voucher_date_invalid".to_string())?;
            let native = native_post_request(&line, remote_ids)?;
            let xml = native.xml.clone();
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
            if masters_for_payload(&payload, &catalogue)?
                .iter()
                .any(|item| item["match_state"] != "exact")
            {
                return Err("import_masters_changed".to_string().into());
            }
            // A ledger that now folds equal to another live ledger could be
            // taken for it by Tally's import lookup (bridge#626). Refused as the
            // build refuses it, including for a batch built before the twin
            // appeared or before the build checked for one.
            if !folded_twins(
                &requested_ledger_names(&payload),
                catalogue_identities.parents(),
            )
            .is_empty()
            {
                return Err("ledger_has_folded_twin".to_string().into());
            }
            let ledger_binding = catalogue_identities
                .bind_selected(requested_ledger_names(&payload))
                .map_err(|_| "import_masters_changed".to_string())?;
            // The same ledgers must still carry the GUIDs the build bound them
            // to (#239): a name alone cannot tell a ledger renamed and replaced
            // under its old name between build and post.
            admit_build_binding(line.ledger_identities.as_deref(), &ledger_binding).map_err(
                |refusal| {
                    if let BuildBindingRefusal::Changed(ledgers) = &refusal {
                        ledgers_changed = Some(ledgers.clone());
                    }
                    ToolFailure::from(refusal.code().to_string())
                },
            )?;
            // A Payment, Receipt or Contra is only the right type while every
            // leg classifies as its build found it. The build's own check is
            // stale by now, so classify again before approval from this
            // catalogue's parents and a fresh group collection, and send the
            // group request with the approval so the queue re-reads both.
            let group_collection_request = if renders_bank_shape(&payload.vouchers) {
                let (groups, evidence) =
                    self.read_group_collection(&identity, &company.name).await?;
                accumulated = combine_evidence(accumulated.clone(), evidence);
                let observed = ObservedMasters::new(catalogue_identities.parents(), groups);
                if cash_bank_refusals(&payload, &observed, RECHECK_REFUSAL_BUDGET).is_refused() {
                    return Err("import_bank_classification_changed".to_string().into());
                }
                Some(
                    crate::tally::agent_read_request::AgentReadRequest::parse(
                        native_group_snapshot_read(&company.name).into_xml(),
                    )
                    .map_err(|error| error.to_string())?,
                )
            } else {
                None
            };
            // Bridge's amounts are plain base-currency figures, so a post goes
            // only into a book with exactly one Currency master (bridge#551).
            // Read before approval, so a refused book is never asked about, and
            // sent with the approval so the queue reads it again.
            let (currencies, evidence) = self
                .post_read(&identity, company_currency_read(&company.name))
                .await?;
            accumulated = combine_evidence(accumulated.clone(), evidence);
            admit_post_currency(&currencies).map_err(|refusal| {
                currencies_seen = refused_currencies(&refusal);
                ToolFailure::from(refusal.to_string())
            })?;
            let currency_request = crate::tally::agent_read_request::AgentReadRequest::parse(
                company_currency_read(&company.name).into_xml(),
            )
            .map_err(|error| error.to_string())?;
            let mode = self.qualified_import_profile().await?;
            validate_post_profile_with_evidence(&payload, &mode, &mut accumulated)?;
            // Kept for the check across the post (#239).
            let approved_binding = ledger_binding.clone();
            let company_marks_request = crate::tally::agent_read_request::AgentReadRequest::parse(
                super::super::read_profiles::render_agent_company_high_water(&company.name),
            )
            .map_err(|error| error.to_string())?;
            let request = ApprovedImport::confirm(
                xml,
                &preview,
                voucher_dates,
                verification_request,
                ledger_catalogue_request,
                ledger_binding.clone(),
                group_collection_request,
                currency_request,
                company_marks_request.clone(),
            )
            .await?;
            // The cross-process lease starts only after the independent native
            // approval. It covers intent, the one POST, its response append and
            // immediate readback; recovery remains the durable batch journal.
            let _endpoint_dispatch_lease = dispatch_lease::acquire(&self.settings.endpoint)?;
            // Before anything can be sent, so a crash, a concurrent reader or a
            // failed later write reads a doubt, never an absent record (#239).
            self.record_masters_check_pending(batch_id)?;
            let posted = self
                .runtime
                .post_approved_import(
                    self.tally_config(),
                    &identity,
                    request,
                    |queued| {
                        recheck_import_admission(
                            &line,
                            identity.company_guid(),
                            &company.name,
                            queued.first,
                            queued.second,
                            queued.catalogue,
                            queued.groups,
                            queued.currencies,
                            queued.ledger_binding,
                        )?;
                        admit_queued_aim(
                            queued.company_marks_at_binding,
                            queued.company_marks,
                            identity.company_guid(),
                            &company.name,
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
                        if self.import_remote_ids_recorded_while_admitted(
                            native.remote_ids.as_slice(),
                        )? {
                            return Err("import_remote_id_reused".into());
                        }
                        if current.batch.sha256 != line.sha256
                            || current.batch.endpoint_origin != line.endpoint_origin
                        {
                            return Err("import_batch_changed".into());
                        }
                        self.append_import_record_while_admitted(
                            &ledger::StatusRecord::dispatch_for(&line, &native),
                        )
                    },
                )
                .await;
            // Whatever the outcome, the book may have changed: no ledger
            // listing of this company is continued from before it (#630).
            self.drop_listing_snapshots(identity.company_guid());
            let posted = posted.map_err(|error| {
                currencies_seen = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<ApprovedImportAdmissionError>())
                    .and_then(refused_currencies);
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
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::BankClassificationChanged)
                    )
                }) {
                    "import_bank_classification_changed"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::LedgerFoldedTwin)
                    )
                }) {
                    "ledger_has_folded_twin"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::AdmissionInconsistent)
                    )
                }) {
                    "import_post_admission_inconsistent"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::CompanyScopeChanged)
                    )
                }) {
                    "post_company_scope_changed"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::CompanyScopeUnconfirmed)
                    )
                }) {
                    "post_company_scope_unconfirmed"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::MultiCurrencyBook { .. })
                    )
                }) {
                    "import_multi_currency_unsupported"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::BaseCurrencyUndetermined)
                    )
                }) {
                    "import_base_currency_undetermined"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::MastersMoved)
                    )
                }) {
                    "post_masters_moved"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::MastersUnconfirmed)
                    )
                }) {
                    "post_masters_unconfirmed"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::CatalogueUnreadable(_))
                    )
                }) {
                    "post_catalogue_unreadable"
                } else if error.chain().any(|cause| {
                    matches!(
                        cause.downcast_ref::<ApprovedImportAdmissionError>(),
                        Some(ApprovedImportAdmissionError::GroupExportInvalid { .. })
                    )
                }) {
                    "group_export_invalid"
                } else if error
                    .chain()
                    .any(|cause| cause.is::<crate::tally::approved_import::PreIntentQueueRefusal>())
                {
                    // Refused in the queue before the intent (#656): nothing
                    // was sent, so the outcome is known.
                    "post_queue_read_failed"
                } else {
                    "import_dispatch_outcome_unknown"
                };
                // A queue read that failed in transport has no typed cause of
                // its own; the transport's safe code names it.
                let transport = (code == "post_queue_read_failed")
                    .then(|| {
                        error.chain().find_map(|cause| {
                            cause
                                .downcast_ref::<bridge_tally_transport::TallyTransportError>()
                                .map(bridge_tally_transport::TallyTransportError::safe_code)
                        })
                    })
                    .flatten();
                // The queue's group re-read names why, as the read before
                // approval does (bridge#717).
                let group = error.chain().find_map(|cause| {
                    match cause.downcast_ref::<ApprovedImportAdmissionError>() {
                        Some(ApprovedImportAdmissionError::GroupExportInvalid { cause }) => *cause,
                        _ => None,
                    }
                });
                let mut failure = ToolFailure::from_runtime(code, error);
                if failure.cause.is_none() {
                    failure.cause = group.or(transport);
                }
                failure
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
            let reported_created = parsed_outcome
                .as_ref()
                .map(|outcome| outcome.counters().created);
            let response = ledger::DispatchResponse {
                request_sha256: posted.response_evidence.request_sha256,
                response_sha256: posted.response_evidence.response_sha256,
                bytes: posted.response_evidence.bytes,
                outcome: parsed_outcome,
            };
            received_response = Some(response.clone());
            // The journal write is attempted first; its result is held, not
            // returned yet, so a local failure to journal still leaves the
            // location of a post that was sent (#574).
            let journaled = self.lock_import_admission().and_then(|_lock| {
                self.append_import_record_while_admitted(&ledger::StatusRecord::response(
                    &line, response,
                ))
            });
            // Where the voucher went (#574), read only once the journal write has
            // been attempted, so a slow or failed read delays nothing that records
            // the post. A failed read is reported, never guessed.
            let marks_after = self
                .runtime
                .read_company_marks_once(self.tally_config(), company_marks_request.clone())
                .await
                .ok()
                .and_then(|marks| location::parse_all_company_marks(&marks).ok());
            post_location = Some(location::classify_post_location(
                &location::parse_all_company_marks(&posted.company_marks_before)
                    .unwrap_or_default(),
                marks_after.as_deref(),
                identity.company_guid(),
                &company.name,
                reported_created,
            ));
            journaled?;
            // A valid counter response is evidence, never proof that Tally preserved
            // the requested ledger/amount/date semantics. Readback is mandatory.
            let masters_after_post = self
                .masters_after_post(
                    &posted.company_marks_before,
                    marks_after.as_deref(),
                    &identity,
                    &company.name,
                    &approved_binding,
                    &mut accumulated,
                )
                .await;
            // The verdict replaces the pending record before the readback, so no
            // later reconcile, which compares by name, can clear a doubt (#239).
            let masters_after_post = self.record_masters_verdict(batch_id, masters_after_post);
            masters_verdict = Some(masters_after_post.clone());
            let mut proof = self
                .verify_import_after_current_dispatch(args, masters_after_post)
                .await?;
            accumulated = combine_evidence(accumulated.clone(), proof.evidence.clone());
            proof.evidence = accumulated.clone();
            if let Some(located) = post_location.clone() {
                proof.payload["result"]["post_location"] = located;
            }
            Ok(proof)
        }
        .await;
        // Even failure after a lost response carries the saved identity. The next
        // call must reconcile that batch, never create a replacement business event.
        match operation {
            Ok(result) => Ok(result),
            Err(failure) => {
                let snapshot = self.latest_import_snapshot(batch_id).ok().flatten();
                let attempted = self.post_failure_attempt_observation(
                    batch_id,
                    guid,
                    snapshot.as_ref(),
                    received_response.as_ref(),
                );
                let cause = failure.cause;
                let mut outcome = post_failure_outcome(
                    batch_id,
                    guid,
                    failure,
                    accumulated,
                    snapshot.as_ref(),
                    received_response.as_ref(),
                    attempted,
                );
                // The typed, data-free reason, under the generic refusal's
                // budget rule; a post refusal used to drop it (bridge#634).
                if let Some(cause) = cause {
                    if self.settings.max_bytes >= crate::agent::REMEDIATION_MIN_RESPONSE_BUDGET {
                        outcome.payload["result"]["error"]["cause"] = json!(cause);
                    }
                }
                if let Some(located) = post_location {
                    outcome.payload["result"]["post_location"] = located;
                }
                if let Some(masters) = masters_verdict {
                    outcome.payload["result"]["masters_after_post"] = masters;
                }
                if let Some(currencies) = currencies_seen {
                    name_refused_currencies(&mut outcome.payload, &currencies);
                }
                if let Some(ledgers) = ledgers_changed {
                    name_changed_ledgers(&mut outcome.payload, &ledgers);
                } else {
                    explain_unbound_batch(&mut outcome.payload);
                }
                Ok(outcome)
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
        let snapshot = {
            // An empty journal cannot rule out another process's pre-intent
            // admission. Observe it only while that dispatch lane is idle.
            let _lease = dispatch_lease::acquire_observation(&self.settings.endpoint)?;
            self.latest_import_snapshot(batch_id)?
                .ok_or_else(|| "import_batch_not_found".to_string())?
        };
        if snapshot.batch.sha256 != expected_sha256 {
            return Err("import_batch_changed".to_string().into());
        }
        if !snapshot.dispatched {
            let origin = super::super::canonical_loopback_origin(&self.settings.endpoint)
                .map_err(|_| "host_setting_invalid".to_string())?;
            if snapshot.batch.endpoint_origin.as_deref() != Some(origin.as_str()) {
                return Err("import_post_endpoint_mismatch".to_string().into());
            }
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
        // Retain batch and endpoint integrity without applying approval-only
        // display restrictions or constructing an approval request.
        let _ = admit_saved_journal_integrity(&snapshot.batch, &self.settings.endpoint)?;
        self.verify_import(args).await
    }

    pub(super) fn post_failure_attempt_observation(
        &self,
        batch_id: &str,
        company_guid: &str,
        snapshot: Option<&ledger::BatchSnapshot>,
        received_response: Option<&ledger::DispatchResponse>,
    ) -> Option<bool> {
        if received_response.is_some() || snapshot.is_some_and(|snapshot| snapshot.dispatched) {
            return Some(true);
        }
        let snapshot = snapshot?;
        if !batch_guid_matches(&snapshot.batch.company_guid, company_guid) {
            return None;
        }
        let origin = super::super::canonical_loopback_origin(&self.settings.endpoint).ok()?;
        if snapshot.batch.endpoint_origin.as_deref() != Some(origin.as_str()) {
            return None;
        }
        let _lease = dispatch_lease::acquire_observation(&self.settings.endpoint).ok()?;
        self.latest_import_snapshot(batch_id)
            .ok()
            .flatten()
            .filter(|snapshot| {
                batch_guid_matches(&snapshot.batch.company_guid, company_guid)
                    && (snapshot.dispatched
                        || snapshot.batch.endpoint_origin.as_deref() == Some(origin.as_str()))
            })
            .map(|snapshot| snapshot.dispatched)
    }
}

fn post_failure_outcome(
    batch_id: &str,
    guid: &str,
    failure: ToolFailure,
    accumulated: Evidence,
    snapshot: Option<&ledger::BatchSnapshot>,
    received_response: Option<&ledger::DispatchResponse>,
    attempted: Option<bool>,
) -> ToolOutcome {
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
    payload["result"]["error"] = json!({"code":"import_reconciliation_required", "message":"The saved attempt has not been confirmed as the intended new voucher. Reconcile this original batch without resending it."});
}

/// A clean response creates exactly the batch's `voucher_count` vouchers and
/// nothing else.
fn import_outcome_is_clean(
    outcome: Option<&bridge_tally_protocol::TallyImportOutcome>,
    voucher_count: usize,
) -> bool {
    outcome.is_some_and(|outcome| {
        outcome.application_status() != TallyImportApplicationStatus::Failure
            && outcome.exceptions_were_reported()
            && outcome
                .counters()
                .is_clean_success_for(voucher_count as u64, 0, 0)
    })
}

fn persisted_response_is_clean(
    response: Option<&ledger::DispatchResponse>,
    voucher_count: usize,
) -> bool {
    response
        .is_some_and(|response| import_outcome_is_clean(response.outcome.as_ref(), voucher_count))
}

fn persisted_response_state(
    response: Option<&ledger::DispatchResponse>,
    voucher_count: usize,
) -> &'static str {
    match response {
        None => "response_missing",
        Some(response) if import_outcome_is_clean(response.outcome.as_ref(), voucher_count) => {
            "response_clean"
        }
        Some(_) => "response_not_clean",
    }
}

pub(super) fn finalize_previous_attempt_reconciliation(
    payload: &mut Value,
    response: Option<&ledger::DispatchResponse>,
    masters_after_post: Option<&Value>,
    voucher_count: usize,
) {
    let name_verified = verification_status(&payload["result"], voucher_count) == "posted_verified"
        && persisted_response_is_clean(response, voucher_count);
    // A doubt recorded when this batch was posted outlives the readback, which
    // compares by name and cannot clear it (#239).
    let doubt = post_doubt(masters_after_post, voucher_count);
    let reconciled = name_verified && doubt.is_none();
    payload["result"]["dispatch"] = json!({
        "state": if reconciled { "previous_attempt_reconciled" } else { "reconciliation_required" },
        "resent": false,
        "response_state": persisted_response_state(response, voucher_count),
        "response": response,
    });
    if !name_verified {
        mark_reconciliation_required(payload);
    } else if let Some((code, message)) = doubt {
        payload["result"]["error"] = json!({"code": code, "message": message});
    }
}

/// Whether the masters check across a post leaves doubt that the voucher went
/// to the ledgers approved (#239), as the refusal code and plain message. Only
/// an unchanged resolution, or a mark proven unmoved, admits: any other state,
/// including one this build does not know, is doubt. `None` means no check was
/// made or recorded, which is not doubt.
pub(super) fn masters_doubt(masters_after_post: Option<&Value>) -> Option<(&'static str, String)> {
    let masters = masters_after_post?;
    let state = masters["state"].as_str().unwrap_or_default();
    if state == "unchanged" || (state == "not_checked" && masters["reason"] == "masters_unmoved") {
        // Clean on the masters; a batch's recorded step verdict may still
        // doubt, independently.
        return masters
            .get("batch_step")
            .and_then(|step| batch_step_doubt(Some(step)));
    }
    const REVIEW: &str = "Review the voucher in Tally and correct it there if it went to the wrong ledger. It is already posted, so do not rebuild this event.";
    Some(if state == "posted_under_changed_masters" {
        let ledgers = masters["ledgers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        (
            "posted_under_changed_masters",
            format!("Posted to Tally, but these ledgers no longer resolve to the master you approved: {ledgers}. {REVIEW}"),
        )
    } else {
        let again = if state == super::MASTERS_CHECK_PENDING || state == "check_unavailable" {
            " A later verify_import of this batch checks again."
        } else {
            ""
        };
        (
            "masters_after_post_unconfirmed",
            format!("Posted to Tally, but Bridge could not confirm that its ledgers are still the masters you approved.{again} {REVIEW}"),
        )
    })
}

/// A batch's step doubt: its target's voucher mark did not move by exactly
/// Tally's CREATED, or that was never recorded (pending, unreadable or
/// absent). Nothing in the step says which voucher, so the review is of the
/// whole batch.
fn batch_step_doubt(step: Option<&Value>) -> Option<(&'static str, String)> {
    if step.is_some_and(|step| step["state"] == "matched") {
        return None;
    }
    Some((
        "batch_step_unconfirmed",
        "Tally reported creating the batch, but this company's voucher mark did not move by exactly that many, so another change may have been made in it while the batch was posting. Review the batch's vouchers in Tally. They are already posted, so do not rebuild this batch. No review record is available for a batch yet.".to_string(),
    ))
}

/// Every doubt across the post: the masters check, and for a batch its step,
/// which must be recorded as matched.
fn post_doubt(
    masters_after_post: Option<&Value>,
    voucher_count: usize,
) -> Option<(&'static str, String)> {
    masters_doubt(masters_after_post).or_else(|| {
        (voucher_count > 1)
            .then(|| {
                batch_step_doubt(masters_after_post.and_then(|masters| masters.get("batch_step")))
            })
            .flatten()
    })
}

pub(super) fn finalize_current_dispatch(
    payload: &mut Value,
    response: Option<&ledger::DispatchResponse>,
    masters_after_post: Option<&Value>,
    voucher_count: usize,
) {
    let verified = verification_status(&payload["result"], voucher_count) == "posted_verified";
    let clean = persisted_response_is_clean(response, voucher_count);
    let masters_doubt = post_doubt(masters_after_post, voucher_count);
    payload["result"]["dispatch"] = json!({
        "state": if clean && verified && masters_doubt.is_none() { "posted_verified" } else { "reconciliation_required" },
        "counters":response.and_then(|response| response.outcome.as_ref().map(|outcome| outcome.counters())),
        "application_status":response.and_then(|response| response.outcome.as_ref().map(|outcome| outcome.application_status())),
        "response_state": persisted_response_state(response, voucher_count),
        "response": response,
        "resent":false, "automatic_retry":false
    });
    if !clean || !verified {
        mark_reconciliation_required(payload);
    } else if let Some((code, message)) = masters_doubt {
        payload["result"]["error"] = json!({"code": code, "message": message});
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

/// Every one of the batch's `voucher_count` vouchers is absent from the book.
fn require_absent_verification_result(result: &Value, voucher_count: usize) -> Result<(), String> {
    if voucher_count == 0
        || result["counts"]["not_found"].as_u64() != Some(voucher_count as u64)
        || result["vouchers"].as_array().map(Vec::len) != Some(voucher_count)
    {
        return Err("import_preexisting_identity".into());
    }
    Ok(())
}

/// The aim check on the snapshot the queue read last before the POST (#574).
/// The aim check (#574), then the master mark (#239): the target's ALTMSTID in
/// the aim snapshot must equal the one read as the queue's binding reads began.
fn admit_queued_aim(
    marks_at_binding: &str,
    marks: &str,
    company_guid: &str,
    company_name: &str,
) -> anyhow::Result<()> {
    let rows = location::parse_all_company_marks(marks)
        .map_err(|_| ApprovedImportAdmissionError::CompanyScopeUnconfirmed)?;
    location::admit_post_target(&rows, company_guid, company_name)
        .map_err(|_| ApprovedImportAdmissionError::CompanyScopeChanged)?;
    let at_binding = location::parse_all_company_marks(marks_at_binding)
        .map_err(|_| ApprovedImportAdmissionError::MastersUnconfirmed)?;
    match location::target_masters_unchanged(&at_binding, &rows, company_guid, company_name) {
        Some(true) => Ok(()),
        Some(false) => Err(ApprovedImportAdmissionError::MastersMoved.into()),
        None => Err(ApprovedImportAdmissionError::MastersUnconfirmed.into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn recheck_import_admission(
    line: &ImportLedgerLine,
    company_guid: &str,
    company_name: &str,
    first: &str,
    second: &str,
    catalogue: &str,
    groups: Option<&str>,
    currencies: &str,
    ledger_binding: &bridge_tally_protocol::StandardLedgerCatalogBinding,
) -> anyhow::Result<()> {
    let observed = parse_import_vouchers(first, company_guid).map_err(anyhow::Error::msg)?;
    let corroboration = parse_import_vouchers(second, company_guid).map_err(anyhow::Error::msg)?;
    corroborate_verification_window(&observed, &corroboration, &line.date_from, &line.date_to)
        .map_err(anyhow::Error::msg)?;
    let result = verify_batch(line, &observed).map_err(anyhow::Error::msg)?;
    require_absent_verification_result(&result, line.vouchers.len()).map_err(|code| match code
        .as_str()
    {
        "import_preexisting_identity" => ApprovedImportAdmissionError::PreexistingIdentity.into(),
        _ => anyhow::Error::msg(code),
    })?;
    if !ledger_binding
        .matches(catalogue, company_name, company_guid)
        .map_err(ApprovedImportAdmissionError::CatalogueUnreadable)?
    {
        return Err(ApprovedImportAdmissionError::LedgerIdentityChanged.into());
    }
    let parents = parse_standard_ledger_catalog_response(catalogue, company_name, company_guid)
        .map_err(ApprovedImportAdmissionError::CatalogueUnreadable)?;
    // Nor can the binding see a ledger added since approval that folds equal to
    // a named one, which Tally's import lookup could take for it (bridge#626).
    let named = line
        .vouchers
        .iter()
        .flat_map(|voucher| &voucher.entries)
        .map(|entry| entry.ledger.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !folded_twins(&named, parents.parents()).is_empty() {
        return Err(ApprovedImportAdmissionError::LedgerFoldedTwin.into());
    }
    // The binding above compares each ledger's name and GUID, not its parent,
    // so it cannot see a ledger or a group re-parented since approval. A bank
    // voucher's type rests on exactly that, so classify every leg again from
    // this catalogue's parents and the group collection read beside it.
    let bank = renders_bank_shape(&line.vouchers);
    match (bank, groups) {
        (false, None) => {}
        (true, Some(groups)) => {
            let groups = parse_native_group_snapshot(groups, company_guid).map_err(|error| {
                ApprovedImportAdmissionError::GroupExportInvalid {
                    cause: crate::tally::approved_import::group_snapshot_cause(&error),
                }
            })?;
            let payload = ImportPayload {
                company_guid: line.company_guid.clone(),
                vouchers: line.vouchers.clone(),
                amends_batch_id: None,
            };
            let observed = ObservedMasters::new(parents.parents(), groups);
            if cash_bank_refusals(&payload, &observed, RECHECK_REFUSAL_BUDGET).is_refused() {
                return Err(ApprovedImportAdmissionError::BankClassificationChanged.into());
            }
        }
        // A bank voucher without its group read, or a Journal with one, is a
        // wiring fault; refuse rather than post on half a check.
        _ => return Err(ApprovedImportAdmissionError::AdmissionInconsistent.into()),
    }
    // A Currency master can be added while approval waits (bridge#551).
    admit_post_currency(currencies)?;
    Ok(())
}

/// Admit a post on the company's Currency masters (bridge#551). Bridge's
/// amounts are plain base-currency figures, and a foreign-currency ledger's
/// balance can read as a plain amount too, so only the ledger's own currency
/// tells them apart (TALLY_PROTOCOL_REFERENCE §8.2d). Which master is the base
/// cannot be identified among several until bridge#601, so until then a post
/// goes only into a book with exactly one. That every ledger of such a book is
/// in the base is an inference (a ledger's currency is one of the book's
/// masters), not a measurement. When #601 lands, each leg's `CURRENCYNAME` is
/// compared with the base instead. A response that parses to no master, or
/// does not parse (a master without a NAME does not), is
/// `BaseCurrencyUndetermined`.
fn admit_post_currency(currencies: &str) -> Result<(), ApprovedImportAdmissionError> {
    let currency = parse_company_currency(currencies)
        .map_err(|_| ApprovedImportAdmissionError::BaseCurrencyUndetermined)?;
    if BaseCurrencyName::of_single_master(&currency).is_some() {
        Ok(())
    } else if currency.currency_count > 1 {
        Err(ApprovedImportAdmissionError::MultiCurrencyBook {
            currencies: currency.names,
        })
    } else {
        Err(ApprovedImportAdmissionError::BaseCurrencyUndetermined)
    }
}

fn refused_currencies(refusal: &ApprovedImportAdmissionError) -> Option<Vec<String>> {
    match refusal {
        ApprovedImportAdmissionError::MultiCurrencyBook { currencies } => Some(currencies.clone()),
        _ => None,
    }
}

/// Why a saved batch's build-time ledger binding does not admit this post.
#[derive(Debug, PartialEq, Eq)]
enum BuildBindingRefusal {
    /// Built before Bridge recorded ledger identities: nothing to compare.
    Unbound,
    /// These ledgers now resolve to another GUID, or are not in the record.
    Changed(Vec<String>),
}

impl BuildBindingRefusal {
    fn code(&self) -> &'static str {
        match self {
            Self::Unbound => "import_batch_predates_ledger_binding",
            Self::Changed(_) => "import_masters_changed_since_build",
        }
    }
}

/// Every ledger the post binds now must carry the GUID the build bound it to,
/// compared as the catalogue binding compares GUIDs (ASCII case folded).
fn admit_build_binding(
    recorded: Option<&[BoundLedger]>,
    current: &bridge_tally_protocol::StandardLedgerCatalogBinding,
) -> Result<(), BuildBindingRefusal> {
    let recorded = recorded.ok_or(BuildBindingRefusal::Unbound)?;
    let changed = current
        .pairs()
        .filter(|(name, guid)| {
            !recorded
                .iter()
                .any(|bound| bound.name == *name && bound.guid.eq_ignore_ascii_case(guid))
        })
        .map(|(name, _)| name.to_string())
        .collect::<Vec<_>>();
    if changed.is_empty() {
        Ok(())
    } else {
        Err(BuildBindingRefusal::Changed(changed))
    }
}

/// How many changed ledgers a refusal names; the rest are counted.
const REFUSAL_LEDGERS_NAMED: usize = 8;

/// Name the ledgers whose GUID changed since the build, in plain words, where
/// no attempt is recorded.
fn name_changed_ledgers(payload: &mut Value, ledgers: &[String]) {
    let named = ledgers
        .iter()
        .take(REFUSAL_LEDGERS_NAMED)
        .cloned()
        .collect::<Vec<_>>();
    let error = &mut payload["result"]["error"];
    error["ledgers_changed"] = json!(named);
    error["ledgers_changed_total"] = json!(ledgers.len());
    if payload["result"]["attempt_recorded"] == json!(false) {
        let mut list = named.join(", ");
        if ledgers.len() > named.len() {
            list.push_str(&format!(" and {} more", ledgers.len() - named.len()));
        }
        payload["result"]["error"]["message"] = json!(format!(
            "A ledger this batch names is no longer the one it was built against ({list}): the \
             name now belongs to a different ledger in Tally. Nothing was posted. Confirm which \
             ledger you meant (it may now have another name) before building the batch again."
        ));
    }
}

/// Say plainly that a batch built before ledger identities were recorded must
/// be rebuilt, where no attempt is recorded.
fn explain_unbound_batch(payload: &mut Value) {
    if payload["result"]["error"]["code"] == json!("import_batch_predates_ledger_binding")
        && payload["result"]["attempt_recorded"] == json!(false)
    {
        payload["result"]["error"]["message"] = json!(
            "This batch was built before Bridge recorded which ledgers it was built against, so \
             it cannot be checked. Nothing was posted. Build the batch again, then post the new \
             batch."
        );
    }
}

/// How many Currency masters a refusal names; the rest are counted.
const REFUSAL_CURRENCIES_NAMED: usize = 8;

/// Say plainly why a multi-currency book was refused, naming the masters seen.
/// The message is replaced only where no attempt is recorded: any other state
/// keeps its instruction to reconcile.
fn name_refused_currencies(payload: &mut Value, currencies: &[String]) {
    let named = currencies
        .iter()
        .take(REFUSAL_CURRENCIES_NAMED)
        .cloned()
        .collect::<Vec<_>>();
    let error = &mut payload["result"]["error"];
    error["currencies_seen"] = json!(named);
    error["currencies_total"] = json!(currencies.len());
    if payload["result"]["attempt_recorded"] == json!(false) {
        let mut list = named.join(", ");
        if currencies.len() > named.len() {
            list.push_str(&format!(" and {} more", currencies.len() - named.len()));
        }
        payload["result"]["error"]["message"] = json!(format!(
            "This company has more than one currency defined ({list}); Bridge does not post \
             into multi-currency books yet. Nothing was posted."
        ));
    }
}

#[cfg(test)]
tokio::task_local! {
    /// Test-only: the REMOTEID a post mints, so a test can make it one the
    /// journal already records.
    pub(super) static SCRIPTED_REMOTE_ID: Uuid;
}

/// A fresh random REMOTEID for one native post.
fn mint_remote_id() -> Uuid {
    #[cfg(test)]
    if let Ok(scripted) = SCRIPTED_REMOTE_ID.try_with(|id| *id) {
        return scripted;
    }
    Uuid::new_v4()
}

/// The fresh REMOTEIDs of one native post, one per voucher in order, all
/// distinct. They are minted together at dispatch time and never derived, so
/// none can repeat one Tally may have seen (protocol reference §9.3).
pub(super) struct RemoteIds(Vec<Uuid>);

impl RemoteIds {
    /// At least one id; a repeat, which a v4 UUID makes vanishingly rare, is
    /// discarded, and a run of repeats refuses rather than loop.
    pub(super) fn mint(count: usize) -> Result<Self, String> {
        if count == 0 {
            return Err("import_post_requires_one_voucher".into());
        }
        // No more than the journal will admit on read, so an intent is never
        // written that the next journal read would refuse.
        if count > ledger::MAX_BATCH_POST_VOUCHERS {
            return Err("import_post_batch_too_large".into());
        }
        let mut ids = Vec::with_capacity(count);
        let mut repeats = 0;
        while ids.len() < count {
            let id = mint_remote_id();
            if ids.contains(&id) {
                repeats += 1;
                if repeats > 3 {
                    return Err("import_remote_id_reused".into());
                }
            } else {
                ids.push(id);
            }
        }
        Ok(Self(ids))
    }

    #[cfg(test)]
    pub(super) fn from_ids(ids: Vec<Uuid>) -> Self {
        Self(ids)
    }

    pub(super) fn as_slice(&self) -> &[Uuid] {
        &self.0
    }
}

/// One native post: the request bytes, their wire digest, and the fresh
/// REMOTEIDs they carry, one per voucher. The post records `request_sha256`
/// and the REMOTEIDs together in its dispatch intent, before sending
/// (bridge#579).
pub(super) struct NativePostRequest {
    pub(super) xml: String,
    pub(super) request_sha256: String,
    pub(super) remote_ids: RemoteIds,
}

pub(super) fn native_post_request(
    line: &ImportLedgerLine,
    remote_ids: RemoteIds,
) -> Result<NativePostRequest, String> {
    let company = line
        .company
        .as_ref()
        .ok_or_else(|| "import_post_company_missing".to_string())?;
    if remote_ids.as_slice().len() != line.vouchers.len() {
        return Err("import_post_remote_ids_mismatch".into());
    }
    let xml = render_native_vouchers_xml(
        &company.name,
        line.identity_batch_id(),
        line.vouchers
            .iter()
            .zip(remote_ids.as_slice().iter().copied()),
    );
    let request_sha256 = sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
        &xml,
    ));
    Ok(NativePostRequest {
        xml,
        request_sha256,
        remote_ids,
    })
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
    admit_saved_voucher_integrity(line, endpoint, PostScope::JournalOnly)
}

pub(super) fn admit_saved_voucher_integrity(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
    scope: PostScope,
) -> Result<String, String> {
    if line.vouchers.len() != 1
        || !scope.admits(&line.vouchers[0].voucher_type)
        || line.identity_scheme != Some(ImportIdentityScheme::BatchV1)
    {
        return Err(scope.refusal().into());
    }
    // A native post uses a fresh private REMOTEID, so it can only create. An
    // amendment exists to alter a voucher already in the book in place.
    if line.amends_batch_id.is_some() {
        return Err("import_post_amendment_requires_file_import".into());
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
        amends_batch_id: None,
    };
    validate_payload(&payload)?;
    totals(&line.vouchers)?;
    let xml = render_import_xml(&company.name, &line.vouchers, line.identity_batch_id());
    if sha256_hex(xml.as_bytes()) != line.sha256 {
        return Err("import_batch_changed".into());
    }
    Ok(xml)
}

pub(super) fn admit_saved_journal(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
) -> Result<(String, String), String> {
    admit_saved_voucher(line, endpoint, PostScope::JournalOnly)
}

pub(super) fn admit_saved_voucher(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
    scope: PostScope,
) -> Result<(String, String), String> {
    let xml = admit_saved_voucher_integrity(line, endpoint, scope)?;
    let preview = admit_fresh_saved_voucher(line, endpoint)?;
    Ok((xml, preview))
}

/// What the approval must show about a bank voucher's legs: which side had to
/// be bank or cash, and that Bridge checks it again just before posting.
fn classification_review_line(voucher_type: &VoucherType) -> Option<&'static str> {
    match voucher_type {
        VoucherType::Journal => None,
        VoucherType::Payment => {
            Some("Checked in Tally: every Cr ledger is bank/cash; every Dr ledger holds no money.")
        }
        VoucherType::Receipt => {
            Some("Checked in Tally: every Dr ledger is bank/cash; every Cr ledger holds no money.")
        }
        VoucherType::Contra => Some("Checked in Tally: every ledger is bank/cash."),
    }
}

fn admit_fresh_saved_voucher(
    line: &ImportLedgerLine,
    endpoint: &super::super::TallyEndpointConfig,
) -> Result<String, String> {
    let company = line.company.as_ref().ok_or("import_post_company_missing")?;
    let origin =
        super::super::canonical_loopback_origin(endpoint).map_err(|_| "host_setting_invalid")?;
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
    require_native_numbering(voucher)?;
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
    let classification = classification_review_line(&voucher.voucher_type)
        .map(|line| format!("\n{line}"))
        .unwrap_or_default();
    let preview = format!("Create ONE {} in {}\nCompany GUID: {}\nCompany number: {}  Books from: {}\nTally: {origin}\nDate: {}  Voucher number: {}\nReference: {}\nNarration: {}\n\n{}\n\nTotal debit: {}  Total credit: {}{classification}\nBatch: {}\n\nLedgers checked by identity against the build; Bridge adds its batch reference.\nDo not post a file already imported manually.\nPause other edits/imports; keep this company and Tally mode unchanged until Bridge finishes.\nAfter a timeout, reconcile this batch; do not rebuild or resend it.",
        voucher.voucher_type.as_str(), quoted(&company.name), company.guid, company.company_number, company.books_from,
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

pub(super) fn has_unsafe_review_layout_character(value: &str) -> bool {
    value
        .chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
}

pub(super) fn has_unreviewable_format_character(value: &str) -> bool {
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

#[path = "agent_import_post_location.rs"]
mod location;

#[cfg(test)]
#[path = "agent_import_post_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_import_post_e2e_tests.rs"]
mod e2e_tests;
