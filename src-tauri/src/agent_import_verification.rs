//! Read-back verification of an imported batch.
//!
//! Parses the verification collection, corroborates its window, attributes each
//! observed voucher to the batch by narration marker or accounting fingerprint,
//! classifies every expected voucher, finds duplicates, and renders the proof.
//! Moved out of `agent_import.rs` with only visibility and `super::` paths changed;
//! nothing here dispatches to Tally.
use super::*;

pub(super) fn parse_import_vouchers(
    xml: &str,
    company_guid: &str,
) -> Result<ImportReadSource, String> {
    let parsed =
        super::super::parse_import_verification_rows(xml, company_guid).map_err(|code| {
            match code.as_str() {
                // Preserve the import error contract while sharing scalar admission.
                "agent_read_protocol_invalid"
                    if super::super::validate_agent_envelope(xml).is_err() =>
                {
                    "import_verification_protocol_invalid"
                }
                "agent_read_protocol_invalid"
                | "change_row_core_field_invalid"
                | "voucher_date_invalid"
                | "voucher_effective_date_invalid"
                | "voucher_accounting_state_not_observed" => "import_verification_export_invalid",
                "change_row_identity_invalid"
                | "voucher_source_identity_invalid"
                | "voucher_company_identity_invalid" => "import_verification_identity_invalid",
                "voucher_amount_invalid" => "import_verification_amount_invalid",
                "voucher_master_id_invalid" => "import_verification_master_id_invalid",
                _ => return code,
            }
            .to_string()
        })?;
    let mut rows: Vec<ReadVoucher> = parsed
        .into_iter()
        .map(|row| {
            serde_json::from_value(row)
                .map_err(|_| "import_verification_export_invalid".to_string())
        })
        .collect::<Result<_, _>>()?;
    // The shared parser has validated these lexemes; comparisons use canonical
    // Yes/No while preserving original amount strings for proof output.
    for entry in rows.iter_mut().flat_map(|row| &mut row.entries) {
        entry.is_deemed_positive = entry.is_deemed_positive.trim().to_string();
    }
    ImportReadSource::admit(rows)
}

// File preflight and later verification require the same window and row identities.
pub(super) fn verification_window_identities(
    observed: &ImportReadSource,
    from: &str,
    to: &str,
) -> Result<BTreeSet<(String, u64)>, String> {
    if observed.rows.iter().any(|voucher| {
        voucher
            .date
            .as_deref()
            .is_none_or(|date| date < from || date > to)
    }) {
        return Err("window_not_honoured".to_string());
    }
    observed
        .rows
        .iter()
        .map(|voucher| {
            Ok((
                voucher
                    .guid
                    .clone()
                    .ok_or_else(|| "verification_incomplete:window_not_corroborated".to_string())?,
                voucher
                    .alter_id
                    .ok_or_else(|| "verification_incomplete:window_not_corroborated".to_string())?,
            ))
        })
        .collect()
}

pub(super) fn corroborate_verification_window(
    observed: &ImportReadSource,
    corroboration: &ImportReadSource,
    from: &str,
    to: &str,
) -> Result<(), String> {
    // This collection request has no row limit. The MCP output-page setting
    // cannot establish source truncation; corroborate the observed identity set
    // independently of that presentation cap.
    if verification_window_identities(observed, from, to)?
        != verification_window_identities(corroboration, from, to)?
    {
        return Err("verification_incomplete:window_not_corroborated".to_string());
    }
    Ok(())
}

pub(super) fn canonical_verification_amount(value: &str) -> Result<String, String> {
    ExactDecimal::parse(value.to_string())
        .and_then(|amount| amount.checked_add(&ExactDecimal::zero()))
        .map(|amount| amount.as_str().to_string())
        .map_err(|_| "import_verification_amount_invalid".to_string())
}

type VerificationFingerprint = (Option<String>, Option<String>, Vec<String>);

#[derive(Default)]
struct VerificationCandidates {
    remaining: BTreeSet<usize>,
    after_mark: BTreeSet<usize>,
    consumed: bool,
}

impl VerificationCandidates {
    fn insert(&mut self, index: usize, after_mark: bool) {
        self.remaining.insert(index);
        if after_mark {
            self.after_mark.insert(index);
        }
    }
    fn consume(&mut self, index: usize) {
        self.consumed |= self.remaining.remove(&index);
        self.after_mark.remove(&index);
    }
}

pub(super) fn observed_fingerprint(voucher: &ReadVoucher) -> VerificationFingerprint {
    (
        voucher.date.clone(),
        voucher.voucher_type.clone(),
        actual_entry_fingerprint(voucher),
    )
}

pub(super) fn verify_batch(
    line: &ImportLedgerLine,
    observed: &ImportReadSource,
) -> Result<Value, String> {
    // Normalize only the comparison copies. Persisted batches and generated XML
    // retain their original amount lexemes and remain backward compatible.
    let mut comparison_line = line.clone();
    for entry in comparison_line
        .vouchers
        .iter_mut()
        .flat_map(|v| &mut v.entries)
    {
        entry.amount = canonical_verification_amount(&entry.amount)?;
    }
    let mut comparison_observed = observed.rows.clone();
    for entry in comparison_observed.iter_mut().flat_map(|v| &mut v.entries) {
        entry.amount = canonical_verification_amount(&entry.amount)?;
    }
    let line = &comparison_line;
    let observed = comparison_observed.as_slice();
    let observed_identities = observed
        .iter()
        .map(observed_voucher_identity)
        .collect::<Result<Vec<_>, _>>()?;
    let mut fully_verified_identities = BTreeSet::new();
    let mut rows = Vec::new();
    let expected_fingerprints = line
        .vouchers
        .iter()
        .map(|voucher| {
            (
                normalized_date(&voucher.date).ok(),
                Some(voucher.voucher_type.as_str().to_string()),
                expected_entry_fingerprint(voucher),
            )
        })
        .collect::<Vec<VerificationFingerprint>>();
    let observed_fingerprints = observed
        .iter()
        .map(observed_fingerprint)
        .collect::<Vec<_>>();
    let mut expected_fingerprint_counts = BTreeMap::new();
    for fingerprint in &expected_fingerprints {
        *expected_fingerprint_counts
            .entry(fingerprint)
            .or_insert(0_usize) += 1;
    }
    let expected_markers = line
        .vouchers
        .iter()
        .map(|voucher| line.attribution_tag(voucher))
        .collect::<Vec<_>>();
    let expected_tags = expected_markers
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    // Source admission permits at most one well-formed reserved marker. Parse it
    // once, then reserve expected tags before any fingerprint fallback is used.
    let observed_tags = observed
        .iter()
        .map(|voucher| {
            narration_markers(voucher.narration.as_deref()?)
                .next()
                .flatten()
        })
        .collect::<Vec<_>>();
    let mut tagged = BTreeMap::<&str, VerificationCandidates>::new();
    let mut fallback = BTreeMap::<&VerificationFingerprint, VerificationCandidates>::new();
    for (index, voucher) in observed.iter().enumerate() {
        let after_mark = line
            .pre_import_mark
            .value
            .is_some_and(|mark| voucher.alter_id.is_some_and(|id| id > mark));
        if let Some(tag) = observed_tags[index].filter(|tag| expected_tags.contains(tag)) {
            tagged.entry(tag).or_default().insert(index, after_mark);
        } else {
            fallback
                .entry(&observed_fingerprints[index])
                .or_default()
                .insert(index, after_mark);
        }
    }
    let mut ambiguous_within_batch = Vec::new();
    let mut counts = BTreeMap::from([
        ("posted_verified", 0_u64),
        ("matching_content_observed", 0),
        ("posted_not_effective", 0),
        ("posted_divergent", 0),
        ("not_found", 0),
        ("not_attributable", 0),
        ("duplicate_fingerprint", 0),
    ]);
    for ((expected, expected_key), marker_identity) in line
        .vouchers
        .iter()
        .zip(&expected_fingerprints)
        .zip(&expected_markers)
    {
        let fingerprint_ambiguous_within_batch = expected_fingerprint_counts[expected_key] > 1;
        let tagged_group = tagged.get(marker_identity.as_str());
        let fingerprint_fallback = tagged_group.is_none();
        let group = tagged_group.or_else(|| fallback.get(expected_key));
        let marker = if fingerprint_fallback {
            "accounting_fingerprint"
        } else {
            "narration_tag"
        };
        let not_attributable = line.pre_import_mark.value.is_some()
            && group.is_some_and(|group| !fingerprint_fallback || !group.remaining.is_empty());
        let match_count = group.map_or(0, |group| group.after_mark.len());
        let matched_index = group.and_then(|group| group.after_mark.first().copied());
        let already_consumed = tagged_group.is_some_and(|group| group.consumed);
        let mut value = if not_attributable && match_count == 0 {
            counts
                .entry("not_attributable")
                .and_modify(|count| *count += 1);
            let reason = if already_consumed {
                "observed_voucher_already_attributed"
            } else if marker == "narration_tag" {
                "tag_precedes_pre_import_voucher_mark"
            } else {
                "fingerprint_precedes_pre_import_voucher_mark"
            };
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"not_attributable","marker":marker,"reason":reason})
        } else if match_count == 0 {
            counts.entry("not_found").and_modify(|count| *count += 1);
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"not_found"})
        } else if match_count > 1 {
            counts
                .entry("duplicate_fingerprint")
                .and_modify(|count| *count += 1);
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"duplicate_fingerprint","marker":marker,"matches":match_count})
        } else {
            let matched_index = matched_index.expect("one indexed candidate");
            if let Some(tag) = observed_tags[matched_index] {
                if let Some(group) = tagged.get_mut(tag) {
                    group.consume(matched_index);
                }
            }
            if let Some(group) = fallback.get_mut(&observed_fingerprints[matched_index]) {
                group.consume(matched_index);
            }
            let matched = &observed[matched_index];
            let effective_date_unobserved = effective_date_not_observed(expected, matched);
            let diffs = voucher_diffs(
                expected,
                matched,
                expected_key.2 == observed_fingerprints[matched_index].2,
            );
            let mut matched_value = if fingerprint_fallback {
                counts
                    .entry("matching_content_observed")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"matching_content_observed","marker":marker,"attribution":"not_established","accounting_effective":voucher_is_accounting_effective(matched)?,"diffs":diffs,"voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id,"alter_id":matched.alter_id})
            } else if diffs.is_empty() && voucher_is_accounting_effective(matched)? {
                fully_verified_identities.insert(observed_identities[matched_index].clone());
                counts
                    .entry("posted_verified")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_verified","marker":marker,"voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id,"alter_id":matched.alter_id})
            } else if diffs.is_empty() {
                counts
                    .entry("posted_not_effective")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_not_effective","marker":marker,"reason":"voucher_cancelled_or_optional","voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id,"alter_id":matched.alter_id})
            } else {
                counts
                    .entry("posted_divergent")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_divergent","marker":marker,"diffs":diffs,"voucher_number":matched.voucher_number,"guid":matched.guid,"master_id":matched.master_id})
            };
            if effective_date_unobserved {
                matched_value["not_observed"] = json!(["effective_date"]);
            }
            matched_value
        };
        if fingerprint_fallback && fingerprint_ambiguous_within_batch {
            value["ambiguous_within_batch"] = Value::Bool(true);
            ambiguous_within_batch.push(expected.bridge_txn_id.clone());
        }
        rows.push(value);
    }
    let (batch_duplicates, unrelated_duplicates_in_window) = batch_duplicate_sets(
        observed,
        &observed_identities,
        &observed_fingerprints,
        &expected_fingerprint_counts,
        &observed_tags,
        &expected_tags,
        &fully_verified_identities,
    );
    Ok(
        json!({"counts":counts,"vouchers":rows,"duplicates":batch_duplicates,"unrelated_duplicates_in_window":unrelated_duplicates_in_window,"ambiguous_within_batch":ambiguous_within_batch}),
    )
}

pub(super) fn voucher_is_accounting_effective(voucher: &ReadVoucher) -> Result<bool, String> {
    match (voucher.cancelled, voucher.optional) {
        (Some(false), Some(false)) => Ok(true),
        (Some(true), _) | (_, Some(true)) => Ok(false),
        _ => Err("voucher_accounting_state_not_observed".to_string()),
    }
}

pub(super) fn verification_status(result: &Value, expected_voucher_count: usize) -> &'static str {
    if result["counts"]["posted_verified"].as_u64() == Some(expected_voucher_count as u64)
        && result["duplicates"].as_array().is_some_and(Vec::is_empty)
    {
        "posted_verified"
    } else {
        "verification_incomplete"
    }
}

pub(super) fn batch_duplicate_sets(
    observed: &[ReadVoucher],
    identities: &[String],
    fingerprints: &[VerificationFingerprint],
    expected_fingerprints: &BTreeMap<&VerificationFingerprint, usize>,
    tags: &[Option<&str>],
    expected_tags: &BTreeSet<&str>,
    fully_verified_identities: &BTreeSet<String>,
) -> (Vec<Value>, Vec<Value>) {
    // Serialize the structured vector before hashing: ledger names may contain
    // the delimiters used inside an entry, so joining entries is ambiguous.
    let duplicate_keys = fingerprints.iter().map(sha256_json).collect::<Vec<_>>();
    let all_duplicates = duplicates(observed, identities, &duplicate_keys)
        .into_iter()
        .filter(|duplicate| {
            duplicate["kind"] != "accounting_fingerprint"
                || !duplicate["voucher_ids"].as_array().is_some_and(|ids| {
                    ids.iter().all(|id| {
                        id.as_str()
                            .is_some_and(|id| fully_verified_identities.contains(id))
                    })
                })
        });
    let batch_indexes = (0..observed.len())
        .filter(|&index| {
            tags[index].is_some_and(|tag| expected_tags.contains(tag))
                || expected_fingerprints.contains_key(&fingerprints[index])
        })
        .collect::<Vec<_>>();
    let batch_remote_ids = batch_indexes
        .iter()
        .filter_map(|&index| observed[index].remote_id.as_deref())
        .collect::<BTreeSet<_>>();
    let batch_fingerprints = batch_indexes
        .iter()
        .map(|&index| duplicate_keys[index].as_str())
        .collect::<BTreeSet<_>>();
    let (batch, unrelated): (Vec<_>, Vec<_>) =
        all_duplicates.partition(|duplicate| match duplicate["kind"].as_str() {
            Some("remote_id") => duplicate["remote_id"]
                .as_str()
                .is_some_and(|id| batch_remote_ids.contains(id)),
            Some("accounting_fingerprint") => duplicate["fingerprint"]
                .as_str()
                .is_some_and(|key| batch_fingerprints.contains(key)),
            _ => false,
        });
    let safe_duplicate = |mut duplicate: Value| {
        if let Some(fields) = duplicate.as_object_mut() {
            if let Some(Value::String(fingerprint)) = fields.remove("fingerprint") {
                fields.insert("fingerprint_sha256".into(), json!(fingerprint));
            }
        }
        duplicate
    };
    (
        batch.into_iter().map(safe_duplicate).collect(),
        unrelated.into_iter().map(safe_duplicate).collect(),
    )
}

/// A bank voucher whose readback carried no `EFFECTIVEDATE`: its effective
/// date was written but could not be compared, which a clean status must not hide.
fn effective_date_not_observed(expected: &ImportVoucher, actual: &ReadVoucher) -> bool {
    expected.voucher_type != VoucherType::Journal && actual.effective_date.is_none()
}

pub(super) fn voucher_diffs(
    expected: &ImportVoucher,
    actual: &ReadVoucher,
    entries_match: bool,
) -> Vec<Value> {
    let mut diffs = Vec::new();
    let expected_date = normalized_date(&expected.date).ok();
    if actual.date.as_deref() != expected_date.as_deref() {
        diffs.push(json!("date"));
    }
    // A bank voucher is written with EFFECTIVEDATE equal to DATE (§9.13), and
    // the verification read returns it (§9.8 scoped correction). Only a value
    // that came back and differs is a diff: a response without the element
    // is reported as not observed, never refused.
    if expected.voucher_type != VoucherType::Journal
        && actual
            .effective_date
            .as_deref()
            .is_some_and(|observed| Some(observed) != expected_date.as_deref())
    {
        diffs.push(json!("effective_date"));
    }
    if actual.voucher_type.as_deref() != Some(expected.voucher_type.as_str()) {
        diffs.push(json!("voucher_type"));
    }
    if expected.voucher_number.is_some()
        && actual.voucher_number.as_deref() != expected.voucher_number.as_deref()
    {
        diffs.push(json!("voucher_number"));
    }
    if !entries_match {
        let expected_entries = expected
            .entries
            .iter()
            .map(|entry| {
                json!({"ledger":party_name(&entry.ledger), "amount":entry.amount, "side":entry.side,
                "is_deemed_positive":entry.side.tally_positive()})
            })
            .collect::<Vec<_>>();
        let actual_entries = actual
            .entries
            .iter()
            .map(|entry| {
                json!({"ledger":party_name(&entry.ledger), "amount":entry.amount,
                "is_deemed_positive":entry.is_deemed_positive})
            })
            .collect::<Vec<_>>();
        diffs.push(json!({"entries":{"expected":expected_entries,"observed":actual_entries}}));
    }
    diffs
}

pub(super) fn expected_entry_fingerprint(voucher: &ImportVoucher) -> Vec<String> {
    let mut result = voucher
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{}",
                entry.ledger,
                match entry.side {
                    EntrySide::Dr => format!("-{}", entry.amount),
                    EntrySide::Cr => entry.amount.clone(),
                },
                entry.side.tally_positive()
            )
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}
pub(super) fn actual_entry_fingerprint(voucher: &ReadVoucher) -> Vec<String> {
    let mut result = voucher
        .entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{}",
                entry.ledger, entry.amount, entry.is_deemed_positive
            )
        })
        .collect::<Vec<_>>();
    result.sort();
    result
}

pub(super) fn observed_voucher_identity(voucher: &ReadVoucher) -> Result<String, String> {
    voucher
        .guid
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .map(|id| format!("guid:{}", id.to_ascii_lowercase()))
        .or_else(|| {
            voucher
                .master_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .map(|id| format!("master_id:{id}"))
        })
        .ok_or_else(|| "import_verification_identity_invalid".to_string())
}

pub(super) fn duplicates(
    observed: &[ReadVoucher],
    identities: &[String],
    cached_fingerprints: &[String],
) -> Vec<Value> {
    let mut remote = BTreeMap::<String, BTreeSet<String>>::new();
    let mut fingerprints = BTreeMap::<String, BTreeMap<String, Option<String>>>::new();
    for ((voucher, identity), fingerprint) in
        observed.iter().zip(identities).zip(cached_fingerprints)
    {
        if let Some(id) = voucher
            .remote_id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
        {
            remote
                .entry(id.clone())
                .or_default()
                .insert(identity.clone());
        }
        fingerprints
            .entry(fingerprint.clone())
            .or_default()
            .insert(identity.clone(), voucher.remote_id.clone());
    }
    let mut result = remote.into_iter().filter(|(_, identities)| identities.len() > 1)
        .map(|(remote_id, identities)| json!({"kind":"remote_id","remote_id":remote_id,"count":identities.len()}))
        .collect::<Vec<_>>();
    result.extend(fingerprints.into_iter().filter(|(_, identities)| identities.len() > 1)
        .map(|(fingerprint, identities)| {
            let remote_ids = identities.values().flatten().collect::<BTreeSet<_>>();
            json!({"kind":"accounting_fingerprint","fingerprint":fingerprint,"voucher_ids":identities.keys().collect::<Vec<_>>(),"remote_ids":remote_ids})
        }));
    result
}

pub(super) fn company_high_water_mark(high_water: &Value) -> Result<PreImportMark, String> {
    let voucher = high_water["altvchid"]
        .as_u64()
        .ok_or_else(|| "pre_import_mark_unobserved".to_string())?;
    let master = high_water["altmstid"]
        .as_u64()
        .ok_or_else(|| "pre_import_mark_unobserved".to_string())?;
    Ok(PreImportMark {
        kind: "company_high_water".to_string(),
        value: Some(voucher),
        master_value: Some(master),
    })
}

pub(super) fn alter_id_delta(mark: &PreImportMark, observed: &[ReadVoucher]) -> Value {
    let latest = observed.iter().filter_map(|voucher| voucher.alter_id).max();
    match (mark.value, latest) {
        (Some(before), Some(after)) if after >= before => {
            json!({"before":before,"after_seen":after,"delta":after-before})
        }
        _ => json!({"before":mark.value,"after_seen":latest,"delta":"not_observed"}),
    }
}

pub(super) fn render_proof_markdown(proof: &Value) -> String {
    let mut output = format!(
        "# Voucher import verification — {}\n\n",
        proof["batch_id"].as_str().unwrap_or("unknown")
    );
    let dispatch_state = proof["dispatch"]["state"].as_str();
    if !proof["error"].is_null()
        || (proof.get("dispatch").is_some()
            && !matches!(
                dispatch_state,
                Some("posted_verified" | "previous_attempt_reconciled")
            ))
    {
        output.push_str("**Reconciliation required — this report does not confirm posting.**\n\nA matching voucher readback alone is insufficient. Reconcile the original saved batch; do not rebuild or resend it.\n\n");
    }
    if let Some(state) = dispatch_state {
        output.push_str(&format!(
            "- Dispatch verdict: `{state}`\n- Response state: `{}`\n",
            proof["dispatch"]["response_state"]
                .as_str()
                .unwrap_or("unknown")
        ));
    }
    if let Some(code) = proof["error"]["code"].as_str() {
        output.push_str(&format!("- Error: `{code}`\n"));
    }
    output.push_str(&format!("\n- Company: `{}`\n- Batch SHA-256: `{}`\n- Readback checked: `{}`\n- Readback counts: matching {}, divergent {}, not found {}\n- AlterID delta: `{}`\n- Unrelated duplicates in window: {}\n\n| Transaction | Readback status |\n| --- | --- |\n", proof["company"]["name"].as_str().unwrap_or("unknown"), proof["batch_sha256"].as_str().unwrap_or("unknown"), proof["verified_at"].as_str().unwrap_or("unknown"), proof["counts"]["posted_verified"], proof["counts"]["posted_divergent"], proof["counts"]["not_found"], proof["alter_id_delta"], proof["unrelated_duplicates_in_window"].as_array().map_or(0, Vec::len)));
    for row in proof["vouchers"].as_array().into_iter().flatten() {
        output.push_str(&format!(
            "| {} | {} |\n",
            row["bridge_txn_id"].as_str().unwrap_or("unknown"),
            row["status"].as_str().unwrap_or("unknown")
        ));
    }
    output.push_str(&format!(
        "\nEvidence hashes: company `{}`, voucher read `{}`.\n",
        proof["evidence"]["company"]["response_sha256"]
            .as_str()
            .unwrap_or("unknown"),
        proof["evidence"]["voucher_read_sha256"]
            .as_str()
            .unwrap_or("unknown")
    ));
    output
}
