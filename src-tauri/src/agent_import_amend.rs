//! Amending a batch Bridge built: the same wire identity, under compare-and-swap.
//!
//! A new build draws a fresh random batch identity, so rebuilding a corrected
//! voucher duplicates it. An amendment instead names a batch this Bridge
//! persisted and re-renders the named transactions under that batch's
//! identity, so a file import alters the vouchers already in the book.
//!
//! Tally does this blind: a same-REMOTEID import replaces whatever the voucher
//! holds, including an edit someone made in Tally since. So an amendment is
//! admitted only while every named voucher is still in the book as some build
//! in its lineage wrote it, in the fields compared — the same discipline as `if_version` on a
//! document write. It is checked against the book as read during the build and
//! cannot see an edit made between the build and the hand import.
//!
//! Tally does not echo a client REMOTEID on readback (TALLY_PROTOCOL_REFERENCE
//! §9.3), so the voucher is located by its narration marker, which carries the
//! same batch-derived UUID. A natively posted batch carried that marker beside a
//! random private REMOTEID, so an amendment of it would create a duplicate; any
//! dispatch in the lineage refuses the amendment.
//!
//! The comparison covers the date, a bank voucher's effective date when read,
//! the voucher type, the voucher number when the batch set one, each entry's
//! ledger, amount and side, and the narration. It does not cover `REFERENCE`,
//! bill-wise or cost-centre allocations, or the party ledger, which the
//! verification read does not fetch. An edit to any of them made after Bridge
//! first verified the build is caught instead by the voucher's ALTERID (#239;
//! see `compare_and_swap`); one made before that verification is not.
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
    /// Tally was observed altering a Payment's type and date in place (the
    /// follow-up to bridge#429; not measured on the other types), but a type
    /// change moves a voucher between numbering series
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
    /// voucher is in the book as some build of this lineage wrote it, in the
    /// fields the module doc lists, and has an ALTERID equal to one Bridge
    /// recorded when it first verified such a build; per-voucher refusals
    /// otherwise.
    pub(super) fn compare_and_swap(
        &self,
        vouchers: &[ImportVoucher],
        observed: &ImportReadSource,
        baselines: &VerifiedBaselines,
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
            // Every build whose compared fields the book still matches: a build
            // that changed only an uncompared field (a reference) matches too,
            // whether or not it was ever imported.
            let mut matching = Vec::new();
            for (batch_id, recorded) in self.versions(txn_id) {
                let recorded = canonical_import_voucher(recorded)?;
                let entries_match =
                    expected_entry_fingerprint(&recorded) == actual_entry_fingerprint(&row);
                let mut diffs = voucher_diffs(&recorded, &row, entries_match);
                // The import rewrites the narration too, so an edit to it in
                // Tally is a change the amendment would overwrite.
                if row.narration.as_deref().map(str::trim)
                    != Some(expected_narration(&recorded, &tag).as_str())
                {
                    diffs.push(json!("narration"));
                }
                if diffs.is_empty() {
                    matching.push(batch_id);
                }
                last_diffs = diffs;
            }
            // The fields above are all the read carries. Anything else a person
            // changed (a reference, an allocation) shows only as the voucher's
            // ALTERID moving past one Bridge recorded when it first verified a
            // build (#239). A voucher's ALTERID advances on every alteration
            // (TALLY_PROTOCOL_REFERENCE §9.3, measured over the gateway; an edit
            // in Tally's own screens is not yet measured), so a current value
            // equal to any matching build's baseline means the voucher has not
            // been altered since that reading. A build never imported
            // has no baseline and is passed over. No equal baseline refuses:
            // as altered when some matching build has one, as never verified
            // when none does.
            let equal = matching.iter().copied().find(|batch_id| {
                row.alter_id.is_some() && baselines.alter_id(batch_id, txn_id) == row.alter_id
            });
            let matched = match (matching.last(), equal) {
                (None, _) => None,
                (Some(_), Some(batch_id)) => Some(batch_id),
                (Some(_), None) => {
                    let verified = matching.iter().rev().find_map(|batch_id| {
                        baselines
                            .alter_id(batch_id, txn_id)
                            .map(|alter_id| (*batch_id, alter_id))
                    });
                    refused.push(match verified {
                        Some((batch_id, verified)) => json!({"bridge_txn_id":txn_id,
                            "reason":"voucher_altered_since_verified","book_matches_batch_id":batch_id,
                            "verified_alter_id":verified,"alter_id":row.alter_id,"guid":row.guid}),
                        None => json!({"bridge_txn_id":txn_id,
                            "reason":"voucher_never_verified","book_matches_batch_ids":matching,
                            "alter_id":row.alter_id,"guid":row.guid}),
                    });
                    continue;
                }
            };
            match matched {
                Some(batch_id) => {
                    let mut entry = json!({"bridge_txn_id":txn_id,
                    "book_matches_batch_id":batch_id,"guid":row.guid,"master_id":row.master_id,
                    "alter_id":row.alter_id});
                    // the compare-and-swap could not see an edit to it
                    if row.effective_date.is_none()
                        && row.voucher_type.as_deref() != Some("Journal")
                    {
                        entry["not_observed"] = json!(["effective_date"]);
                    }
                    admitted.push(entry);
                }
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

/// The narration `render_voucher_xml` writes for this voucher under `tag`.
fn expected_narration(voucher: &ImportVoucher, tag: &str) -> String {
    format!(
        "{} [BRIDGE:{tag}]",
        voucher.narration.as_deref().unwrap_or("").trim()
    )
    .trim()
    .to_string()
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

/// The ALTERID each voucher carried the first time Bridge verified it posted,
/// per build (#239). Written once per voucher and never changed afterwards, so
/// a later verify, which still reports a voucher posted after a person edits a
/// field it does not compare, cannot move it.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct VerifiedBaseline {
    pub(super) vouchers: BTreeMap<String, u64>,
}

/// The baselines of every build in a lineage, by batch id.
#[derive(Debug, Default)]
pub(super) struct VerifiedBaselines(pub(super) BTreeMap<String, VerifiedBaseline>);

impl VerifiedBaselines {
    fn alter_id(&self, batch_id: &str, txn_id: &str) -> Option<u64> {
        self.0.get(batch_id)?.vouchers.get(txn_id).copied()
    }
}

/// Add to `existing` each voucher `proof` reports posted_verified with an
/// ALTERID, leaving every voucher already recorded exactly as it was.
pub(super) fn record_first_verified(existing: &mut VerifiedBaseline, proof: &Value) -> bool {
    let mut added = false;
    for voucher in proof["vouchers"].as_array().into_iter().flatten() {
        if voucher["status"] != "posted_verified" {
            continue;
        }
        let (Some(txn_id), Some(alter_id)) = (
            voucher["bridge_txn_id"].as_str(),
            voucher["alter_id"].as_u64(),
        ) else {
            continue;
        };
        if !existing.vouchers.contains_key(txn_id) {
            existing.vouchers.insert(txn_id.to_string(), alter_id);
            added = true;
        }
    }
    added
}
