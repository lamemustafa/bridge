use super::{
    combine_evidence, company_json, normalized_date, parse_company_high_water, party_name,
    render_agent_company_high_water, required_string, sha256_hex, sha256_json, Evidence, Server,
    ToolOutcome,
};
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::parse_ledgers;
use bridge_tally_protocol::xml_read_profiles::{ReadOnlyProfile, ValidatedCompanyName};
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

const MAX_VOUCHERS: usize = 1_000;
const MAX_TEXT_BYTES: usize = 2_000;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImportPayload {
    company_guid: String,
    vouchers: Vec<ImportVoucher>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImportVoucher {
    bridge_txn_id: String,
    date: String,
    voucher_type: VoucherType,
    #[serde(default)]
    narration: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    voucher_number: Option<String>,
    entries: Vec<ImportEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum VoucherType {
    Payment,
    Receipt,
    Journal,
    Contra,
}

impl VoucherType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Payment => "Payment",
            Self::Receipt => "Receipt",
            Self::Journal => "Journal",
            Self::Contra => "Contra",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum EntrySide {
    Dr,
    Cr,
}

impl EntrySide {
    fn tally_positive(&self) -> &'static str {
        match self {
            Self::Dr => "Yes",
            Self::Cr => "No",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ImportEntry {
    ledger: String,
    amount: String,
    side: EntrySide,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ImportLedgerLine {
    batch_id: String,
    company_guid: String,
    #[serde(default)]
    company: Option<ImportCompanyTuple>,
    txn_ids: Vec<String>,
    date_from: String,
    date_to: String,
    sha256: String,
    built_at: String,
    status: String,
    pre_import_mark: PreImportMark,
    vouchers: Vec<ImportVoucher>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct ImportCompanyTuple {
    name: String,
    guid: String,
    company_number: String,
    books_from: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PreImportMark {
    kind: String,
    value: Option<u64>,
    #[serde(default)]
    master_value: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadEntry {
    ledger: String,
    amount: String,
    is_deemed_positive: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReadVoucher {
    remote_id: Option<String>,
    guid: Option<String>,
    alter_id: Option<u64>,
    date: Option<String>,
    voucher_type: Option<String>,
    narration: Option<String>,
    voucher_number: Option<String>,
    master_id: Option<String>,
    cancelled: Option<bool>,
    optional: Option<bool>,
    entries: Vec<ReadEntry>,
}

impl Server {
    pub(super) fn voucher_schema(&self) -> Result<ToolOutcome, String> {
        let schema = voucher_input_schema();
        Ok((
            json!({"result": {"schema": schema, "rules": [
                "bridge_txn_id is client-supplied, unique, 1-64 ASCII characters from [A-Za-z0-9_-]",
                "each voucher has at least two entries and exact debit total equals credit total",
                "amounts are positive decimal strings with exactly two fractional digits",
                "dates must be within the selected company's BOOKSFROM through today",
                "ledger names must exactly match the live catalogue; validate_masters before build_import_xml"
            ], "limits": {"education_mode_date_restriction": "If the connected Tally is Education mode, only days 1, 2, and 31 are observed safe; this server does not infer licence mode."}}}),
            local_evidence("voucher_schema"),
            None,
            0,
            vec!["schema".into(), "rules".into()],
            false,
        ))
    }

    pub(super) async fn validate_masters(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let ledgers = args
            .get("ledgers")
            .and_then(Value::as_array)
            .ok_or_else(|| "ledgers_required".to_string())?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()
            .filter(|names| !names.is_empty())
            .ok_or_else(|| "ledgers_required".to_string())?;
        let (company, identity, identity_evidence) = self.verified_company(guid).await?;
        let (catalogue, evidence) = self.read_ledger_catalogue(&identity, &company.name).await?;
        let report = ledgers
            .into_iter()
            .map(|wanted| master_match(wanted, &catalogue))
            .collect::<Vec<_>>();
        let reported_masters = report.len();
        let hash = sha256_json(&catalogue);
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"masters": report, "catalogue_evidence_sha256": hash}}),
            combine_evidence(identity_evidence, evidence),
            Some(guid.to_string()),
            reported_masters,
            vec![
                "name".into(),
                "match_state".into(),
                "catalogue_evidence_sha256".into(),
            ],
            false,
        ))
    }

    pub(super) async fn build_import_xml(&self, args: &Value) -> Result<ToolOutcome, String> {
        let mut payload = parse_payload(args)?;
        validate_payload(&payload)?;
        normalize_payload_dates(&mut payload)?;
        let (company, identity, identity_evidence) =
            self.verified_company(&payload.company_guid).await?;
        let (catalogue, catalogue_evidence) =
            self.read_ledger_catalogue(&identity, &company.name).await?;
        let report = masters_for_payload(&payload, &catalogue);
        if report.iter().any(|value| value["match_state"] != "exact") {
            return Ok((
                json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {
                    "state":"refused", "reason":"masters_not_exact", "masters":report,
                    "catalogue_evidence_sha256":sha256_json(&catalogue),
                    "next_step":"Use the exact live spelling from validate_masters, then build a new batch. No file was written."
                }}),
                combine_evidence(identity_evidence, catalogue_evidence),
                Some(payload.company_guid),
                0,
                vec!["match_state".into(), "catalogue_evidence_sha256".into()],
                false,
            ));
        }
        validate_dates(&payload, company.books_from.as_deref())?;
        let _admission_lock = self.lock_import_admission()?;
        let existing = self.import_ledger()?;
        reject_known_transactions(&payload, &existing)?;
        let (mark, mark_evidence) = self.pre_import_mark(&company, &identity).await?;
        let batch_id = format!("bridge-{}", Uuid::new_v4());
        let xml = render_import_xml(&company.name, &payload.vouchers);
        let sha256 = sha256_hex(xml.as_bytes());
        let date_from = payload
            .vouchers
            .iter()
            .map(|voucher| voucher.date.clone())
            .min()
            .unwrap_or_default();
        let date_to = payload
            .vouchers
            .iter()
            .map(|voucher| voucher.date.clone())
            .max()
            .unwrap_or_default();
        let line = ImportLedgerLine {
            batch_id: batch_id.clone(),
            company_guid: canonical_batch_guid(&payload.company_guid),
            company: Some(import_company_tuple(&company)?),
            txn_ids: payload
                .vouchers
                .iter()
                .map(|voucher| voucher.bridge_txn_id.clone())
                .collect(),
            date_from,
            date_to,
            sha256: sha256.clone(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: mark,
            vouchers: payload.vouchers,
        };
        let path = self.imports_dir()?.join(format!("{batch_id}.xml"));
        write_private(&path, xml.as_bytes())?;
        if let Err(error) = self.append_import_ledger_while_admitted(&line) {
            return Err(remove_orphaned_import_file(&path, error));
        }
        let (debit, credit) = totals(&line.vouchers)?;
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {
                "batch_id": batch_id, "path": path, "sha256": sha256,
                "voucher_count": line.vouchers.len(), "total_debit": debit.as_str(), "total_credit": credit.as_str(),
                "live_evidence": "none_recorded",
                "live_evidence_report": "docs/agent/GOAL2-REPORT.md",
                "warnings": ["No XML was sent to Tally. Import the written file manually, then use verify_import."],
                "next_step": "Import this file in Tally (Gateway of Tally → Import → Vouchers) with the company open, then call verify_import"
            }}),
            combine_evidence(
                combine_evidence(identity_evidence, catalogue_evidence),
                mark_evidence,
            ),
            Some(line.company_guid.clone()),
            line.vouchers.len(),
            vec!["batch_id".into(), "path".into(), "sha256".into()],
            false,
        ))
    }

    pub(super) async fn verify_import(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let batch_id = required_string(args, "batch_id")?;
        let line = self
            .latest_import_line(batch_id)?
            .ok_or_else(|| "import_batch_not_found".to_string())?;
        if !batch_guid_matches(&line.company_guid, guid) {
            return Err("import_batch_company_mismatch".to_string());
        }
        let (company, identity, identity_evidence) = self.verified_company(guid).await?;
        if line.company.as_ref() != Some(&import_company_tuple(&company)?) {
            return Err("company_identity_mismatch".to_string());
        }
        let request =
            render_import_verification_read(&company.name, &line.date_from, &line.date_to);
        let (xml, evidence) = self.post_read(&identity, request.clone()).await?;
        let observed = parse_import_vouchers(&xml)?;
        let (corroboration_xml, corroboration_evidence) =
            self.post_read(&identity, request).await?;
        let corroboration = parse_import_vouchers(&corroboration_xml)?;
        corroborate_verification_window(
            &observed,
            &corroboration,
            &line.date_from,
            &line.date_to,
            self.settings.max_rows,
            verification_read_possibly_truncated(&xml)
                || verification_read_possibly_truncated(&corroboration_xml),
        )?;
        let result = verify_batch(&line, &observed)?;
        let proof = json!({
            "company": company_json(&company, std::slice::from_ref(&company)),
            "batch_id": line.batch_id, "batch_sha256": line.sha256,
            "built_at": line.built_at, "verified_at": now(),
            "pre_import_mark": line.pre_import_mark, "alter_id_delta": alter_id_delta(&line.pre_import_mark, &observed),
            "counts": result["counts"], "vouchers": result["vouchers"], "duplicates": result["duplicates"],
            "unrelated_duplicates_in_window": result["unrelated_duplicates_in_window"],
            "evidence": {"company": identity_evidence, "voucher_read": evidence, "voucher_read_corroboration": corroboration_evidence, "voucher_read_sha256": sha256_hex(xml.as_bytes())}
        });
        let imports = self.imports_dir()?;
        write_private(
            &imports.join(format!("{batch_id}.proof.json")),
            serde_json::to_vec_pretty(&proof)
                .map_err(|_| "proof_serialization_failed".to_string())?
                .as_slice(),
        )?;
        write_private(
            &imports.join(format!("{batch_id}.proof.md")),
            render_proof_markdown(&proof).as_bytes(),
        )?;
        let status = verification_status(&result, line.vouchers.len());
        let mut update = line.clone();
        update.status = status.to_string();
        self.append_import_ledger(&update)?;
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": proof}),
            combine_evidence(
                combine_evidence(identity_evidence, evidence),
                corroboration_evidence,
            ),
            Some(guid.to_string()),
            line.vouchers.len(),
            vec!["remote_id".into(), "alter_id".into(), "entries".into()],
            false,
        ))
    }

    async fn read_ledger_catalogue(
        &self,
        identity: &super::VerifiedCompanyIdentity,
        company_name: &str,
    ) -> Result<(Vec<String>, Evidence), String> {
        let name = ValidatedCompanyName::new(company_name.to_string())
            .map_err(|_| "company_name_invalid".to_string())?;
        let (xml, evidence) = self
            .post_read(
                identity,
                ReadOnlyProfile::LedgersV1 { company: &name }.render(),
            )
            .await?;
        let ledgers = parse_ledgers(&xml).map_err(|_| "ledger_export_invalid".to_string())?;
        Ok((
            ledgers.into_iter().map(|ledger| ledger.name).collect(),
            evidence,
        ))
    }

    async fn pre_import_mark(
        &self,
        company: &bridge_tally_protocol::TallyCompany,
        identity: &super::VerifiedCompanyIdentity,
    ) -> Result<(PreImportMark, Evidence), String> {
        let guid = company
            .guid
            .as_deref()
            .ok_or_else(|| "pre_import_mark_unobserved".to_string())?;
        let (xml, evidence) = self
            .post_read(identity, render_agent_company_high_water(&company.name))
            .await?;
        let high_water = parse_company_high_water(&xml, guid)
            .map_err(|_| "pre_import_mark_unobserved".to_string())?;
        Ok((company_high_water_mark(&high_water)?, evidence))
    }

    fn imports_dir(&self) -> Result<PathBuf, String> {
        let path = self.settings.data_dir.join("imports");
        fs::create_dir_all(&path).map_err(|_| "imports_dir_unavailable".to_string())?;
        set_private_dir(&path)?;
        Ok(path)
    }

    fn lock_import_admission(&self) -> Result<std::fs::File, String> {
        let path = self.settings.data_dir.join("agent-import-admission.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|_| "import_admission_lock_unavailable".to_string())?;
        file.lock_exclusive()
            .map_err(|_| "import_admission_lock_unavailable".to_string())?;
        Ok(file)
    }

    fn import_ledger(&self) -> Result<Vec<ImportLedgerLine>, String> {
        let path = self.settings.data_dir.join("agent-import-ledger.jsonl");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(_) => return Err("import_ledger_unavailable".to_string()),
        };
        text.lines()
            .map(|line| serde_json::from_str(line).map_err(|_| "import_ledger_invalid".to_string()))
            .collect()
    }

    fn latest_import_line(&self, batch_id: &str) -> Result<Option<ImportLedgerLine>, String> {
        Ok(self
            .import_ledger()?
            .into_iter()
            .rfind(|line| line.batch_id == batch_id))
    }

    fn append_import_ledger(&self, line: &ImportLedgerLine) -> Result<(), String> {
        let _admission_lock = self.lock_import_admission()?;
        self.append_import_ledger_while_admitted(line)
    }

    fn append_import_ledger_while_admitted(&self, line: &ImportLedgerLine) -> Result<(), String> {
        let path = self.settings.data_dir.join("agent-import-ledger.jsonl");
        let encoded = serde_json::to_string(line)
            .map_err(|_| "import_ledger_serialization_failed".to_string())?;
        let encoded = format!("{encoded}\n");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|_| "import_ledger_unavailable".to_string())?;
        file.write_all(encoded.as_bytes())
            .map_err(|_| "import_ledger_unavailable".to_string())?;
        file.sync_data()
            .map_err(|_| "import_ledger_unavailable".to_string())?;
        set_private(&path)
    }
}

fn remove_orphaned_import_file(path: &Path, append_error: String) -> String {
    match fs::remove_file(path) {
        Ok(()) => append_error,
        Err(_) => format!("import_file_orphaned:{}", path.display()),
    }
}

fn canonical_batch_guid(guid: &str) -> String {
    guid.to_ascii_lowercase()
}

fn batch_guid_matches(stored: &str, supplied: &str) -> bool {
    stored.eq_ignore_ascii_case(supplied)
}

pub(super) fn voucher_input_schema() -> Value {
    json!({"type":"object", "additionalProperties":false, "required":["company_guid","vouchers"], "properties": {
        "company_guid":{"type":"string","minLength":1}, "vouchers":{"type":"array","minItems":1,"items":{"type":"object","additionalProperties":false,"required":["bridge_txn_id","date","voucher_type","entries"],"properties": {
        "bridge_txn_id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,64}$"}, "date":{"type":"string","pattern":"^\\d{4}-\\d{2}-\\d{2}$"}, "voucher_type":{"enum":["Payment","Receipt","Journal","Contra"]}, "narration":{"type":"string"}, "reference":{"type":"string"}, "voucher_number":{"type":"string","minLength":1,"maxLength":32}, "entries":{"type":"array","minItems":2,"items":{"type":"object","additionalProperties":false,"required":["ledger","amount","side"],"properties":{"ledger":{"type":"string","minLength":1},"amount":{"type":"string","pattern":"^\\d+\\.\\d{2}$"},"side":{"enum":["Dr","Cr"]}}}} }}} }})
}

fn parse_payload(args: &Value) -> Result<ImportPayload, String> {
    serde_json::from_value(args.clone()).map_err(|_| "voucher_schema_invalid".to_string())
}

fn import_company_tuple(
    company: &bridge_tally_protocol::TallyCompany,
) -> Result<ImportCompanyTuple, String> {
    Ok(ImportCompanyTuple {
        name: nonempty_company_field(&company.name)?,
        guid: nonempty_company_field(
            company
                .guid
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?,
        company_number: nonempty_company_field(
            company
                .company_number
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?,
        books_from: normalized_date(
            company
                .books_from
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?,
    })
}

fn nonempty_company_field(value: &str) -> Result<String, String> {
    (!value.trim().is_empty())
        .then(|| value.to_string())
        .ok_or_else(|| "company_identity_incomplete".to_string())
}

fn validate_payload(payload: &ImportPayload) -> Result<(), String> {
    if payload.company_guid.trim().is_empty()
        || payload.vouchers.is_empty()
        || payload.vouchers.len() > MAX_VOUCHERS
    {
        return Err("voucher_count_invalid".to_string());
    }
    let mut txn_ids = BTreeSet::new();
    for voucher in &payload.vouchers {
        if !valid_txn_id(&voucher.bridge_txn_id) || !txn_ids.insert(&voucher.bridge_txn_id) {
            return Err("bridge_txn_id_invalid_or_duplicate".to_string());
        }
        normalized_date(&voucher.date)?;
        if voucher.entries.len() < 2 {
            return Err("voucher_entries_too_few".to_string());
        }
        for text in [
            voucher.narration.as_deref(),
            voucher.reference.as_deref(),
            voucher.voucher_number.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if text.is_empty() || text.len() > MAX_TEXT_BYTES || text.chars().any(char::is_control)
            {
                return Err("voucher_text_invalid".to_string());
            }
        }
        if voucher
            .voucher_number
            .as_deref()
            .is_some_and(|number| number.len() > 32 || number.contains('$'))
        {
            return Err("voucher_number_invalid".to_string());
        }
        for entry in &voucher.entries {
            if entry.ledger.trim().is_empty()
                || entry.ledger.chars().any(char::is_control)
                || !valid_2dp_amount(&entry.amount)
            {
                return Err("voucher_entry_invalid".to_string());
            }
        }
        let (debit, credit) = totals(std::slice::from_ref(voucher))?;
        if !debit.numeric_eq(&credit) {
            return Err("voucher_not_balanced".to_string());
        }
    }
    Ok(())
}

/// Dates cross the tool boundary in the human-friendly form but are persisted
/// in the exact Tally form used in the generated XML and verification window.
fn normalize_payload_dates(payload: &mut ImportPayload) -> Result<(), String> {
    for voucher in &mut payload.vouchers {
        voucher.date = normalized_date(&voucher.date)?;
    }
    Ok(())
}

fn validate_dates(payload: &ImportPayload, books_from: Option<&str>) -> Result<(), String> {
    let from =
        normalized_date(books_from.ok_or_else(|| "company_identity_incomplete".to_string())?)?;
    let today = super::tally_host_today();
    for voucher in &payload.vouchers {
        let date = normalized_date(&voucher.date)?;
        if date < from || date > today {
            return Err("voucher_date_outside_company_extent".to_string());
        }
    }
    Ok(())
}

fn valid_txn_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}
fn valid_2dp_amount(value: &str) -> bool {
    let Some((whole, fractional)) = value.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fractional.len() == 2
        && fractional.bytes().all(|byte| byte.is_ascii_digit())
        && ExactDecimal::parse(value.to_string())
            .is_ok_and(|amount| !amount.is_zero() && !amount.is_negative())
}

fn totals(vouchers: &[ImportVoucher]) -> Result<(ExactDecimal, ExactDecimal), String> {
    let mut debit = ExactDecimal::zero();
    let mut credit = ExactDecimal::zero();
    for entry in vouchers.iter().flat_map(|voucher| &voucher.entries) {
        let amount = ExactDecimal::parse(entry.amount.clone())
            .map_err(|_| "voucher_amount_invalid".to_string())?;
        match entry.side {
            EntrySide::Dr => {
                debit = debit
                    .checked_add(&amount)
                    .map_err(|_| "voucher_amount_overflow".to_string())?
            }
            EntrySide::Cr => {
                credit = credit
                    .checked_add(&amount)
                    .map_err(|_| "voucher_amount_overflow".to_string())?
            }
        }
    }
    Ok((debit, credit))
}

fn masters_for_payload(payload: &ImportPayload, catalogue: &[String]) -> Vec<Value> {
    let mut names = BTreeSet::new();
    for entry in payload.vouchers.iter().flat_map(|voucher| &voucher.entries) {
        names.insert(entry.ledger.as_str());
    }
    names
        .into_iter()
        .map(|name| master_match(name, catalogue))
        .collect()
}

fn master_match(wanted: &str, catalogue: &[String]) -> Value {
    if catalogue.iter().any(|name| name == wanted) {
        return json!({"requested": party_name(wanted), "match_state":"exact", "exact_live_spelling":party_name(wanted)});
    }
    let key = master_key(wanted);
    let mut candidates = catalogue
        .iter()
        .filter(|name| {
            let candidate = master_key(name);
            candidate == key || candidate.starts_with(&key) || key.starts_with(&candidate)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    if candidates.is_empty() {
        json!({"requested":party_name(wanted),"match_state":"missing"})
    } else {
        let candidates = candidates.into_iter().map(party_name).collect::<Vec<_>>();
        json!({"requested":party_name(wanted),"match_state":"near_miss","exact_live_spelling":candidates.first(),"candidates":candidates})
    }
}

fn master_key(value: &str) -> String {
    value
        .nfc()
        .flat_map(|character| match character {
            '–' | '—' | '−' | '‐' | '‑' => "-".chars().collect::<Vec<_>>(),
            '‘' | '’' | '‚' | '‛' => "'".chars().collect(),
            '“' | '”' | '„' | '‟' => "\"".chars().collect(),
            other => other.to_lowercase().collect(),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn reject_known_transactions(
    payload: &ImportPayload,
    lines: &[ImportLedgerLine],
) -> Result<(), String> {
    let known = lines
        .iter()
        .flat_map(|line| &line.txn_ids)
        .collect::<BTreeSet<_>>();
    if payload
        .vouchers
        .iter()
        .any(|voucher| known.contains(&voucher.bridge_txn_id))
    {
        Err("bridge_txn_id_already_built".to_string())
    } else {
        Ok(())
    }
}

fn render_import_xml(company: &str, vouchers: &[ImportVoucher]) -> String {
    let messages = vouchers.iter().map(render_voucher_xml).collect::<String>();
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ENVELOPE><HEADER><TALLYREQUEST>Import Data</TALLYREQUEST></HEADER><BODY><IMPORTDATA><REQUESTDESC><REPORTNAME>Vouchers</REPORTNAME><STATICVARIABLES><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES></REQUESTDESC><REQUESTDATA>{messages}</REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>", xml_escape(company))
}

fn render_voucher_xml(voucher: &ImportVoucher) -> String {
    let narration = format!(
        "<NARRATION>{}</NARRATION>",
        xml_escape(
            format!(
                "{} [BRIDGE:{}]",
                voucher.narration.as_deref().unwrap_or("").trim(),
                voucher.bridge_txn_id
            )
            .trim(),
        )
    );
    // REFERENCE is retained because it is part of the agent input contract. Its effect is not used as posting evidence; verify_import compares the accounting entries, not this annotation.
    let reference = voucher
        .reference
        .as_deref()
        .map(|value| format!("<REFERENCE>{}</REFERENCE>", xml_escape(value)))
        .unwrap_or_default();
    let voucher_number = voucher
        .voucher_number
        .as_deref()
        .map(|value| format!("<VOUCHERNUMBER>{}</VOUCHERNUMBER>", xml_escape(value)))
        .unwrap_or_default();
    let entries = voucher.entries.iter().map(|entry| {
        let amount = match entry.side { EntrySide::Dr => format!("-{}", entry.amount), EntrySide::Cr => entry.amount.clone() };
        format!("<ALLLEDGERENTRIES.LIST><LEDGERNAME>{}</LEDGERNAME><ISDEEMEDPOSITIVE>{}</ISDEEMEDPOSITIVE><AMOUNT>{}</AMOUNT></ALLLEDGERENTRIES.LIST>", xml_escape(&entry.ledger), entry.side.tally_positive(), amount)
    }).collect::<String>();
    format!("<TALLYMESSAGE xmlns:UDF=\"TallyUDF\"><VOUCHER REMOTEID=\"{}\" VCHTYPE=\"{}\" ACTION=\"Create\" OBJVIEW=\"Accounting Voucher View\"><DATE>{}</DATE><VOUCHERTYPENAME>{}</VOUCHERTYPENAME>{voucher_number}{narration}{reference}{entries}</VOUCHER></TALLYMESSAGE>", xml_escape(&voucher.bridge_txn_id), voucher.voucher_type.as_str(), normalized_date(&voucher.date).unwrap_or_default(), voucher.voucher_type.as_str())
}

fn render_import_verification_read(company: &str, from: &str, to: &str) -> String {
    format!("<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Import Verification</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeImportWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"</SYSTEM><COLLECTION NAME=\"Bridge Agent Import Verification\" TYPE=\"Voucher\"><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,REMOTEID,GUID,MASTERID,ALTERID,NARRATION,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.LIST</FETCH><FILTERS>BridgeImportWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>", xml_escape(company))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_import_vouchers(xml: &str) -> Result<Vec<ReadVoucher>, String> {
    validate_verification_envelope(xml)?;
    let mut reader = quick_xml::Reader::from_str(xml);
    // Keep text fragments intact: quick-xml emits entity references separately,
    // and trimming the neighbouring fragments would turn `Party & Co` into
    // `Party&Co` before the fingerprint is built.
    reader.config_mut().trim_text(false);
    let mut vouchers = Vec::new();
    let mut voucher: Option<ReadVoucher> = None;
    let mut entry: Option<ReadEntry> = None;
    let mut tag = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).to_ascii_uppercase();
                if name == "VOUCHER" {
                    let remote_id = start
                        .attributes()
                        .flatten()
                        .find(|attribute| attribute.key.as_ref().eq_ignore_ascii_case(b"REMOTEID"))
                        .and_then(|attribute| {
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .ok()
                        })
                        .map(|value| value.into_owned());
                    voucher = Some(ReadVoucher {
                        remote_id,
                        guid: None,
                        alter_id: None,
                        date: None,
                        voucher_type: None,
                        narration: None,
                        voucher_number: None,
                        master_id: None,
                        cancelled: None,
                        optional: None,
                        entries: Vec::new(),
                    });
                } else if name == "ALLLEDGERENTRIES.LIST" && voucher.is_some() {
                    entry = Some(ReadEntry {
                        ledger: String::new(),
                        amount: String::new(),
                        is_deemed_positive: String::new(),
                    });
                }
                tag = name;
            }
            Ok(Event::Text(text)) => {
                if let (Some(current), Ok(value)) = (voucher.as_mut(), decoded_tally_text(text)) {
                    append_import_text(current, entry.as_mut(), &tag, value);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let (Some(current), Ok(value)) =
                    (voucher.as_mut(), decoded_tally_reference(reference))
                {
                    append_import_text(current, entry.as_mut(), &tag, value);
                }
            }
            Ok(Event::End(end)) => {
                let name = String::from_utf8_lossy(end.name().as_ref()).to_ascii_uppercase();
                if name == "ALLLEDGERENTRIES.LIST" {
                    if let (Some(current), Some(item)) = (voucher.as_mut(), entry.take()) {
                        if !item.ledger.is_empty()
                            && !item.amount.is_empty()
                            && !item.is_deemed_positive.is_empty()
                        {
                            current.entries.push(item);
                        }
                    }
                } else if name == "VOUCHER" {
                    if let Some(current) = voucher.take() {
                        vouchers.push(current);
                    }
                }
                tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err("import_verification_export_invalid".to_string()),
            _ => {}
        }
    }
    Ok(vouchers)
}

/// Tally exports XML-escaped names. Decode entities before a name becomes part
/// of the accounting fingerprint or a proof response, otherwise a successful
/// import with names such as `R&D` is falsely reported as divergent.
fn decoded_tally_text(text: quick_xml::events::BytesText<'_>) -> Result<String, String> {
    let decoded = text
        .decode()
        .map_err(|_| "import_verification_export_invalid".to_string())?;
    quick_xml::escape::unescape(&decoded)
        .map(|value| value.into_owned())
        .map_err(|_| "import_verification_export_invalid".to_string())
}

fn append_read_text(slot: &mut Option<String>, value: String) {
    slot.get_or_insert_with(String::new).push_str(&value);
}

fn decoded_tally_reference(reference: quick_xml::events::BytesRef<'_>) -> Result<String, String> {
    let reference = reference
        .decode()
        .map_err(|_| "import_verification_export_invalid".to_string())?;
    quick_xml::escape::unescape(&format!("&{reference};"))
        .map(|value| value.into_owned())
        .map_err(|_| "import_verification_export_invalid".to_string())
}

fn append_import_text(
    current: &mut ReadVoucher,
    entry: Option<&mut ReadEntry>,
    tag: &str,
    value: String,
) {
    if let Some(item) = entry {
        match tag {
            "LEDGERNAME" => item.ledger.push_str(&value),
            "AMOUNT" => item.amount.push_str(&value),
            "ISDEEMEDPOSITIVE" => item.is_deemed_positive.push_str(&value),
            _ => {}
        }
        return;
    }
    match tag {
        "DATE" => append_read_text(&mut current.date, value),
        "VOUCHERTYPENAME" => append_read_text(&mut current.voucher_type, value),
        "GUID" => append_read_text(&mut current.guid, value),
        "MASTERID" => append_read_text(&mut current.master_id, value),
        "VOUCHERNUMBER" => append_read_text(&mut current.voucher_number, value),
        "ALTERID" => current.alter_id = value.parse().ok(),
        "NARRATION" => append_read_text(&mut current.narration, value),
        "ISCANCELLED" => current.cancelled = tally_bool(&value),
        "ISOPTIONAL" => current.optional = tally_bool(&value),
        _ => {}
    }
}

fn tally_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "Yes" => Some(true),
        "No" => Some(false),
        _ => None,
    }
}

fn validate_verification_envelope(xml: &str) -> Result<(), String> {
    let trimmed = xml.trim();
    if trimmed.is_empty()
        || !trimmed.starts_with("<ENVELOPE")
        || trimmed.contains("<LINEERROR")
        || trimmed.contains("<ERROR")
        || trimmed.contains("<RESPONSE")
    {
        return Err("import_verification_protocol_invalid".to_string());
    }
    if !trimmed.contains("<COLLECTION") {
        return Err("import_verification_protocol_invalid".to_string());
    }
    Ok(())
}

fn verification_read_possibly_truncated(xml: &str) -> bool {
    xml.to_ascii_uppercase().contains("TRUNCAT")
}

fn corroborate_verification_window(
    observed: &[ReadVoucher],
    corroboration: &[ReadVoucher],
    from: &str,
    to: &str,
    max_rows: usize,
    truncated_signal: bool,
) -> Result<(), String> {
    if observed.iter().any(|voucher| {
        voucher
            .date
            .as_deref()
            .is_none_or(|date| date < from || date > to)
    }) {
        return Err("window_not_honoured".to_string());
    }
    if observed.len() >= max_rows || truncated_signal {
        return Err("verification_incomplete:window_possibly_truncated".to_string());
    }
    let pairs = |rows: &[ReadVoucher]| {
        rows.iter()
            .map(|voucher| {
                Ok((
                    voucher.guid.clone().ok_or_else(|| {
                        "verification_incomplete:window_not_corroborated".to_string()
                    })?,
                    voucher.alter_id.ok_or_else(|| {
                        "verification_incomplete:window_not_corroborated".to_string()
                    })?,
                ))
            })
            .collect::<Result<BTreeSet<_>, String>>()
    };
    if pairs(observed)? != pairs(corroboration)? {
        return Err("verification_incomplete:window_not_corroborated".to_string());
    }
    Ok(())
}

fn verify_batch(line: &ImportLedgerLine, observed: &[ReadVoucher]) -> Result<Value, String> {
    let mut rows = Vec::new();
    let mut counts = BTreeMap::from([
        ("posted_verified", 0_u64),
        ("posted_not_effective", 0),
        ("posted_divergent", 0),
        ("not_found", 0),
        ("not_attributable", 0),
        ("duplicate_fingerprint", 0),
    ]);
    for expected in &line.vouchers {
        let tag = format!("[BRIDGE:{}]", expected.bridge_txn_id);
        let tagged = observed
            .iter()
            .filter(|voucher| {
                voucher
                    .narration
                    .as_deref()
                    .is_some_and(|value| value.contains(&tag))
            })
            .collect::<Vec<_>>();
        let fingerprint = expected_entry_fingerprint(expected);
        let fingerprint_matches = observed
            .iter()
            .filter(|voucher| {
                voucher.date.as_deref() == normalized_date(&expected.date).ok().as_deref()
                    && voucher.voucher_type.as_deref() == Some(expected.voucher_type.as_str())
                    && actual_entry_fingerprint(voucher) == fingerprint
            })
            .collect::<Vec<_>>();
        let (matches, marker, not_attributable) = if !tagged.is_empty() {
            let attributable = tagged
                .iter()
                .copied()
                .filter(|voucher| {
                    line.pre_import_mark
                        .value
                        .is_some_and(|mark| voucher.alter_id.is_some_and(|id| id > mark))
                })
                .collect::<Vec<_>>();
            (
                attributable,
                "narration_tag",
                !tagged.is_empty() && line.pre_import_mark.value.is_some(),
            )
        } else {
            let attributable = fingerprint_matches
                .iter()
                .copied()
                .filter(|voucher| {
                    line.pre_import_mark
                        .value
                        .is_some_and(|mark| voucher.alter_id.is_some_and(|id| id > mark))
                })
                .collect::<Vec<_>>();
            (
                attributable,
                "accounting_fingerprint",
                !fingerprint_matches.is_empty() && line.pre_import_mark.value.is_some(),
            )
        };
        let value = if not_attributable && matches.is_empty() {
            counts
                .entry("not_attributable")
                .and_modify(|count| *count += 1);
            let reason = if marker == "narration_tag" {
                "tag_precedes_pre_import_voucher_mark"
            } else {
                "fingerprint_precedes_pre_import_voucher_mark"
            };
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"not_attributable","marker":marker,"reason":reason})
        } else if matches.is_empty() {
            counts.entry("not_found").and_modify(|count| *count += 1);
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"not_found"})
        } else if matches.len() > 1 {
            counts
                .entry("duplicate_fingerprint")
                .and_modify(|count| *count += 1);
            json!({"bridge_txn_id":expected.bridge_txn_id,"status":"duplicate_fingerprint","marker":marker,"matches":matches.len()})
        } else {
            let diffs = voucher_diffs(expected, matches[0]);
            if diffs.is_empty() && voucher_is_accounting_effective(matches[0])? {
                counts
                    .entry("posted_verified")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_verified","marker":marker,"voucher_number":matches[0].voucher_number,"guid":matches[0].guid,"master_id":matches[0].master_id,"alter_id":matches[0].alter_id})
            } else if diffs.is_empty() {
                counts
                    .entry("posted_not_effective")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_not_effective","marker":marker,"reason":"voucher_cancelled_or_optional","voucher_number":matches[0].voucher_number,"guid":matches[0].guid,"master_id":matches[0].master_id,"alter_id":matches[0].alter_id})
            } else {
                counts
                    .entry("posted_divergent")
                    .and_modify(|count| *count += 1);
                json!({"bridge_txn_id":expected.bridge_txn_id,"status":"posted_divergent","marker":marker,"diffs":diffs,"voucher_number":matches[0].voucher_number,"guid":matches[0].guid,"master_id":matches[0].master_id})
            }
        };
        rows.push(value);
    }
    let (batch_duplicates, unrelated_duplicates_in_window) = batch_duplicate_sets(line, observed);
    Ok(
        json!({"counts":counts,"vouchers":rows,"duplicates":batch_duplicates,"unrelated_duplicates_in_window":unrelated_duplicates_in_window}),
    )
}

fn voucher_is_accounting_effective(voucher: &ReadVoucher) -> Result<bool, String> {
    match (voucher.cancelled, voucher.optional) {
        (Some(false), Some(false)) => Ok(true),
        (Some(true), _) | (_, Some(true)) => Ok(false),
        _ => Err("voucher_accounting_state_not_observed".to_string()),
    }
}

fn verification_status(result: &Value, expected_voucher_count: usize) -> &'static str {
    if result["counts"]["posted_verified"].as_u64() == Some(expected_voucher_count as u64)
        && result["duplicates"].as_array().is_some_and(Vec::is_empty)
    {
        "posted_verified"
    } else {
        "verification_incomplete"
    }
}

fn batch_duplicate_sets(
    line: &ImportLedgerLine,
    observed: &[ReadVoucher],
) -> (Vec<Value>, Vec<Value>) {
    let all_duplicates = duplicates(observed);
    let batch_vouchers = observed
        .iter()
        .filter(|voucher| voucher_belongs_to_batch(line, voucher))
        .collect::<Vec<_>>();
    let batch_remote_ids = batch_vouchers
        .iter()
        .filter_map(|voucher| voucher.remote_id.as_deref())
        .collect::<BTreeSet<_>>();
    let batch_fingerprints = batch_vouchers
        .iter()
        .map(|voucher| voucher_fingerprint_key(voucher))
        .collect::<BTreeSet<_>>();
    let batch_duplicates = all_duplicates
        .iter()
        .filter(|duplicate| match duplicate["kind"].as_str() {
            Some("remote_id") => duplicate["remote_id"]
                .as_str()
                .is_some_and(|id| batch_remote_ids.contains(id)),
            Some("accounting_fingerprint") => duplicate["fingerprint"]
                .as_str()
                .is_some_and(|fingerprint| batch_fingerprints.contains(fingerprint)),
            _ => false,
        })
        .cloned()
        .collect::<Vec<_>>();
    let unrelated = all_duplicates
        .into_iter()
        .filter(|duplicate| !batch_duplicates.contains(duplicate))
        .collect();
    (batch_duplicates, unrelated)
}

fn voucher_belongs_to_batch(line: &ImportLedgerLine, voucher: &ReadVoucher) -> bool {
    line.vouchers.iter().any(|expected| {
        voucher.narration.as_deref().is_some_and(|narration| {
            narration.contains(&format!("[BRIDGE:{}]", expected.bridge_txn_id))
        }) || (voucher.date.as_deref() == normalized_date(&expected.date).ok().as_deref()
            && voucher.voucher_type.as_deref() == Some(expected.voucher_type.as_str())
            && actual_entry_fingerprint(voucher) == expected_entry_fingerprint(expected))
    })
}

fn voucher_diffs(expected: &ImportVoucher, actual: &ReadVoucher) -> Vec<Value> {
    let mut diffs = Vec::new();
    if actual.date.as_deref() != normalized_date(&expected.date).ok().as_deref() {
        diffs.push(json!("date"));
    }
    if actual.voucher_type.as_deref() != Some(expected.voucher_type.as_str()) {
        diffs.push(json!("voucher_type"));
    }
    if expected.voucher_number.is_some()
        && actual.voucher_number.as_deref() != expected.voucher_number.as_deref()
    {
        diffs.push(json!("voucher_number"));
    }
    let expected_entries = expected_entry_fingerprint(expected);
    let actual_entries = actual_entry_fingerprint(actual);
    if expected_entries != actual_entries {
        diffs.push(json!({"entries":{"expected":expected_entries,"observed":actual_entries}}));
    }
    diffs
}

fn expected_entry_fingerprint(voucher: &ImportVoucher) -> Vec<String> {
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
fn actual_entry_fingerprint(voucher: &ReadVoucher) -> Vec<String> {
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

fn voucher_fingerprint_key(voucher: &ReadVoucher) -> String {
    format!(
        "{}|{}|{}",
        voucher.date.as_deref().unwrap_or(""),
        voucher.voucher_type.as_deref().unwrap_or(""),
        actual_entry_fingerprint(voucher).join(",")
    )
}

fn duplicates(observed: &[ReadVoucher]) -> Vec<Value> {
    let mut remote = BTreeMap::<String, usize>::new();
    let mut fingerprints = BTreeMap::<String, BTreeSet<String>>::new();
    for voucher in observed {
        if let Some(id) = &voucher.remote_id {
            *remote.entry(id.clone()).or_default() += 1;
            let key = voucher_fingerprint_key(voucher);
            fingerprints.entry(key).or_default().insert(id.clone());
        }
    }
    let mut result = remote
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(remote_id, count)| json!({"kind":"remote_id","remote_id":remote_id,"count":count}))
        .collect::<Vec<_>>();
    result.extend(fingerprints.into_iter().filter(|(_, ids)| ids.len() > 1).map(|(fingerprint, ids)| json!({"kind":"accounting_fingerprint","fingerprint":fingerprint,"remote_ids":ids})));
    result
}

fn company_high_water_mark(high_water: &Value) -> Result<PreImportMark, String> {
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

fn alter_id_delta(mark: &PreImportMark, observed: &[ReadVoucher]) -> Value {
    let latest = observed.iter().filter_map(|voucher| voucher.alter_id).max();
    match (mark.value, latest) {
        (Some(before), Some(after)) if after >= before => {
            json!({"before":before,"after_seen":after,"delta":after-before})
        }
        _ => json!({"before":mark.value,"after_seen":latest,"delta":"not_observed"}),
    }
}

fn render_proof_markdown(proof: &Value) -> String {
    let mut output = format!("# Proof-of-Post — {}\n\n- Company: `{}`\n- Batch SHA-256: `{}`\n- Verified: `{}`\n- Counts: verified {}, divergent {}, not found {}\n- AlterID delta: `{}`\n- Unrelated duplicates in window: {}\n\n| Transaction | Status |\n| --- | --- |\n", proof["batch_id"].as_str().unwrap_or("unknown"), proof["company"]["name"].as_str().unwrap_or("unknown"), proof["batch_sha256"].as_str().unwrap_or("unknown"), proof["verified_at"].as_str().unwrap_or("unknown"), proof["counts"]["posted_verified"], proof["counts"]["posted_divergent"], proof["counts"]["not_found"], proof["alter_id_delta"], proof["unrelated_duplicates_in_window"].as_array().map_or(0, Vec::len));
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
fn local_evidence(label: &str) -> Evidence {
    Evidence {
        request_sha256: sha256_hex(label.as_bytes()),
        response_sha256: sha256_hex(label.as_bytes()),
        bytes: 0,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    }
}
fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|_| "import_file_write_failed".to_string())?;
    set_private(path)
}
fn set_private(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "import_file_permissions_failed".to_string())?;
    }
    Ok(())
}
fn set_private_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "import_file_permissions_failed".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_tally_transport::TallyEndpointConfig;
    use tally_protocol_simulator::{
        Fixture, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    const GUID: &str = "00000000-0000-4000-8000-000000000001";

    fn payload() -> ImportPayload {
        serde_json::from_value(json!({"company_guid":GUID,"vouchers":[
            {"bridge_txn_id":"txn-001","date":"2026-09-01","voucher_type":"Payment","narration":"Paid & settled","reference":"REF-1","entries":[{"ledger":"Expense","amount":"12.50","side":"Dr"},{"ledger":"Bank","amount":"12.50","side":"Cr"}]},
            {"bridge_txn_id":"txn-002","date":"2026-09-02","voucher_type":"Receipt","entries":[{"ledger":"Bank","amount":"7.50","side":"Dr"},{"ledger":"Income","amount":"7.50","side":"Cr"}]}
        ]})).expect("sample payload")
    }

    #[test]
    fn schema_balance_matcher_rendering_and_ledger_append_are_fail_closed() {
        let input = payload();
        validate_payload(&input).expect("valid payload");
        let mut unbalanced = input.clone();
        unbalanced.vouchers[0].entries[1].amount = "12.49".to_string();
        assert_eq!(
            validate_payload(&unbalanced),
            Err("voucher_not_balanced".to_string())
        );
        assert_eq!(
            master_match("bank ", &["Bank".to_string()])["match_state"],
            "near_miss"
        );
        assert_eq!(
            master_match("bank", &["Bank".to_string()])["match_state"],
            "near_miss"
        );
        assert_eq!(
            master_match("A\u{a0}B", &["A B".to_string()])["match_state"],
            "near_miss"
        );
        assert_eq!(
            master_match("Fees-Admin", &["Fees–Admin".to_string()])["match_state"],
            "near_miss"
        );
        assert_eq!(
            master_match("Bob's", &["Bob’s".to_string()])["match_state"],
            "near_miss"
        );
        assert_eq!(
            master_match("Bank", &["Bank Charges".to_string()])["match_state"],
            "near_miss"
        );
        let xml = render_import_xml("Book & Co", &input.vouchers);
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<NARRATION>Paid &amp; settled [BRIDGE:txn-001]</NARRATION>"));
        assert!(xml.contains("<NARRATION>[BRIDGE:txn-002]</NARRATION>"));
        assert_eq!(
            voucher_input_schema()["properties"]["vouchers"]["items"]["properties"]["entries"]
                ["items"]["properties"]["side"]["enum"],
            json!(["Dr", "Cr"])
        );
        let directory = tempfile::tempdir().expect("temporary data directory");
        let server = Server::new(super::super::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: 9,
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: super::super::Redaction::None,
            import_enabled: true,
        });
        let line = ImportLedgerLine {
            batch_id: "batch-a".to_string(),
            company_guid: GUID.to_string(),
            company: None,
            txn_ids: vec!["txn-001".to_string()],
            date_from: "2026-09-01".to_string(),
            date_to: "2026-09-01".to_string(),
            sha256: "hash".to_string(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: PreImportMark {
                kind: "latest_voucher_alterid_seen".to_string(),
                value: Some(4),
                master_value: Some(4),
            },
            vouchers: vec![input.vouchers[0].clone()],
        };
        server.append_import_ledger(&line).expect("append");
        assert_eq!(server.import_ledger().expect("read").len(), 1);

        let settings = server.settings.clone();
        let first_settings = settings.clone();
        let first_line = line.clone();
        let second_settings = settings;
        let second_line = line.clone();
        let first = std::thread::spawn(move || {
            Server::new(first_settings).append_import_ledger(&first_line)
        });
        let second = std::thread::spawn(move || {
            Server::new(second_settings).append_import_ledger(&second_line)
        });
        first.join().expect("first writer").expect("first append");
        second
            .join()
            .expect("second writer")
            .expect("second append");
        let lines = server.import_ledger().expect("parseable ledger lines");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn verification_reports_absence_divergence_and_duplicate_fingerprints() {
        let input = payload();
        let line = ImportLedgerLine {
            batch_id: "batch-b".to_string(),
            company_guid: GUID.to_string(),
            company: None,
            txn_ids: input
                .vouchers
                .iter()
                .map(|voucher| voucher.bridge_txn_id.clone())
                .collect(),
            date_from: "2026-09-01".to_string(),
            date_to: "2026-09-02".to_string(),
            sha256: "hash".to_string(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: PreImportMark {
                kind: "latest_voucher_alterid_seen".to_string(),
                value: Some(10),
                master_value: Some(10),
            },
            vouchers: input.vouchers,
        };
        let observed = vec![
            ReadVoucher {
                remote_id: Some("txn-001".to_string()),
                guid: Some("g-1".to_string()),
                alter_id: Some(12),
                date: Some("20260901".to_string()),
                voucher_type: Some("Payment".to_string()),
                narration: Some("[BRIDGE:txn-001]".to_string()),
                voucher_number: None,
                master_id: None,
                cancelled: Some(false),
                optional: Some(false),
                entries: vec![
                    ReadEntry {
                        ledger: "Expense".to_string(),
                        amount: "-12.51".to_string(),
                        is_deemed_positive: "Yes".to_string(),
                    },
                    ReadEntry {
                        ledger: "Bank".to_string(),
                        amount: "12.50".to_string(),
                        is_deemed_positive: "No".to_string(),
                    },
                ],
            },
            ReadVoucher {
                remote_id: Some("other-id".to_string()),
                guid: None,
                alter_id: Some(13),
                date: Some("20260901".to_string()),
                voucher_type: Some("Payment".to_string()),
                narration: None,
                voucher_number: None,
                master_id: None,
                cancelled: Some(false),
                optional: Some(false),
                entries: vec![
                    ReadEntry {
                        ledger: "Expense".to_string(),
                        amount: "-12.51".to_string(),
                        is_deemed_positive: "Yes".to_string(),
                    },
                    ReadEntry {
                        ledger: "Bank".to_string(),
                        amount: "12.50".to_string(),
                        is_deemed_positive: "No".to_string(),
                    },
                ],
            },
        ];
        let result = verify_batch(&line, &observed).expect("verification result");
        assert_eq!(result["counts"]["posted_divergent"], 1);
        assert_eq!(result["counts"]["not_found"], 1);
        assert_eq!(result["duplicates"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            parse_import_vouchers(""),
            Err("import_verification_protocol_invalid".to_string())
        );
        assert_eq!(
            parse_import_vouchers("<ENVELOPE><BODY><RESPONSE>error</RESPONSE></BODY></ENVELOPE>"),
            Err("import_verification_protocol_invalid".to_string())
        );
    }

    #[test]
    fn verification_window_corroboration_rejects_each_unsafe_branch() {
        let voucher = |guid: &str, alter_id, date: &str| ReadVoucher {
            remote_id: None,
            guid: Some(guid.to_string()),
            alter_id: Some(alter_id),
            date: Some(date.to_string()),
            voucher_type: None,
            narration: None,
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![],
        };
        let inside = voucher("guid-1", 3, "20260901");
        assert_eq!(
            corroborate_verification_window(
                &[voucher("guid-1", 3, "20260903")],
                &[inside.clone()],
                "20260901",
                "20260902",
                10,
                false
            ),
            Err("window_not_honoured".to_string())
        );
        assert_eq!(
            corroborate_verification_window(
                &[inside.clone()],
                &[voucher("guid-2", 3, "20260901")],
                "20260901",
                "20260902",
                10,
                false
            ),
            Err("verification_incomplete:window_not_corroborated".to_string())
        );
        assert_eq!(
            corroborate_verification_window(
                &[inside.clone()],
                &[inside],
                "20260901",
                "20260902",
                1,
                false
            ),
            Err("verification_incomplete:window_possibly_truncated".to_string())
        );
    }

    #[test]
    fn batch_guid_is_canonicalized_and_compared_case_insensitively() {
        let stored = canonical_batch_guid("A1B2-C3D4");
        assert_eq!(stored, "a1b2-c3d4");
        assert!(batch_guid_matches(&stored, "A1B2-C3D4"));
    }

    #[test]
    fn failed_ledger_append_removes_the_written_import_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("batch.xml");
        fs::write(&path, "xml").expect("import file");
        assert_eq!(
            remove_orphaned_import_file(&path, "import_ledger_unavailable".to_string()),
            "import_ledger_unavailable"
        );
        assert!(!path.exists());
    }

    #[test]
    fn unrelated_window_duplicates_do_not_block_a_verified_batch() {
        let input = payload();
        let line = ImportLedgerLine {
            batch_id: "batch-unrelated".to_string(),
            company_guid: GUID.to_string(),
            company: None,
            txn_ids: vec!["txn-001".to_string()],
            date_from: "20260901".to_string(),
            date_to: "20260901".to_string(),
            sha256: "hash".to_string(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: PreImportMark {
                kind: "company_high_water".to_string(),
                value: Some(10),
                master_value: Some(7),
            },
            vouchers: vec![input.vouchers[0].clone()],
        };
        let expected = &line.vouchers[0];
        let posted = ReadVoucher {
            remote_id: Some("posted-1".to_string()),
            guid: Some("posted-guid".to_string()),
            alter_id: Some(11),
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            narration: Some("[BRIDGE:txn-001]".to_string()),
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![
                ReadEntry {
                    ledger: "Expense".to_string(),
                    amount: "-12.50".to_string(),
                    is_deemed_positive: "Yes".to_string(),
                },
                ReadEntry {
                    ledger: "Bank".to_string(),
                    amount: "12.50".to_string(),
                    is_deemed_positive: "No".to_string(),
                },
            ],
        };
        assert_eq!(expected.bridge_txn_id, "txn-001");
        let unrelated = |guid: &str| ReadVoucher {
            remote_id: Some("unrelated-duplicate".to_string()),
            guid: Some(guid.to_string()),
            alter_id: Some(3),
            date: Some("20260901".to_string()),
            voucher_type: Some("Journal".to_string()),
            narration: None,
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![ReadEntry {
                ledger: "Unrelated".to_string(),
                amount: "1.00".to_string(),
                is_deemed_positive: "No".to_string(),
            }],
        };
        let result = verify_batch(&line, &[posted, unrelated("u-1"), unrelated("u-2")])
            .expect("verification result");
        assert_eq!(result["counts"]["posted_verified"], 1);
        assert!(result["duplicates"].as_array().is_some_and(Vec::is_empty));
        assert_eq!(
            result["unrelated_duplicates_in_window"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            verification_status(&result, line.vouchers.len()),
            "posted_verified"
        );
    }

    #[test]
    fn fingerprint_only_verification_requires_a_post_mark_voucher() {
        let input = payload();
        let line = ImportLedgerLine {
            batch_id: "batch-mark".to_string(),
            company_guid: GUID.to_string(),
            company: None,
            txn_ids: vec!["txn-001".to_string()],
            date_from: "20260901".to_string(),
            date_to: "20260901".to_string(),
            sha256: "hash".to_string(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: PreImportMark {
                kind: "company_high_water".to_string(),
                value: Some(10),
                master_value: Some(10),
            },
            vouchers: vec![input.vouchers[0].clone()],
        };
        let observed = |alter_id| ReadVoucher {
            remote_id: None,
            guid: None,
            alter_id: Some(alter_id),
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            narration: None,
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![
                ReadEntry {
                    ledger: "Expense".to_string(),
                    amount: "-12.50".to_string(),
                    is_deemed_positive: "Yes".to_string(),
                },
                ReadEntry {
                    ledger: "Bank".to_string(),
                    amount: "12.50".to_string(),
                    is_deemed_positive: "No".to_string(),
                },
            ],
        };
        assert_eq!(
            verify_batch(&line, &[observed(10)]).expect("verification result")["vouchers"][0]
                ["status"],
            "not_attributable"
        );
        assert_eq!(
            verify_batch(&line, &[observed(11)]).expect("verification result")["vouchers"][0]
                ["status"],
            "posted_verified"
        );
    }

    #[test]
    fn narration_tag_verification_requires_a_post_mark_voucher() {
        let input = payload();
        let line = ImportLedgerLine {
            batch_id: "batch-tag-mark".to_string(),
            company_guid: GUID.to_string(),
            company: None,
            txn_ids: vec!["txn-001".to_string()],
            date_from: "20260901".to_string(),
            date_to: "20260901".to_string(),
            sha256: "hash".to_string(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: PreImportMark {
                kind: "company_high_water".to_string(),
                value: Some(10),
                master_value: Some(7),
            },
            vouchers: vec![input.vouchers[0].clone()],
        };
        let observed = |alter_id| ReadVoucher {
            remote_id: Some("posted-1".to_string()),
            guid: Some("posted-guid".to_string()),
            alter_id: Some(alter_id),
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            narration: Some("[BRIDGE:txn-001]".to_string()),
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![
                ReadEntry {
                    ledger: "Expense".to_string(),
                    amount: "-12.50".to_string(),
                    is_deemed_positive: "Yes".to_string(),
                },
                ReadEntry {
                    ledger: "Bank".to_string(),
                    amount: "12.50".to_string(),
                    is_deemed_positive: "No".to_string(),
                },
            ],
        };
        let before = verify_batch(&line, &[observed(10)]).expect("pre-mark tag");
        assert_eq!(before["vouchers"][0]["status"], "not_attributable");
        assert_eq!(
            before["vouchers"][0]["reason"],
            "tag_precedes_pre_import_voucher_mark"
        );
        assert_eq!(
            verify_batch(&line, &[observed(11)]).expect("post-mark tag")["vouchers"][0]["status"],
            "posted_verified"
        );
    }

    #[test]
    fn verified_import_vouchers_require_observed_effective_accounting_flags() {
        let input = payload();
        let line = ImportLedgerLine {
            batch_id: "batch-accounting-state".to_string(),
            company_guid: GUID.to_string(),
            company: None,
            txn_ids: vec!["txn-001".to_string()],
            date_from: "20260901".to_string(),
            date_to: "20260901".to_string(),
            sha256: "hash".to_string(),
            built_at: now(),
            status: "built".to_string(),
            pre_import_mark: PreImportMark {
                kind: "company_high_water".to_string(),
                value: Some(10),
                master_value: Some(7),
            },
            vouchers: vec![input.vouchers[0].clone()],
        };
        let observed = ReadVoucher {
            remote_id: Some("posted-1".to_string()),
            guid: Some("posted-guid".to_string()),
            alter_id: Some(11),
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            narration: Some("[BRIDGE:txn-001]".to_string()),
            voucher_number: None,
            master_id: None,
            cancelled: Some(false),
            optional: Some(false),
            entries: vec![
                ReadEntry {
                    ledger: "Expense".to_string(),
                    amount: "-12.50".to_string(),
                    is_deemed_positive: "Yes".to_string(),
                },
                ReadEntry {
                    ledger: "Bank".to_string(),
                    amount: "12.50".to_string(),
                    is_deemed_positive: "No".to_string(),
                },
            ],
        };
        assert_eq!(
            verify_batch(&line, std::slice::from_ref(&observed)).expect("effective voucher")
                ["vouchers"][0]["status"],
            "posted_verified"
        );
        for (cancelled, optional) in [(Some(true), Some(false)), (Some(false), Some(true))] {
            let mut ineffective = observed.clone();
            ineffective.cancelled = cancelled;
            ineffective.optional = optional;
            let result = verify_batch(&line, &[ineffective]).expect("ineffective voucher result");
            assert_eq!(result["vouchers"][0]["status"], "posted_not_effective");
            assert_eq!(result["counts"]["posted_not_effective"], 1);
        }
        let mut missing = observed;
        missing.optional = None;
        assert_eq!(
            verify_batch(&line, &[missing]),
            Err("voucher_accounting_state_not_observed".to_string())
        );
    }

    #[test]
    fn company_high_water_mark_refuses_voucher_scan_shapes_and_preserves_attribution_boundary() {
        assert_eq!(
            company_high_water_mark(&json!({"vouchers":[{"alter_id":999}]})),
            Err("pre_import_mark_unobserved".to_string())
        );
        let mark = company_high_water_mark(&json!({"altvchid":10,"altmstid":7}))
            .expect("company high water");
        assert_eq!(mark.kind, "company_high_water");
        assert_eq!(mark.value, Some(10));
        assert_eq!(mark.master_value, Some(7));
    }

    #[test]
    fn verification_unescapes_every_record_text_node_before_fingerprinting() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><NARRATION>Party &amp; Co &lt;quoted&gt; &quot;name&quot; &#x26;</NARRATION><ALLLEDGERENTRIES.LIST><LEDGERNAME>R&amp;D &lt;Lab&gt; &quot;A&quot; &#38;</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        let observed = parse_import_vouchers(xml).expect("escaped export parses");
        assert_eq!(
            observed[0].narration.as_deref(),
            Some("Party & Co <quoted> \"name\" &")
        );
        assert_eq!(observed[0].entries[0].ledger, "R&D <Lab> \"A\" &");
    }

    #[test]
    fn optional_voucher_number_is_rendered_only_when_valid_and_supplied() {
        let mut input = payload();
        input.vouchers[0].voucher_number = Some("PV-0001".to_string());
        let rendered = render_import_xml("Book", &input.vouchers);
        assert!(rendered.contains("<VOUCHERNUMBER>PV-0001</VOUCHERNUMBER>"));
        assert_eq!(rendered.matches("<VOUCHERNUMBER>").count(), 1);
        input.vouchers[0].voucher_number = Some("bad$number".to_string());
        assert_eq!(
            validate_payload(&input),
            Err("voucher_number_invalid".to_string())
        );
    }

    #[test]
    fn batch_company_tuple_rejects_a_same_guid_different_book() {
        let company = |books_from: &str| bridge_tally_protocol::TallyCompany {
            name: "Bridge Book".to_string(),
            guid: Some(GUID.to_string()),
            company_number: Some("1".to_string()),
            books_from: Some(books_from.to_string()),
        };
        let original = import_company_tuple(&company("20260401")).expect("complete tuple");
        let different_book = import_company_tuple(&company("20270401")).expect("complete tuple");
        assert_ne!(original, different_book);
    }

    #[tokio::test]
    async fn simulator_build_then_manual_import_readback_verifies_every_voucher() {
        let company = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"BRIDGE SYNTHETIC BOOK\"><GUID>{GUID}</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>");
        let ledgers = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COMPANYCONTEXT><SCHEMA>bridge.tally.ledgers/1</SCHEMA><OBJECTTYPE>LEDGER</OBJECTTYPE><NAME>BRIDGE SYNTHETIC BOOK</NAME><GUID>{GUID}</GUID><RECORDCOUNT>3</RECORDCOUNT></COMPANYCONTEXT><COLLECTION><LEDGER NAME=\"Expense\" GUID=\"{GUID}-00000001\"><PARENT>Primary</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER><LEDGER NAME=\"Bank\" GUID=\"{GUID}-00000002\"><PARENT>Primary</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER><LEDGER NAME=\"Income\" GUID=\"{GUID}-00000003\"><PARENT>Primary</PARENT><OPENINGBALANCE>0</OPENINGBALANCE></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>");
        let premark = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>{GUID}</GUID><ALTVCHID>10</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>");
        let readback = concat!(
            "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"tally-assigned-1\"><DATE>20260901</DATE><VOUCHERNUMBER>PV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Payment</VOUCHERTYPENAME><GUID>g-1</GUID><MASTERID>1</MASTERID><ALTERID>12</ALTERID><NARRATION>Paid &amp; settled [BRIDGE:txn-001]</NARRATION><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>Expense</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-12.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>Bank</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>12.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER>",
            "<VOUCHER REMOTEID=\"tally-assigned-2\"><DATE>20260902</DATE><VOUCHERNUMBER>RV-1</VOUCHERNUMBER><VOUCHERTYPENAME>Receipt</VOUCHERTYPENAME><GUID>g-2</GUID><MASTERID>2</MASTERID><ALTERID>13</ALTERID><NARRATION>[BRIDGE:txn-002]</NARRATION><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>Bank</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-7.50</AMOUNT></ALLLEDGERENTRIES.LIST><ALLLEDGERENTRIES.LIST><LEDGERNAME>Income</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>7.50</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
        )
        .to_string();
        let simulator = SequenceSimulator::spawn(
            vec![
                company.clone(),
                company.clone(),
                ledgers,
                company.clone(),
                company.clone(),
                premark,
                company.clone(),
                company.clone(),
                company.clone(),
                readback,
                company,
            ]
            .into_iter()
            .map(|xml| {
                ScenarioPlan::new(Fixture::SyntheticXml(xml))
                    .with_encoding(WireEncoding::Utf16Le)
                    .with_framing(ResponseFraming::ContentLength)
            })
            .collect(),
        )
        .expect("simulator");
        let directory = tempfile::tempdir().expect("temporary data directory");
        let server = Server::new(super::super::Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: simulator.address().port(),
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: super::super::Redaction::None,
            import_enabled: true,
        });
        let built = server
            .build_import_xml(&serde_json::to_value(payload()).expect("json"))
            .await
            .expect("build");
        let batch_id = built.0["result"]["batch_id"]
            .as_str()
            .expect("batch id")
            .to_string();
        assert_eq!(built.0["result"]["live_evidence"], "none_recorded");
        assert!(directory
            .path()
            .join("imports")
            .join(format!("{batch_id}.xml"))
            .exists());
        let proof = server
            .verify_import(&json!({"company_guid":GUID,"batch_id":batch_id}))
            .await
            .expect("verify");
        assert_eq!(proof.0["result"]["counts"]["posted_verified"], 2);
        assert!(directory
            .path()
            .join("imports")
            .join(format!(
                "{}.proof.md",
                proof.0["result"]["batch_id"].as_str().expect("batch id")
            ))
            .exists());
        assert_eq!(simulator.finish().expect("requests").len(), 11);
    }
}
