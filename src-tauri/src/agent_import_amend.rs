//! Amending a batch Bridge built: the same wire identity, under compare-and-swap.
//!
//! A new build draws a fresh random batch identity, so rebuilding a corrected
//! voucher duplicates it. An amendment instead names a batch this Bridge
//! persisted and re-renders the named transactions under that batch's
//! identity, so a file import alters the vouchers already in the book.
//!
//! Tally does this blind: a same-REMOTEID import replaces whatever the voucher
//! holds, including an edit someone made in Tally since. So an amendment is
//! admitted only while every named voucher is still in the book exactly as some
//! build in its lineage wrote it — the same discipline as `if_version` on a
//! document write. It is checked against the book as read during the build and
//! cannot see an edit made between the build and the hand import.
//!
//! Tally does not echo a client REMOTEID on readback (TALLY_PROTOCOL_REFERENCE
//! §9.3), so the voucher is located by its narration marker, which carries the
//! same batch-derived UUID. A natively posted batch carried that marker beside a
//! random private REMOTEID, so an amendment of it would create a duplicate; any
//! dispatch in the lineage refuses the amendment.
use super::*;

/// Every build that shares one wire identity, in journal order.
pub(super) struct Lineage {
    pub(super) identity_batch_id: String,
    pub(super) builds: Vec<ledger::BatchSnapshot>,
}

/// Admit the lineage an amendment names before any voucher is compared.
///
/// `named` is the snapshot of the batch the caller supplied, which may itself
/// be an amendment; `builds` is every snapshot sharing its identity.
pub(super) fn admit_lineage(
    named: &ledger::BatchSnapshot,
    builds: Vec<ledger::BatchSnapshot>,
    company_guid: &str,
    endpoint_origin: &str,
) -> Result<Lineage, String> {
    let identity_batch_id = named.batch.identity_batch_id().to_string();
    if !builds.iter().any(|build| {
        build.batch.batch_id == identity_batch_id && build.batch.amends_batch_id.is_none()
    }) {
        return Err("import_amend_lineage_invalid".into());
    }
    for build in &builds {
        let batch = &build.batch;
        if batch.identity_batch_id() != identity_batch_id {
            return Err("import_amend_lineage_invalid".into());
        }
        if batch.identity_scheme != Some(ImportIdentityScheme::BatchV1) {
            return Err("import_amend_identity_scheme_unsupported".into());
        }
        if !batch_guid_matches(&batch.company_guid, company_guid) {
            return Err("import_batch_company_mismatch".into());
        }
        if batch.endpoint_origin.as_deref() != Some(endpoint_origin) {
            return Err("import_amend_endpoint_mismatch".into());
        }
        // A native post carried the marker beside a random private REMOTEID.
        // Re-importing under the batch identity would create, not alter.
        if build.dispatched {
            return Err("import_amend_natively_posted".into());
        }
    }
    Ok(Lineage {
        identity_batch_id,
        builds,
    })
}

impl Lineage {
    /// Every recorded version of one transaction, oldest first.
    fn versions<'a>(
        &'a self,
        txn_id: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a ImportVoucher)> {
        self.builds.iter().flat_map(move |build| {
            build
                .batch
                .vouchers
                .iter()
                .filter(move |voucher| voucher.bridge_txn_id == txn_id)
                .map(move |voucher| (build.batch.batch_id.as_str(), voucher))
        })
    }

    /// Refuse a proposed amendment that changes what an in-place alteration
    /// was not established to change, before reading the book.
    ///
    /// Tally was observed altering type and date in place (bridge#429's
    /// follow-up), but a type change moves a voucher between numbering series
    /// and cash/bank shapes, and is a different business event rather than a
    /// correction of this one. A supplied voucher number is ignored under
    /// automatic numbering (§9.8), so changing it is not a correction either.
    pub(super) fn admit_proposal(&self, vouchers: &[ImportVoucher]) -> Result<(), String> {
        for voucher in vouchers {
            let mut versions = self.versions(&voucher.bridge_txn_id).peekable();
            if versions.peek().is_none() {
                return Err("import_amend_txn_not_in_batch".into());
            }
            for (_, recorded) in versions {
                if recorded.voucher_type != voucher.voucher_type {
                    return Err("import_amend_voucher_type_changed".into());
                }
                if recorded.voucher_number != voucher.voucher_number {
                    return Err("import_amend_voucher_number_changed".into());
                }
            }
        }
        Ok(())
    }

    /// The read window that holds both where each voucher is now and where the
    /// amendment will put it: an amendment may move a voucher's date.
    pub(super) fn window(&self, vouchers: &[ImportVoucher]) -> (String, String) {
        let dates = vouchers
            .iter()
            .flat_map(|voucher| {
                std::iter::once(voucher.date.clone()).chain(
                    self.versions(&voucher.bridge_txn_id)
                        .map(|(_, recorded)| recorded.date.clone()),
                )
            })
            .collect::<BTreeSet<_>>();
        (
            dates.first().cloned().unwrap_or_default(),
            dates.last().cloned().unwrap_or_default(),
        )
    }

    /// The compare-and-swap. Returns per-voucher evidence when every amended
    /// voucher is in the book exactly as some build of this lineage wrote it,
    /// and per-voucher refusals otherwise.
    pub(super) fn compare_and_swap(
        &self,
        vouchers: &[ImportVoucher],
        observed: &ImportReadSource,
    ) -> Result<Result<Vec<Value>, Vec<Value>>, String> {
        let mut admitted = Vec::new();
        let mut refused = Vec::new();
        for voucher in vouchers {
            let txn_id = voucher.bridge_txn_id.as_str();
            let tag = import_identity(&self.identity_batch_id, txn_id).to_string();
            // Source admission already refuses a row with two markers and a
            // marker on two rows, so at most one row can carry this tag.
            let Some(row) = observed.rows.iter().find(|row| {
                row.narration
                    .as_deref()
                    .and_then(|narration| narration_markers(narration).next().flatten())
                    == Some(tag.as_str())
            }) else {
                refused.push(json!({"bridge_txn_id":txn_id,"reason":"not_in_book"}));
                continue;
            };
            if !voucher_is_accounting_effective(row)? {
                refused.push(
                    json!({"bridge_txn_id":txn_id,"reason":"voucher_cancelled_or_optional",
                    "guid":row.guid,"alter_id":row.alter_id}),
                );
                continue;
            }
            let row = canonical_read_voucher(row)?;
            let mut last_diffs = Vec::new();
            let mut matched = None;
            for (batch_id, recorded) in self.versions(txn_id) {
                let recorded = canonical_import_voucher(recorded)?;
                let entries_match =
                    expected_entry_fingerprint(&recorded) == actual_entry_fingerprint(&row);
                let diffs = voucher_diffs(&recorded, &row, entries_match);
                if diffs.is_empty() {
                    matched = Some(batch_id);
                }
                last_diffs = diffs;
            }
            match matched {
                Some(batch_id) => admitted.push(json!({"bridge_txn_id":txn_id,
                    "book_matches_batch_id":batch_id,"guid":row.guid,"master_id":row.master_id,
                    "alter_id":row.alter_id})),
                None => refused.push(
                    json!({"bridge_txn_id":txn_id,"reason":"book_voucher_diverged",
                    "diffs_from_latest_build":last_diffs,"guid":row.guid,"alter_id":row.alter_id}),
                ),
            }
        }
        Ok(if refused.is_empty() {
            Ok(admitted)
        } else {
            Err(refused)
        })
    }
}

fn canonical_import_voucher(voucher: &ImportVoucher) -> Result<ImportVoucher, String> {
    let mut voucher = voucher.clone();
    for entry in &mut voucher.entries {
        entry.amount = canonical_verification_amount(&entry.amount)?;
    }
    Ok(voucher)
}

fn canonical_read_voucher(voucher: &ReadVoucher) -> Result<ReadVoucher, String> {
    let mut voucher = voucher.clone();
    for entry in &mut voucher.entries {
        entry.amount = canonical_verification_amount(&entry.amount)?;
    }
    Ok(voucher)
}
