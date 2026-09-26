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
//! the verification read returns, including its REMOTEID). The fingerprint catches an edit to any of
//! those fields whether or not it moved the ALTERID; an edit only to a field
//! that read does not return (a reference, an allocation, GST detail, the
//! party ledger) is caught only if it moves the ALTERID, which is not yet
//! measured for an edit made in Tally's own screens.
use super::*;
use crate::tally::approved_import::{ReviewAcknowledged, REVIEW_BUTTON};

/// Names the fields [`voucher_fingerprint`] covers, and in which order. A new
/// field is a new version, so a record never matches a fingerprint computed
/// over different fields.
const FINGERPRINT_FIELDS: &str = "v1:guid,master_id,remote_id,date,effective_date,voucher_type,voucher_number,narration,cancelled,optional,entries(ledger,amount,is_deemed_positive)";
const RECORD_VERSION: u32 = 1;

fn masters_ack_path(imports: &Path, batch_id: &str) -> PathBuf {
    imports.join(format!("{batch_id}.masters_ack.json"))
}

/// A batch's review record format: every voucher bound (batch posting, D2b).
const BATCH_RECORD_VERSION: u32 = 2;

/// The doubts a post can record, each reviewed on its own: a review covers
/// only the doubt it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DoubtKind {
    /// The #239 masters check found a ledger now resolving to another master.
    Masters,
    /// Bridge did not confirm that a batch's voucher mark moved by exactly
    /// Tally's CREATED.
    BatchStep,
}

impl DoubtKind {
    fn name(self) -> &'static str {
        match self {
            Self::Masters => "masters",
            Self::BatchStep => "batch_step",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "masters" => Some(Self::Masters),
            "batch_step" => Some(Self::BatchStep),
            _ => None,
        }
    }

    fn ack_path(self, imports: &Path, batch_id: &str) -> PathBuf {
        match self {
            Self::Masters => masters_ack_path(imports, batch_id),
            Self::BatchStep => imports.join(format!("{batch_id}.batch_step_ack.json")),
        }
    }

    /// The doubts a batch of `voucher_count` can hold.
    fn possible(voucher_count: usize) -> &'static [Self] {
        if voucher_count > 1 {
            &[Self::Masters, Self::BatchStep]
        } else {
            &[Self::Masters]
        }
    }

    fn read(self, imports: &Path, batch_id: &str) -> MastersRecord {
        match self {
            Self::Masters => read_masters_records(imports, batch_id),
            Self::BatchStep => read_step_records(imports, batch_id),
        }
    }
}

/// A batch's step records, read as [`read_masters_records`] reads the masters
/// ones: an observed doubt (its own file, `unmatched`), a doubt the check
/// record holds whose own file is absent, a pending step, no doubt, or
/// unreadable.
fn read_step_records(imports: &Path, batch_id: &str) -> MastersRecord {
    let Ok(doubt) = read_masters_record_raw(&batch_step_doubt_path(imports, batch_id)) else {
        return MastersRecord::Unreadable;
    };
    match doubt {
        Some((raw, doubt))
            if doubt["state"] == "unmatched" && doubt.get("target_voucher_step").is_some() =>
        {
            MastersRecord::Doubt { raw }
        }
        Some(_) => MastersRecord::Unreadable,
        None => match read_masters_record_raw(&masters_check_path(imports, batch_id)) {
            Err(()) => MastersRecord::Unreadable,
            Ok(Some((_, check))) if check["batch_step"]["state"] == MASTERS_CHECK_PENDING => {
                MastersRecord::Pending
            }
            Ok(Some((_, check))) if check["batch_step"]["state"] == "unmatched" => {
                MastersRecord::DoubtRecordUnavailable
            }
            Ok(_) => MastersRecord::NoDoubt,
        },
    }
}

/// Which doubt a review is for. Named, it must be one the batch can hold;
/// unnamed, it is the one observed doubt. A doubt the check record holds
/// without its own file is observed too (#722), so it and another doubt need
/// a name (`ack_doubt_ambiguous`), never a silent pick. With none observed,
/// the kind whose record says why (pending, unreadable) is chosen, so the
/// refusal names it.
fn select_doubt(
    requested: Option<DoubtKind>,
    states: &[(DoubtKind, MastersRecord)],
) -> Result<DoubtKind, &'static str> {
    if let Some(kind) = requested {
        return if states.iter().any(|(possible, _)| *possible == kind) {
            Ok(kind)
        } else {
            Err("ack_no_observed_doubt")
        };
    }
    let observed = states
        .iter()
        .filter(|(_, state)| {
            matches!(
                state,
                MastersRecord::Doubt { .. } | MastersRecord::DoubtRecordUnavailable
            )
        })
        .map(|(kind, _)| *kind)
        .collect::<Vec<_>>();
    match observed.as_slice() {
        [kind] => Ok(*kind),
        [] => Ok(states
            .iter()
            .find(|(_, state)| *state != MastersRecord::NoDoubt)
            .map_or(DoubtKind::Masters, |(kind, _)| *kind)),
        _ => Err("ack_doubt_ambiguous"),
    }
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
    /// The check record holds a doubt whose own file is absent, so there are
    /// no doubt bytes to bind a review to (#722). Its write failed, which
    /// marks the verdict `doubt_record: unavailable`, or the file was lost or
    /// removed later, which leaves no mark: absence alone decides.
    DoubtRecordUnavailable,
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
    match doubt {
        // An observed doubt outranks the check record, as it does for the
        // verdict (`read_masters_check`), which then never reads or rewrites
        // that record again: a write that failed between the two can leave it
        // pending or unreadable beside the doubt for good.
        Some((raw, doubt))
            if doubt["state"] == "posted_under_changed_masters"
                && doubt["ledgers"].as_array().is_some_and(|ledgers| {
                    !ledgers.is_empty()
                        && ledgers
                            .iter()
                            .all(|ledger| ledger.as_str().is_some_and(|name| !name.is_empty()))
                }) =>
        {
            MastersRecord::Doubt { raw }
        }
        // A doubt file holds only that verdict, naming each of its ledgers;
        // anything else is not one this build can bind to.
        Some(_) => MastersRecord::Unreadable,
        None => match read_masters_record_raw(&masters_check_path(imports, batch_id)) {
            Err(()) => MastersRecord::Unreadable,
            Ok(Some((_, check))) if check["state"] == MASTERS_CHECK_PENDING => {
                MastersRecord::Pending
            }
            Ok(Some((_, check))) if check["state"] == "posted_under_changed_masters" => {
                MastersRecord::DoubtRecordUnavailable
            }
            Ok(_) => MastersRecord::NoDoubt,
        },
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

/// One reviewed voucher as read, which the record binds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewedVoucher {
    bridge_txn_id: String,
    guid: String,
    master_id: String,
    alter_id: u64,
    fingerprint_sha256: String,
}

/// A batch's written record: the doubt it covers and every voucher. Unknown
/// fields refuse to parse, as for [`AckRecord`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchAckRecord {
    version: u32,
    batch_id: String,
    company_guid: String,
    doubt: String,
    doubt_sha256: String,
    vouchers: Vec<ReviewedVoucher>,
    voucher_fingerprint_fields: String,
    shown: Value,
    reviewed_at: String,
    /// The OS account's name: a local label, not an identity. Kept in this
    /// file only, never returned or exported.
    local_account_label: Option<String>,
}

/// What the dialog was rendered from, and what the record binds: the doubt,
/// and every voucher of the batch, in batch order.
#[derive(Debug, PartialEq, Eq)]
struct ReviewSnapshot {
    doubt_raw: Vec<u8>,
    vouchers: Vec<ReviewedVoucher>,
}

/// The one row carrying `line`'s marker, if exactly one does.
fn marked_row<'a>(line: &ImportLedgerLine, rows: &'a [ReadVoucher]) -> Option<&'a ReadVoucher> {
    marked_row_for(line, line.vouchers.first()?, rows)
}

/// The one row carrying `voucher`'s marker, if exactly one does.
fn marked_row_for<'a>(
    line: &ImportLedgerLine,
    voucher: &ImportVoucher,
    rows: &'a [ReadVoucher],
) -> Option<&'a ReadVoucher> {
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

/// Admit a review only when the named doubt is observed and is the sole
/// reason the batch is not verified: the response clean and every voucher
/// read back, verified and accounting-effective. A batch read back only in
/// part is refused; it is reconciled first.
fn admit_review(
    imports: &Path,
    line: &ImportLedgerLine,
    payload: &Value,
    rows: &[ReadVoucher],
    kind: DoubtKind,
) -> Result<ReviewSnapshot, String> {
    let doubt_raw = match kind.read(imports, &line.batch_id) {
        MastersRecord::Doubt { raw } => raw,
        MastersRecord::Pending => return Err("ack_check_pending".into()),
        MastersRecord::Unreadable => {
            return Err(match kind {
                DoubtKind::Masters => "ack_masters_record_unreadable",
                DoubtKind::BatchStep => "ack_step_record_unreadable",
            }
            .into())
        }
        MastersRecord::NoDoubt => return Err("ack_no_observed_doubt".into()),
        MastersRecord::DoubtRecordUnavailable => return Err("ack_doubt_record_unavailable".into()),
    };
    let result = &payload["result"];
    if result["dispatch"]["response_state"] != "response_clean" {
        return Err("ack_response_not_clean".into());
    }
    // `verify_batch` today reports `posted_verified` only for a voucher that
    // is neither cancelled nor optional. This checks it again here rather than
    // rely on that staying true.
    if verification_status(result, line.vouchers.len()) != "posted_verified" {
        return Err("ack_readback_not_matched".into());
    }
    let vouchers = line
        .vouchers
        .iter()
        .map(|voucher| {
            let row = marked_row_for(line, voucher, rows)
                .filter(|row| voucher_is_accounting_effective(row) == Ok(true))?;
            Some(ReviewedVoucher {
                bridge_txn_id: voucher.bridge_txn_id.clone(),
                guid: row.guid.clone()?,
                master_id: row.master_id.clone()?,
                alter_id: row.alter_id?,
                fingerprint_sha256: voucher_fingerprint(row),
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "ack_readback_not_matched".to_string())?;
    Ok(ReviewSnapshot {
        doubt_raw,
        vouchers,
    })
}

/// The review dialog's text, under the post dialog's caps and character rules.
/// `marker` is this batch's own narration marker (`[BRIDGE:<tag>]`).
fn review_preview(
    batch_id: &str,
    marker: &str,
    company_name: &str,
    doubt: &Value,
    row: &ReadVoucher,
) -> Result<String, String> {
    let preview = render_review_text(batch_id, marker, company_name, doubt, row)?;
    if caps_exceeded(&preview).is_empty() {
        Ok(preview)
    } else {
        Err("ack_review_too_large".into())
    }
}

/// The review text before the caps: every value checked, nothing truncated.
fn render_review_text(
    batch_id: &str,
    marker: &str,
    company_name: &str,
    doubt: &Value,
    row: &ReadVoucher,
) -> Result<String, String> {
    let quoted = |text: &str| serde_json::to_string(text).expect("string serialization");
    // One per line: a changed ledger's name is as long as the book made it.
    let ledgers = doubt["ledgers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|ledger| format!("  {}", quoted(ledger)))
        .collect::<Vec<_>>()
        .join("\n");
    // Only this batch's own marker, exactly where Bridge wrote it (the
    // end), is left to the Batch line. Anything else, including text added
    // after it or another marker, is shown: the record binds all of it.
    let narration = row.narration.as_deref().map(|narration| {
        if narration == marker {
            String::new()
        } else {
            narration
                .strip_suffix(marker)
                .and_then(|text| text.strip_suffix(' '))
                .unwrap_or(narration)
                .to_string()
        }
    });
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
        "Record that you reviewed ONE {} in {}\nBridge posted it, but these ledgers no longer resolve\nto the master you approved:\n{ledgers}\n\nAs it is in Tally now:\nDate: {}  Voucher number: {}  ALTERID: {}\nNarration:\n  {}\n{entries}\nBatch: {}\n\nChoosing \"{REVIEW_BUTTON}\" records: \"I reviewed this voucher in Tally.\nIt is correct as it stands.\" Bridge changes nothing in Tally,\nand the batch still reads reconciliation_required.",
        row.voucher_type.as_deref().unwrap_or("voucher"),
        quoted(company_name),
        shown(&row.date),
        shown(&row.voucher_number),
        row.alter_id.map(|id| id.to_string()).unwrap_or_else(|| "(none)".into()),
        shown(&narration),
        batch_id,
    );
    Ok(preview)
}

/// The review dialog for a batch: a summary of the vouchers as read back
/// from Tally, never a listing, under the batch approval's budget. Refused,
/// never cut, when it does not fit.
fn batch_review_preview(
    line: &ImportLedgerLine,
    kind: DoubtKind,
    company_name: &str,
    doubt: &Value,
    rows: &[&ReadVoucher],
) -> Result<String, String> {
    let quoted = |text: &str| serde_json::to_string(text).expect("string serialization");
    let doubted_ledgers = doubt["ledgers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let text_read = std::iter::once(company_name)
        .chain(doubted_ledgers.iter().copied())
        .chain(rows.iter().flat_map(|row| {
            row.entries
                .iter()
                .flat_map(|entry| [entry.ledger.as_str(), entry.amount.as_str()])
                .chain(row.date.as_deref())
                .chain(row.voucher_type.as_deref())
        }));
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
    let mut ledgers = BTreeMap::<&str, (ExactDecimal, ExactDecimal, usize)>::new();
    for entry in rows.iter().flat_map(|row| &row.entries) {
        let amount = ExactDecimal::parse(entry.amount.clone())
            .map_err(|_| "ack_readback_not_matched".to_string())?;
        let totals = ledgers
            .entry(entry.ledger.as_str())
            .or_insert_with(|| (ExactDecimal::zero(), ExactDecimal::zero(), 0));
        totals.2 += 1;
        // Tally signs a debit negative. The debit total is shown as the
        // negated sum, so a person reads the same figure the post dialog
        // showed, and a line with an unexpected sign still shows as it is.
        let summed = if entry.is_deemed_positive.eq_ignore_ascii_case("yes") {
            totals.0.checked_subtract(&amount).map(|dr| totals.0 = dr)
        } else {
            totals.1.checked_add(&amount).map(|cr| totals.1 = cr)
        };
        summed.map_err(|_| "ack_readback_not_matched".to_string())?;
    }
    let dates = rows.iter().filter_map(|row| row.date.as_deref());
    let alter_ids = rows.iter().filter_map(|row| row.alter_id);
    let mut text = vec![format!(
        "Record that you reviewed {} vouchers in {}",
        rows.len(),
        quoted(company_name)
    )];
    match kind {
        DoubtKind::Masters => {
            text.push("Bridge posted them, but these ledgers no longer resolve".into());
            text.push("to the master you approved:".into());
            text.extend(
                doubted_ledgers
                    .iter()
                    .map(|ledger| format!("  {}", quoted(ledger))),
            );
        }
        DoubtKind::BatchStep => {
            // Each case in its own words: a mark never read, one that went
            // backwards, or an answer that did not parse is never shown as a
            // count. A mark never read records no CREATED, so says nothing of it.
            let step = &doubt["target_voucher_step"];
            let created = step["reported_created"].as_u64().map_or_else(
                || "Tally's answer could not be read".to_string(),
                |created| format!("Tally reported creating {created}"),
            );
            text.push("Bridge posted them, but cannot confirm that only they".into());
            text.push("changed this company's vouchers:".into());
            text.push(
                match (
                    step["before"].as_u64(),
                    step["after"].as_u64(),
                    step["step"].as_u64(),
                ) {
                    (Some(before), Some(after), Some(moved)) => format!(
                        "its voucher mark moved by {moved} (from {before} to {after}); {created}."
                    ),
                    (Some(before), Some(after), None) => format!(
                        "its voucher mark went backwards (from {before} to {after}); {created}."
                    ),
                    _ => "Bridge could not read its voucher mark after posting.".into(),
                },
            );
            text.push("Reviewing these vouchers covers no other voucher in this company.".into());
        }
    }
    text.push(String::new());
    text.push("As they are in Tally now:".into());
    text.push("Not shown here: narrations, voucher numbers and types.".into());
    text.push(format!(
        "Dates: {} to {}  ALTERIDs: {} to {}",
        dates.clone().min().unwrap_or("(none)"),
        dates.max().unwrap_or("(none)"),
        alter_ids
            .clone()
            .min()
            .map_or("(none)".into(), |id| id.to_string()),
        alter_ids.max().map_or("(none)".into(), |id| id.to_string()),
    ));
    for (ledger, (dr, cr, count)) in &ledgers {
        text.push(format!(
            "Dr {}  Cr {}  {count} {}  {}",
            dr.as_str(),
            cr.as_str(),
            if *count == 1 { "entry" } else { "entries" },
            quoted(ledger)
        ));
    }
    text.push(format!("Batch: {}", line.batch_id));
    text.push(String::new());
    text.push(format!(
        "Choosing \"{REVIEW_BUTTON}\" records: \"I reviewed these {} vouchers in Tally.",
        rows.len()
    ));
    text.push("They are correct as they stand.\" Bridge changes nothing in Tally,".into());
    text.push("and the batch still reads reconciliation_required.".into());
    let preview = text.join("\n");
    if text.len() > post::BATCH_REVIEW_MAX_LINES
        || preview.chars().count() > post::BATCH_REVIEW_MAX_CHARS
        || preview.len() > post::BATCH_REVIEW_MAX_BYTES
        || text
            .iter()
            .any(|line| line.chars().count() > post::BATCH_REVIEW_MAX_LINE_CHARS)
    {
        return Err("ack_review_too_large".into());
    }
    Ok(preview)
}

/// Which of the post dialog's caps `preview` exceeds: native message boxes
/// have no scrollable review surface, so a review over any is refused.
fn caps_exceeded(preview: &str) -> Vec<&'static str> {
    let mut exceeded = Vec::new();
    if preview.chars().count() > 1_600 {
        exceeded.push("characters");
    }
    if preview.lines().count() > 24 {
        exceeded.push("lines");
    }
    if preview.lines().any(|line| line.chars().count() > 100) {
        exceeded.push("line_width");
    }
    exceeded
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
        Ok(()) => {
            // Make the new name durable; a record lost to a power failure
            // would read as absent, never as someone else's.
            #[cfg(unix)]
            if let Some(directory) = path.parent() {
                let _ = fs::File::open(directory).and_then(|directory| directory.sync_all());
            }
            Ok(())
        }
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
    if line.vouchers.len() > 1 {
        return batch_operator_review(imports, line, rows);
    }
    let masters = read_masters_records(imports, &line.batch_id);
    let record = read_masters_record_raw(&masters_ack_path(imports, &line.batch_id));
    let raw = match (masters, &record) {
        (MastersRecord::Doubt { raw }, _) => raw,
        // A doubt whose own file is absent can take no review (#722). With a
        // review already recorded, that review reads stale below instead.
        (MastersRecord::DoubtRecordUnavailable, Ok(None)) => {
            return Some(json!({"state":"doubt_record_unavailable"}))
        }
        (_, Ok(None)) => return None,
        // A record whose doubt can no longer be read answers nothing it can
        // be checked against, and says so rather than disappearing.
        (MastersRecord::Unreadable, _) => return Some(json!({"state":"unreadable"})),
        (_, _) => {
            return Some(json!({"state":"stale","covers_doubt":false,"voucher_unchanged":false}))
        }
    };
    let record = match record {
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
        // The fingerprint also covers the GUID and MASTERID today; they are
        // compared on their own as well, and the ALTERID only here.
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

/// `operator_review` for a batch: each kind of doubt reported on its own,
/// `pending` while that kind's verdict is not recorded,
/// `doubt_record_unavailable` where the check record holds a doubt whose own
/// file is absent (#722; a review already recorded then reads stale), and
/// `null` where no doubt of that kind is observed. A review covers only the doubt
/// it names, and only while every voucher it bound is unchanged; a stale
/// review names the vouchers that changed. `None` when no doubt is observed.
fn batch_operator_review(
    imports: &Path,
    line: &ImportLedgerLine,
    rows: &[ReadVoucher],
) -> Option<Value> {
    let mut reviews = serde_json::Map::new();
    let mut any = false;
    for kind in DoubtKind::possible(line.vouchers.len()) {
        let review = match kind.read(imports, &line.batch_id) {
            MastersRecord::Doubt { raw } => {
                any = true;
                batch_review_state(imports, line, rows, *kind, &raw)
            }
            MastersRecord::Unreadable => {
                any = true;
                json!({"state":"unreadable"})
            }
            // A review whose doubt can no longer be read answers nothing, and
            // says so rather than disappearing: a review record outranks a
            // doubt file that is now absent.
            MastersRecord::Pending
            | MastersRecord::NoDoubt
            | MastersRecord::DoubtRecordUnavailable
                if kind.ack_path(imports, &line.batch_id).exists() =>
            {
                any = true;
                json!({"state":"stale","covers_doubt":false,"vouchers_unchanged":false})
            }
            MastersRecord::DoubtRecordUnavailable => {
                any = true;
                json!({"state":"doubt_record_unavailable"})
            }
            MastersRecord::Pending => {
                any = true;
                json!({"state":"pending"})
            }
            MastersRecord::NoDoubt => Value::Null,
        };
        reviews.insert(kind.name().into(), review);
    }
    any.then_some(Value::Object(reviews))
}

fn batch_review_state(
    imports: &Path,
    line: &ImportLedgerLine,
    rows: &[ReadVoucher],
    kind: DoubtKind,
    doubt_raw: &[u8],
) -> Value {
    let record = match read_masters_record_raw(&kind.ack_path(imports, &line.batch_id)) {
        Ok(None) => return json!({"state":"absent"}),
        Ok(Some((_, value))) => serde_json::from_value::<BatchAckRecord>(value).ok(),
        Err(()) => None,
    };
    let Some(record) = record.filter(|record| record.version == BATCH_RECORD_VERSION) else {
        return json!({"state":"unreadable"});
    };
    let covers_doubt = record.batch_id == line.batch_id
        && batch_guid_matches(&line.company_guid, &record.company_guid)
        && record.doubt == kind.name()
        && record.doubt_sha256 == sha256_hex(doubt_raw)
        && record
            .vouchers
            .iter()
            .map(|voucher| voucher.bridge_txn_id.as_str())
            .eq(line
                .vouchers
                .iter()
                .map(|voucher| voucher.bridge_txn_id.as_str()));
    let changed = record
        .vouchers
        .iter()
        .filter(|reviewed| {
            let row = line
                .vouchers
                .iter()
                .find(|voucher| voucher.bridge_txn_id == reviewed.bridge_txn_id)
                .and_then(|voucher| marked_row_for(line, voucher, rows));
            !(record.voucher_fingerprint_fields == FINGERPRINT_FIELDS
                && row.is_some_and(|row| {
                    row.guid.as_deref() == Some(reviewed.guid.as_str())
                        && row.master_id.as_deref() == Some(reviewed.master_id.as_str())
                        && row.alter_id == Some(reviewed.alter_id)
                        && voucher_fingerprint(row) == reviewed.fingerprint_sha256
                }))
        })
        .map(|reviewed| reviewed.bridge_txn_id.clone())
        .collect::<Vec<_>>();
    let vouchers_unchanged = changed.is_empty();
    json!({
        "state": if covers_doubt && vouchers_unchanged { "current" } else { "stale" },
        "reviewed_at": record.reviewed_at,
        "covers_doubt": covers_doubt,
        "vouchers_unchanged": vouchers_unchanged,
        "changed_vouchers": changed,
    })
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
        // Refused before any read or dialog: nothing was posted.
        if !dispatched || line.vouchers.is_empty() {
            return Err("ack_batch_not_posted".to_string().into());
        }
        let requested = match args.get("doubt") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_str()
                    .and_then(DoubtKind::parse)
                    .ok_or_else(|| "ack_doubt_invalid".to_string())?,
            ),
        };
        let states = DoubtKind::possible(line.vouchers.len())
            .iter()
            .map(|kind| (*kind, kind.read(&imports, &line.batch_id)))
            .collect::<Vec<_>>();
        let kind = select_doubt(requested, &states).map_err(|code| {
            let mut failure = ToolFailure::from(code.to_string());
            if code == "ack_doubt_ambiguous" {
                failure.cause = Some("masters_and_batch_step");
            }
            failure
        })?;
        // A record already answers it. The path is built from the journal's
        // batch id, never the argument.
        let ack_path = kind.ack_path(&imports, &line.batch_id);
        if ack_path.exists() {
            return Err("ack_already_recorded".to_string().into());
        }
        let batch = line.vouchers.len() > 1;
        // For a batch, what no read can change is refused before any read: no
        // doubt of the named kind, or a step verdict left pending, which only
        // a post records. A single post keeps its order: read, then refuse.
        if batch {
            let state = states
                .iter()
                .find(|(state_kind, _)| *state_kind == kind)
                .map(|(_, state)| state);
            match (kind, state) {
                (_, Some(MastersRecord::NoDoubt)) => {
                    return Err("ack_no_observed_doubt".to_string().into())
                }
                (_, Some(MastersRecord::DoubtRecordUnavailable)) => {
                    return Err("ack_doubt_record_unavailable".to_string().into())
                }
                (DoubtKind::BatchStep, Some(MastersRecord::Pending)) => {
                    return Err("ack_check_pending".to_string().into())
                }
                _ => {}
            }
        }

        let mut rows = None;
        let first = self.verify_for_review(args, &mut rows).await?;
        let evidence = first.evidence.clone();
        let fail = |code: String| ToolFailure::from(code).with_prior_evidence(evidence.clone());
        let rows = rows.unwrap_or_default();
        let shown = admit_review(&imports, &line, &first.payload, &rows, kind).map_err(fail)?;
        let doubt: Value = serde_json::from_slice(&shown.doubt_raw).unwrap_or_default();
        let company_name = line
            .company
            .as_ref()
            .map(|company| company.name.clone())
            .unwrap_or_default();
        let reviewed_rows = line
            .vouchers
            .iter()
            .filter_map(|voucher| marked_row_for(&line, voucher, &rows))
            .collect::<Vec<_>>();
        let preview = if batch {
            batch_review_preview(&line, kind, &company_name, &doubt, &reviewed_rows)
        } else {
            let marker = format!(
                "{NARRATION_MARKER_PREFIX}{}]",
                line.attribution_tag(&line.vouchers[0])
            );
            review_preview(
                &line.batch_id,
                &marker,
                &company_name,
                &doubt,
                reviewed_rows[0],
            )
        }
        .map_err(fail)?;
        let shown_vouchers = reviewed_rows
            .iter()
            .map(|row| {
                json!({
                    "date": row.date, "voucher_number": row.voucher_number, "narration": row.narration,
                    "entries": row.entries.iter().map(|entry| json!({
                        "ledger": entry.ledger, "amount": entry.amount,
                        "is_deemed_positive": entry.is_deemed_positive,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();

        let approval = ReviewAcknowledged::confirm(&preview).await.map_err(fail)?;

        // What was approved must still be what Tally and the records hold.
        let mut rows_after = None;
        let second = self.verify_for_review(args, &mut rows_after).await?;
        let evidence = combine_evidence(evidence.clone(), second.evidence.clone());
        let fail =
            |code: &str| ToolFailure::from(code.to_string()).with_prior_evidence(evidence.clone());
        let rows_after = rows_after.unwrap_or_default();
        let again = admit_review(&imports, &line, &second.payload, &rows_after, kind);
        if again.as_ref() != Ok(&shown) {
            return Err(fail("ack_changed_while_reviewing"));
        }
        let local_account_label = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .ok();
        // One voucher keeps the single-voucher record exactly; a batch binds
        // every voucher and the doubt it covers.
        let bytes = if batch {
            serde_json::to_vec_pretty(&BatchAckRecord {
                version: BATCH_RECORD_VERSION,
                batch_id: line.batch_id.clone(),
                company_guid: line.company_guid.clone(),
                doubt: kind.name().into(),
                doubt_sha256: sha256_hex(&shown.doubt_raw),
                vouchers: shown.vouchers.clone(),
                voucher_fingerprint_fields: FINGERPRINT_FIELDS.to_string(),
                shown: json!(shown_vouchers),
                reviewed_at: now(),
                local_account_label,
            })
        } else {
            let voucher = &shown.vouchers[0];
            serde_json::to_vec_pretty(&AckRecord {
                version: RECORD_VERSION,
                batch_id: line.batch_id.clone(),
                company_guid: line.company_guid.clone(),
                voucher_guid: voucher.guid.clone(),
                voucher_master_id: voucher.master_id.clone(),
                doubt_sha256: sha256_hex(&shown.doubt_raw),
                voucher_fingerprint_sha256: voucher.fingerprint_sha256.clone(),
                voucher_fingerprint_fields: FINGERPRINT_FIELDS.to_string(),
                alter_id: voucher.alter_id,
                shown: shown_vouchers[0].clone(),
                reviewed_at: now(),
                local_account_label,
            })
        }
        .map_err(|_| fail("proof_serialization_failed"))?;
        write_record_once(&ack_path, &bytes, approval).map_err(|code| fail(&code))?;
        Ok(ToolOutcome {
            payload: json!({
                "company": second.payload["company"],
                "result": {
                    "batch_id": line.batch_id,
                    "dispatch": {"state": second.payload["result"]["dispatch"]["state"]},
                    // Read back as verify_import reports it, not asserted.
                    "operator_review": operator_review(&imports, &line, &rows_after),
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
