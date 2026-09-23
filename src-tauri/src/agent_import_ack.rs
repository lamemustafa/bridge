//! A person's record that they reviewed a doubted post (#239).
//!
//! A post whose masters check found a ledger now resolving to another master
//! (`posted_under_changed_masters`) reads `reconciliation_required` on every
//! later readback, which compares by name and cannot clear it. After someone
//! has checked the voucher in Tally, that verdict still stands: this records,
//! beside it, that they did. It changes nothing in Tally, changes no verdict
//! and unblocks nothing. It is written only after the native review dialog,
//! which the model cannot answer.
//!
//! The record binds the exact doubt shown (the sha256 of its bytes) and the
//! voucher as read (GUID, MASTERID, ALTERID and a fingerprint of every field
//! the verification read returns). The fingerprint catches an edit to any of
//! those fields whether or not it moved the ALTERID; an edit only to a field
//! that read does not return (a reference, an allocation, GST detail, the
//! party ledger) is caught only if it moves the ALTERID, which is not yet
//! measured for an edit made in Tally's own screens.
use super::*;
use crate::tally::approved_import::ReviewAcknowledged;

/// Names the fields [`voucher_fingerprint`] covers, and in which order. A new
/// field is a new version, so a record never matches a fingerprint computed
/// over different fields.
const FINGERPRINT_FIELDS: &str = "v1:guid,master_id,remote_id,date,effective_date,voucher_type,voucher_number,narration,cancelled,optional,entries(ledger,amount,is_deemed_positive)";
const RECORD_VERSION: u32 = 1;

fn masters_ack_path(imports: &Path, batch_id: &str) -> PathBuf {
    imports.join(format!("{batch_id}.masters_ack.json"))
}

/// A batch's masters records, read so that an unreadable file stays
/// distinguishable from a pending check (the verdict path folds the two into
/// one doubt, which is right for it and wrong here).
#[derive(Debug, PartialEq, Eq)]
enum MastersRecord {
    /// No record, or a finished check that observed no doubt.
    NoDoubt,
    Pending,
    Doubt {
        raw: Vec<u8>,
    },
    Unreadable,
}

fn read_masters_record_raw(path: &Path) -> Result<Option<(Vec<u8>, Value)>, ()> {
    let mut file = match super::super::local_file::open_local_file(path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).map_err(|_| ())?;
    let value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    Ok(Some((bytes, value)))
}

fn read_masters_records(imports: &Path, batch_id: &str) -> MastersRecord {
    let Ok(doubt) = read_masters_record_raw(&masters_doubt_path(imports, batch_id)) else {
        return MastersRecord::Unreadable;
    };
    let Ok(check) = read_masters_record_raw(&masters_check_path(imports, batch_id)) else {
        return MastersRecord::Unreadable;
    };
    let pending = check
        .as_ref()
        .is_some_and(|(_, check)| check["state"] == MASTERS_CHECK_PENDING);
    match doubt {
        Some((raw, doubt)) if doubt["state"] == "posted_under_changed_masters" => {
            if pending {
                MastersRecord::Pending
            } else {
                MastersRecord::Doubt { raw }
            }
        }
        // A doubt file holds only that verdict; anything else is not one this
        // build can bind to.
        Some(_) => MastersRecord::Unreadable,
        None if pending => MastersRecord::Pending,
        None => MastersRecord::NoDoubt,
    }
}

/// The sha256 of every field the verification read returns for one voucher,
/// in [`FINGERPRINT_FIELDS`] order, as read (no normalisation).
fn voucher_fingerprint(row: &ReadVoucher) -> String {
    let entries = row
        .entries
        .iter()
        .map(|entry| json!([entry.ledger, entry.amount, entry.is_deemed_positive]))
        .collect::<Vec<_>>();
    sha256_json(&json!([
        FINGERPRINT_FIELDS,
        row.guid,
        row.master_id,
        row.remote_id,
        row.date,
        row.effective_date,
        row.voucher_type,
        row.voucher_number,
        row.narration,
        row.cancelled,
        row.optional,
        entries,
    ]))
}

/// The written record. Unknown fields refuse to parse, so a record from a
/// later format reads as unreadable, never as a match.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AckRecord {
    version: u32,
    batch_id: String,
    company_guid: String,
    voucher_guid: String,
    voucher_master_id: String,
    doubt_sha256: String,
    voucher_fingerprint_sha256: String,
    voucher_fingerprint_fields: String,
    alter_id: u64,
    shown: Value,
    reviewed_at: String,
    /// The OS account's name: a local label, not an identity. Kept in this
    /// file only, never returned or exported.
    local_account_label: Option<String>,
}

/// What the dialog was rendered from, and what the record binds.
#[derive(Debug, PartialEq, Eq)]
struct ReviewSnapshot {
    doubt_raw: Vec<u8>,
    voucher_guid: String,
    voucher_master_id: String,
    alter_id: u64,
    fingerprint: String,
}

/// The one row carrying `line`'s marker, if exactly one does.
fn marked_row<'a>(line: &ImportLedgerLine, rows: &'a [ReadVoucher]) -> Option<&'a ReadVoucher> {
    let voucher = line.vouchers.first()?;
    let tag = line.attribution_tag(voucher);
    let mut marked = rows.iter().filter(|row| {
        row.narration
            .as_deref()
            .and_then(|narration| narration_markers(narration).next().flatten())
            == Some(tag.as_str())
    });
    let row = marked.next()?;
    marked.next().is_none().then_some(row)
}

/// Admit a review only when the observed masters doubt is the sole reason the
/// batch is not verified.
fn admit_review(
    imports: &Path,
    line: &ImportLedgerLine,
    dispatched: bool,
    payload: &Value,
    rows: &[ReadVoucher],
) -> Result<ReviewSnapshot, String> {
    if !dispatched || line.vouchers.len() != 1 {
        return Err("ack_batch_not_posted".into());
    }
    let doubt_raw = match read_masters_records(imports, &line.batch_id) {
        MastersRecord::Doubt { raw } => raw,
        MastersRecord::Pending => return Err("ack_check_pending".into()),
        MastersRecord::Unreadable => return Err("ack_masters_record_unreadable".into()),
        MastersRecord::NoDoubt => return Err("ack_no_observed_doubt".into()),
    };
    let result = &payload["result"];
    if result["dispatch"]["response_state"] != "response_clean" {
        return Err("ack_response_not_clean".into());
    }
    // `posted_verified` already requires the voucher accounting-effective:
    // neither cancelled nor optional (`verify_batch`).
    let row = marked_row(line, rows)
        .filter(|_| verification_status(result, 1) == "posted_verified")
        .ok_or_else(|| "ack_readback_not_matched".to_string())?;
    let (Some(voucher_guid), Some(voucher_master_id), Some(alter_id)) =
        (row.guid.clone(), row.master_id.clone(), row.alter_id)
    else {
        return Err("ack_readback_not_matched".into());
    };
    Ok(ReviewSnapshot {
        doubt_raw,
        voucher_guid,
        voucher_master_id,
        alter_id,
        fingerprint: voucher_fingerprint(row),
    })
}

/// The review dialog's text, under the post dialog's caps and character rules.
fn review_preview(
    batch_id: &str,
    company_name: &str,
    doubt: &Value,
    row: &ReadVoucher,
) -> Result<String, String> {
    let quoted = |text: &str| serde_json::to_string(text).expect("string serialization");
    let ledgers = doubt["ledgers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(quoted)
        .collect::<Vec<_>>()
        .join(", ");
    let entries = row
        .entries
        .iter()
        .map(|entry| {
            let side = if entry.is_deemed_positive.eq_ignore_ascii_case("yes") {
                "Dr"
            } else {
                "Cr"
            };
            format!("{side} {}  {}", entry.amount, quoted(&entry.ledger))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let shown = |value: &Option<String>| {
        value
            .as_deref()
            .map(quoted)
            .unwrap_or_else(|| "(none)".into())
    };
    // Every value read from Tally or the doubt, checked on its own: the
    // preview's own line breaks are layout, a value's are not.
    let text_read = std::iter::once(company_name)
        .chain(
            doubt["ledgers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .chain(
            row.entries
                .iter()
                .flat_map(|entry| [entry.ledger.as_str(), entry.amount.as_str()]),
        )
        .chain(row.narration.as_deref())
        .chain(row.voucher_number.as_deref())
        .chain(row.voucher_type.as_deref())
        .chain(row.date.as_deref());
    if text_read
        .clone()
        .any(post::has_unsafe_review_layout_character)
    {
        return Err("ack_review_layout_text".into());
    }
    if text_read
        .clone()
        .any(post::has_unreviewable_format_character)
    {
        return Err("ack_review_format_text".into());
    }
    let preview = format!(
        "Record that you reviewed ONE {} in {}\nBridge posted it, but these ledgers no longer resolve to the master you approved: {ledgers}\n\nAs it is in Tally now:\nDate: {}  Voucher number: {}\nNarration: {}\n{entries}\nALTERID: {}\nBatch: {}\n\nChoosing \"I reviewed it\" records: \"I reviewed this voucher in Tally. It is correct as it stands.\"\nBridge changes nothing in Tally, and the batch still reads reconciliation_required.",
        row.voucher_type.as_deref().unwrap_or("voucher"),
        quoted(company_name),
        shown(&row.date),
        shown(&row.voucher_number),
        shown(&row.narration),
        row.alter_id.map(|id| id.to_string()).unwrap_or_else(|| "(none)".into()),
        batch_id,
    );
    if preview.chars().count() > 1_600
        || preview.lines().count() > 24
        || preview.lines().any(|line| line.chars().count() > 100)
    {
        return Err("ack_review_too_large".into());
    }
    Ok(preview)
}

/// Place `bytes` at `path` only if nothing is there: staged, synced, then
/// hard-linked, which fails when the name exists. Taking the approval by value
/// means no record can be written without one.
fn write_record_once(
    path: &Path,
    bytes: &[u8],
    _approval: ReviewAcknowledged,
) -> Result<(), String> {
    let staged = path.with_extension(format!("{}.next", Uuid::new_v4()));
    write_private(&staged, bytes)?;
    let linked = fs::hard_link(&staged, path);
    let _ = fs::remove_file(&staged);
    match linked {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("ack_already_recorded".into())
        }
        Err(_) => Err("import_file_write_failed".into()),
    }
}

/// `operator_review` for a readback of a doubted batch: whether a recorded
/// review still covers this doubt and this voucher. `None` when the batch has
/// no observed doubt. It changes no verdict.
pub(super) fn operator_review(
    imports: &Path,
    line: &ImportLedgerLine,
    rows: &[ReadVoucher],
) -> Option<Value> {
    let MastersRecord::Doubt { raw } = read_masters_records(imports, &line.batch_id) else {
        return None;
    };
    let record = match read_masters_record_raw(&masters_ack_path(imports, &line.batch_id)) {
        Ok(None) => return Some(json!({"state":"absent"})),
        Ok(Some((_, value))) => serde_json::from_value::<AckRecord>(value).ok(),
        Err(()) => None,
    };
    let Some(record) = record.filter(|record| record.version == RECORD_VERSION) else {
        return Some(json!({"state":"unreadable"}));
    };
    let covers_doubt = record.batch_id == line.batch_id
        && batch_guid_matches(&line.company_guid, &record.company_guid)
        && record.doubt_sha256 == sha256_hex(&raw);
    let row = marked_row(line, rows);
    let voucher_unchanged = record.voucher_fingerprint_fields == FINGERPRINT_FIELDS
        && row.is_some_and(|row| {
            row.guid.as_deref() == Some(record.voucher_guid.as_str())
                && row.master_id.as_deref() == Some(record.voucher_master_id.as_str())
                && row.alter_id == Some(record.alter_id)
                && voucher_fingerprint(row) == record.voucher_fingerprint_sha256
        });
    Some(json!({
        "state": if covers_doubt && voucher_unchanged { "current" } else { "stale" },
        "reviewed_at": record.reviewed_at,
        "covers_doubt": covers_doubt,
        "voucher_unchanged": voucher_unchanged,
    }))
}

impl Server {
    pub(in crate::agent) async fn acknowledge_post_review(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        let batch_id = required_string(args, "batch_id")?;
        let imports = self.imports_dir()?;
        let ledger::BatchSnapshot {
            batch: line,
            dispatched,
            ..
        } = self
            .latest_import_snapshot(batch_id)?
            .ok_or_else(|| "import_batch_not_found".to_string())?;
        // Refused before any read or dialog: a record already answers it. The
        // path is built from the journal's batch id, never the argument.
        if masters_ack_path(&imports, &line.batch_id).exists() {
            return Err("ack_already_recorded".to_string().into());
        }

        let mut rows = None;
        let first = self.verify_for_review(args, &mut rows).await?;
        let evidence = first.evidence.clone();
        let fail = |code: String| ToolFailure::from(code).with_prior_evidence(evidence.clone());
        let rows = rows.unwrap_or_default();
        let shown =
            admit_review(&imports, &line, dispatched, &first.payload, &rows).map_err(fail)?;
        let row = marked_row(&line, &rows).expect("admitted on this row");
        let doubt: Value = serde_json::from_slice(&shown.doubt_raw).unwrap_or_default();
        let company_name = line
            .company
            .as_ref()
            .map(|company| company.name.clone())
            .unwrap_or_default();
        let preview = review_preview(&line.batch_id, &company_name, &doubt, row).map_err(fail)?;
        let shown_voucher = json!({
            "date": row.date, "voucher_number": row.voucher_number, "narration": row.narration,
            "entries": row.entries.iter().map(|entry| json!({
                "ledger": entry.ledger, "amount": entry.amount,
                "is_deemed_positive": entry.is_deemed_positive,
            })).collect::<Vec<_>>(),
        });

        let approval = ReviewAcknowledged::confirm(&preview).await.map_err(fail)?;

        // What was approved must still be what Tally and the records hold.
        let mut rows_after = None;
        let second = self.verify_for_review(args, &mut rows_after).await?;
        let evidence = combine_evidence(evidence.clone(), second.evidence.clone());
        let fail =
            |code: &str| ToolFailure::from(code.to_string()).with_prior_evidence(evidence.clone());
        let again = admit_review(
            &imports,
            &line,
            dispatched,
            &second.payload,
            &rows_after.unwrap_or_default(),
        );
        if again.as_ref() != Ok(&shown) {
            return Err(fail("ack_changed_while_reviewing"));
        }
        let record = AckRecord {
            version: RECORD_VERSION,
            batch_id: line.batch_id.clone(),
            company_guid: line.company_guid.clone(),
            voucher_guid: shown.voucher_guid.clone(),
            voucher_master_id: shown.voucher_master_id.clone(),
            doubt_sha256: sha256_hex(&shown.doubt_raw),
            voucher_fingerprint_sha256: shown.fingerprint.clone(),
            voucher_fingerprint_fields: FINGERPRINT_FIELDS.to_string(),
            alter_id: shown.alter_id,
            shown: shown_voucher,
            reviewed_at: now(),
            local_account_label: std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .ok(),
        };
        let bytes =
            serde_json::to_vec_pretty(&record).map_err(|_| fail("proof_serialization_failed"))?;
        write_record_once(
            &masters_ack_path(&imports, &line.batch_id),
            &bytes,
            approval,
        )
        .map_err(|code| fail(&code))?;
        Ok(ToolOutcome {
            payload: json!({
                "company": second.payload["company"],
                "result": {
                    "batch_id": line.batch_id,
                    "dispatch": {"state": second.payload["result"]["dispatch"]["state"]},
                    "operator_review": {
                        "state": "current", "reviewed_at": record.reviewed_at,
                        "covers_doubt": true, "voucher_unchanged": true,
                    },
                },
            }),
            evidence,
            company_guid: Some(line.company_guid.clone()),
            truncated: false,
        })
    }
}

#[cfg(test)]
#[path = "agent_import_ack_unit_tests.rs"]
mod tests;
