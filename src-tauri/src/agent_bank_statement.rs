//! `parse_bank_statement`: a local, read-only capability that turns a
//! password-protected bank-statement PDF into voucher proposals.
//!
//! **What may leave the machine is decided here.** The statement's rows are
//! a client's banking record, and a tool result reaches the AI conversation.
//! So the full proposals — every row's date, amount, bank reference and
//! narration — are written to a private local file, and the result carries
//! only what an operator needs to write the mapping: each counterparty's
//! spelling as printed, its row count and total, its disposition and whether
//! it reached suspense, and the ledger names to check with `validate_masters`.
//! Every name in that summary is marked as a party name, so the
//! `mask_parties` redaction preset masks it.
//!
//! **The password never enters the conversation.** It is read from a local
//! owner-only file named by `password_file`, held in a zeroizing buffer, handed
//! to PDFium once and dropped. It is not echoed, logged, persisted or hashed
//! into anything; the egress receipt hashes the arguments, which carry only the
//! file's path. Residual: `pdfium-render` and PDFium each keep a copy that is
//! not zeroised (see `bridge_bank_statement::pdf`).
//!
//! **No identity is minted.** Proposals carry `bridge_txn_id` labels and no
//! REMOTEID or XML. `build_import_xml` builds from the file when given its
//! `proposals_id` and the `sha256` this tool returned
//! ([`resolve_import_arguments`]); the file's vouchers then pass the same
//! admission as inline vouchers, so nothing here decides what is admitted.

use super::*;
use bridge_bank_statement::bank::Bank;
use bridge_bank_statement::date::Date;
use bridge_bank_statement::mapping::{Mapping, MappingRow};
use bridge_bank_statement::money::Controls;
use bridge_bank_statement::pdf::{self, MAX_PDF_BYTES};
use bridge_bank_statement::pipeline::{prepare, ParsedStatement, StatementRequest};
use bridge_bank_statement::proposals::{format_amount, Disposition};
use bridge_bank_statement::Refusal;
use std::fs;
use std::io::Read;
use std::path::Path;
use zeroize::Zeroizing;

pub(super) const MAX_MAPPING_ROWS: usize = 1000;
const MAX_PASSWORD_FILE_BYTES: u64 = 1024;
const MAX_PATH_CHARS: usize = 4096;
const PROPOSALS_DIRECTORY: &str = "bank-statements";
const PROPOSALS_SCHEMA: &str = "bridge.bank_statement.proposals.v1";
/// Larger than any admissible file: 1000 vouchers at the text limits.
const MAX_PROPOSALS_FILE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn input_schema() -> Value {
    let path = json!({"type":"string","minLength":1,"maxLength":MAX_PATH_CHARS,"pattern":r"\S"});
    let control = json!({"type":"string","minLength":1,"maxLength":64,"pattern":r"\S"});
    let totals_control = json!({"type":"string","minLength":1,"maxLength":64,"pattern":r"\S","description":"Required for sbi and hdfc, whose statements print it. A ubi statement prints no totals: omit both, and every page must then print its Page N of M footer."});
    let ledger = json!({"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"});
    json!({
        "type":"object", "additionalProperties":false,
        "required":["statement_path","password_file","bank","account_label","opening_balance","closing_balance","bank_ledger","suspense_ledger"],
        "properties":{
            "statement_path": path,
            "password_file": path,
            "bank":{"type":"string","enum":["sbi","hdfc","ubi"],"description":"sbi (State Bank of India), hdfc (HDFC Bank) or ubi (Union Bank of India)."},
            "account_label":{"type":"string","minLength":4,"maxLength":64,"pattern":r"\S","description":"A short label carrying at least the last 4 digits of the account, e.g. 'HDFC CA xx4321'. Those digits must end a number on the statement's account-number line. The label is written into each narration."},
            "opening_balance": control,
            "closing_balance": control,
            "total_debits": totals_control.clone(),
            "total_credits": totals_control,
            "bank_ledger": ledger,
            "suspense_ledger": ledger,
            "from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
            "to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},
            "mapping":{
                "type":"array", "maxItems":MAX_MAPPING_ROWS,
                "items":{
                    "type":"object", "additionalProperties":false, "required":["party","ledger"],
                    "properties":{
                        "party":{"type":"string","minLength":1,"maxLength":agent_import::MAX_MASTER_NAME_CHARS,"pattern":r"\S"},
                        "ledger":{"type":"string","maxLength":agent_import::MAX_MASTER_NAME_CHARS},
                        "treatment":{"type":"string","enum":["auto","contra","skip"]}
                    }
                }
            }
        }
    })
}

pub(super) const DESCRIPTION: &str = "Read a local, password-protected SBI, HDFC or Union Bank of India bank-statement PDF and propose one Payment, Receipt or Contra per row, for build_import_xml's voucher shape. The whole run is refused unless the statement's account-number line ends with the digits in account_label, and every row's running balance, the closing balance, and (where the statement prints them) the debit and credit totals reproduce the figures supplied exactly. The password is read from password_file, a local file only its owner can read, and is never returned. Full proposals stay in a private local file; the result is a counterparty summary (spelling as printed, row count, total, disposition, suspense) for writing `mapping`, and the ledger names to check with validate_masters. A party the mapping does not name, or the parser could not identify, goes to suspense_ledger; `skip` omits a transfer already carried by another account's Contra. An ambiguous mapping is refused, never guessed. Re-run with a corrected mapping: bridge_txn_id labels depend only on the statement row, so they do not change. To build, pass the returned proposals_id and sha256 to build_import_xml as proposals_id and proposals_sha256; to correct a batch already built from an earlier run, add amends_batch_id. Never contacts Tally.";

impl Server {
    pub(super) async fn parse_bank_statement(
        &self,
        args: &Value,
    ) -> Result<ToolOutcome, ToolFailure> {
        let request = OwnedRequest::from_args(args)?;
        let data_dir = self.settings.data_dir.clone();
        let result = tokio::task::spawn_blocking(move || run(&request, &data_dir))
            .await
            .map_err(|_| "statement_task_failed".to_string())??;
        let bytes = serde_json::to_vec(&result).map_or(0, |bytes| bytes.len());
        Ok(ToolOutcome {
            evidence: Evidence {
                request_sha256: sha256_hex(b"parse_bank_statement"),
                response_sha256: sha256_json(&result),
                bytes,
                state: "complete",
                read_at: None,
                duration_ms: None,
                reason_code: None,
            },
            payload: json!({"result": result}),
            company_guid: None,
            truncated: false,
        })
    }
}

/// The admitted arguments, owned so the parse can run off the async runtime.
struct OwnedRequest {
    statement_path: PathBuf,
    password_file: PathBuf,
    bank: Bank,
    account_label: String,
    controls: Controls,
    bank_ledger: String,
    suspense_ledger: String,
    date_from: Option<Date>,
    date_to: Option<Date>,
    mapping: Mapping,
}

fn refused(refusal: &Refusal) -> String {
    match refusal.row {
        Some(row) => format!("statement_{}:row_{row}", refusal.category),
        None => format!("statement_{}", refusal.category),
    }
}

fn absolute_path(args: &Value, key: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required_string(args, key)?);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("argument_invalid:{key}"))
    }
}

fn window_date(args: &Value, key: &str) -> Result<Option<Date>, String> {
    optional_string(args, key)?
        .map(|text| {
            let compact = normalized_date(&text)?;
            Date::parse_iso(&format!(
                "{}-{}-{}",
                &compact[..4],
                &compact[4..6],
                &compact[6..]
            ))
            .ok_or_else(|| format!("argument_invalid:{key}"))
        })
        .transpose()
}

impl OwnedRequest {
    fn from_args(args: &Value) -> Result<Self, String> {
        let schema = input_schema();
        if let Some(mapping) = args.get("mapping") {
            catalog::validate_against_schema(mapping, &schema["properties"]["mapping"], "mapping")?;
        }
        let bank = Bank::from_name(required_string(args, "bank")?)
            .ok_or_else(|| "argument_invalid:bank".to_string())?;
        let total_debits = optional_string(args, "total_debits")?;
        let total_credits = optional_string(args, "total_credits")?;
        let controls = Controls::parse_optional(
            required_string(args, "opening_balance")?,
            required_string(args, "closing_balance")?,
            total_debits.as_deref(),
            total_credits.as_deref(),
        )
        .map_err(|refusal| refused(&refusal))?;
        if bank.prints_totals() && controls.debits.is_none() {
            return Err("statement_control_totals_required".into());
        }
        let date_from = window_date(args, "from")?;
        let date_to = window_date(args, "to")?;
        if let (Some(from), Some(to)) = (date_from, date_to) {
            if from > to {
                return Err("statement_reversed_date_window".into());
            }
        }
        let rows = args
            .get("mapping")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(index, row)| MappingRow {
                origin: format!("mapping[{index}]"),
                party: row["party"].as_str().unwrap_or_default().to_string(),
                ledger: row["ledger"].as_str().unwrap_or_default().to_string(),
                treatment: row["treatment"].as_str().map(str::to_string),
            });
        let mapping = Mapping::from_rows(rows).map_err(|refusal| refused(&refusal))?;
        Ok(Self {
            statement_path: absolute_path(args, "statement_path")?,
            password_file: absolute_path(args, "password_file")?,
            bank,
            account_label: required_string(args, "account_label")?.to_string(),
            controls,
            bank_ledger: required_string(args, "bank_ledger")?.to_string(),
            suspense_ledger: required_string(args, "suspense_ledger")?.to_string(),
            date_from,
            date_to,
            mapping,
        })
    }
}

/// The password file's contents, less one trailing line ending.
///
/// Refused unless it is a regular file owned by this user with a single link
/// and, on Unix, no group or other permission bits: a password that anyone
/// else can read is not one this tool should be trusted to keep.
fn read_password(path: &Path) -> Result<Zeroizing<String>, String> {
    let file = local_file::open_local_file(path, false)
        .map_err(|_| "statement_password_file_unreadable".to_string())?;
    let metadata = file
        .metadata()
        .map_err(|_| "statement_password_file_unreadable".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("statement_password_file_permissions".into());
        }
    }
    if metadata.len() > MAX_PASSWORD_FILE_BYTES {
        return Err("statement_password_file_too_large".into());
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_PASSWORD_FILE_BYTES as usize));
    file.take(MAX_PASSWORD_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "statement_password_file_unreadable".to_string())?;
    if bytes.len() as u64 > MAX_PASSWORD_FILE_BYTES {
        return Err("statement_password_file_too_large".into());
    }
    for ending in [&b"\r\n"[..], &b"\n"[..]] {
        if bytes.ends_with(ending) {
            let keep = bytes.len() - ending.len();
            bytes.truncate(keep);
            break;
        }
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|_| "statement_password_file_not_utf8".to_string())?;
    Ok(Zeroizing::new(text.to_string()))
}

fn read_statement(path: &Path) -> Result<Vec<u8>, String> {
    let file = local_file::open_local_file(path, false)
        .map_err(|_| "statement_file_unreadable".to_string())?;
    let limit = MAX_PDF_BYTES as u64;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "statement_file_unreadable".to_string())?;
    if bytes.len() as u64 > limit {
        return Err("statement_file_too_large".into());
    }
    Ok(bytes)
}

/// Where the bundled PDFium library is: beside this executable, unless
/// `BRIDGE_PDFIUM_LIBRARY` names an absolute path to it.
fn pdfium_library() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("BRIDGE_PDFIUM_LIBRARY") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("statement_pdf_engine_setting_invalid".into())
        };
    }
    let executable =
        env::current_exe().map_err(|_| "statement_pdf_engine_unavailable".to_string())?;
    let directory = executable
        .parent()
        .ok_or_else(|| "statement_pdf_engine_unavailable".to_string())?;
    Ok(pdf::platform_library_path(directory))
}

fn run(request: &OwnedRequest, data_dir: &Path) -> Result<Value, String> {
    // Everything that can be refused without the PDF was refused already.
    let bytes = read_statement(&request.statement_path)?;
    let pages = {
        let password = read_password(&request.password_file)?;
        let engine = pdf::engine(&pdfium_library()?).map_err(|refusal| refused(&refusal))?;
        pdf::extract_pages(engine, &bytes, &password).map_err(|refusal| refused(&refusal))?
    };
    let parsed = prepare(
        &pages,
        &StatementRequest {
            bank: request.bank,
            account_label: &request.account_label,
            controls: &request.controls,
            bank_ledger: &request.bank_ledger,
            suspense_ledger: &request.suspense_ledger,
            mapping: &request.mapping,
            date_from: request.date_from,
            date_to: request.date_to,
        },
    )
    .map_err(|refusal| refused(&refusal))?;
    let statement_sha256 = sha256_hex(&bytes);
    let (proposals_id, path, file_sha256) = persist(data_dir, request, &parsed, &statement_sha256)?;
    Ok(summary(
        request,
        &parsed,
        &proposals_id,
        &path,
        &file_sha256,
    ))
}

fn account_last4(account_number: &str) -> String {
    let characters: Vec<char> = account_number.chars().collect();
    characters[characters.len().saturating_sub(4)..]
        .iter()
        .collect()
}

fn persist(
    data_dir: &Path,
    request: &OwnedRequest,
    parsed: &ParsedStatement,
    statement_sha256: &str,
) -> Result<(String, PathBuf, String), String> {
    let directory = data_dir.join(PROPOSALS_DIRECTORY);
    ensure_private_directory(&directory)
        .map_err(|_| "statement_proposals_directory_unavailable".to_string())?;
    let proposals_id = format!("statement-{}", uuid::Uuid::new_v4());
    let document = json!({
        "schema": PROPOSALS_SCHEMA,
        "proposals_id": proposals_id,
        "created_at": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "bank": request.bank.name(),
        "account_last4": account_last4(&parsed.account_number),
        "statement_sha256": statement_sha256,
        "bank_ledger": request.bank_ledger,
        "suspense_ledger": request.suspense_ledger,
        "controls": {
            "opening_balance": request.controls.opening.as_str(),
            "closing_balance": request.controls.closing.as_str(),
            "total_debits": request.controls.debits.as_ref().map(|value| value.as_str()),
            "total_credits": request.controls.credits.as_ref().map(|value| value.as_str()),
        },
        "vouchers": parsed.build.proposals,
        "records": parsed.build.records,
    });
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|_| "statement_proposals_serialization_failed".to_string())?;
    let path = directory.join(format!("{proposals_id}.json"));
    let staged = directory.join(format!(".{proposals_id}.json.partial"));
    // A private write, then a rename: a reader never sees a partial file under
    // the published name.
    let written = (|| {
        let mut file = local_file::open_local_file(&staged, true)
            .map_err(|_| "statement_proposals_write_failed".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "statement_proposals_write_failed".to_string())?;
        }
        std::io::Write::write_all(&mut file, &bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| "statement_proposals_write_failed".to_string())?;
        fs::rename(&staged, &path).map_err(|_| "statement_proposals_write_failed".to_string())
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    Ok((proposals_id, path, sha256_hex(&bytes)))
}

fn summary(
    request: &OwnedRequest,
    parsed: &ParsedStatement,
    proposals_id: &str,
    path: &Path,
    file_sha256: &str,
) -> Value {
    let records = &parsed.build.records;
    let skipped = records
        .iter()
        .filter(|record| record.disposition == Disposition::Skipped)
        .count();
    let counterparties: Vec<Value> = parsed
        .counterparties
        .iter()
        .map(|group| {
            json!({
                "party": party_name(group.party.clone()),
                "also_printed_as": group.also_printed_as.iter().cloned().map(party_name).collect::<Vec<_>>(),
                "ledger": party_name(group.ledger.clone()),
                "disposition": group.disposition,
                "suspense": group.suspense,
                "rows": group.rows,
                "total": group.total,
            })
        })
        .collect();
    let mut ledgers: Vec<&str> =
        std::iter::once(request.bank_ledger.as_str())
            .chain(
                parsed.build.proposals.iter().flat_map(|proposal| {
                    proposal.entries.iter().map(|entry| entry.ledger.as_str())
                }),
            )
            .collect();
    ledgers.sort_unstable();
    ledgers.dedup();
    json!({
        "proposals_id": proposals_id,
        "path": path.to_str().unwrap_or_default(),
        "sha256": file_sha256,
        "bank": request.bank.name(),
        "account_last4": account_last4(&parsed.account_number),
        "statement_rows": parsed.statement_rows,
        "rows_in_window": records.len(),
        "vouchers": parsed.build.proposals.len(),
        "skipped": skipped,
        "suspense_rows": records.iter().filter(|record| record.suspense).count(),
        "bank_ledger_out": format_amount(&parsed.check.bank_out),
        "bank_ledger_in": format_amount(&parsed.check.bank_in),
        "reconciled": {
            "running_balance_every_row": true,
            "closing_balance": parsed.closing.as_str(),
            "total_debits": format_amount(&parsed.totals.debits),
            "total_credits": format_amount(&parsed.totals.credits),
            // false for a layout that prints no totals: they were summed, not checked
            "totals_match_statement": request.controls.debits.is_some(),
        },
        "counterparties": counterparties,
        "ledgers_to_validate": ledgers.into_iter().map(|name| party_name(name.to_string())).collect::<Vec<_>>(),
        "next_step": "Write mapping from counterparties and re-run until no row needs a ledger it should not reach; check ledgers_to_validate with validate_masters. Then call build_import_xml with company_guid, proposals_id and proposals_sha256 set to this proposals_id and sha256. To correct a batch already built from an earlier run of this statement, also pass amends_batch_id; never rebuild it as a new batch.",
    })
}

/// `build_import_xml`'s arguments with a proposals file resolved into
/// `vouchers`, or the arguments unchanged when none is named.
///
/// The file must be one this tool published: named by a well-formed
/// `proposals_id`, under the data directory's `bank-statements/`, a regular
/// file owned by this user with a single link, carrying the declared schema
/// and its own id, and hashing to `proposals_sha256` — the digest the parse
/// returned. A file edited or replaced since is refused rather than built, so
/// the batch is exactly what the summary described.
pub(super) fn resolve_import_arguments(data_dir: &Path, args: &Value) -> Result<Value, String> {
    let Some(object) = args.as_object() else {
        return Err("argument_schema_invalid".into());
    };
    let Some(proposals_id) = object.get("proposals_id") else {
        if object.contains_key("proposals_sha256") {
            return Err("proposals_id_required".into());
        }
        if !object.contains_key("vouchers") {
            return Err("vouchers_required".into());
        }
        return Ok(args.clone());
    };
    if object.contains_key("vouchers") {
        return Err("proposals_id_with_vouchers".into());
    }
    let proposals_id = proposals_id
        .as_str()
        .filter(|id| {
            id.strip_prefix("statement-")
                .is_some_and(catalog::is_uuid_v4_lowercase)
        })
        .ok_or_else(|| "argument_invalid:proposals_id".to_string())?;
    let expected_sha256 = object
        .get("proposals_sha256")
        .ok_or_else(|| "proposals_sha256_required".to_string())?
        .as_str()
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| "argument_invalid:proposals_sha256".to_string())?;
    let path = data_dir
        .join(PROPOSALS_DIRECTORY)
        .join(format!("{proposals_id}.json"));
    let file = local_file::open_local_file(&path, false).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "proposals_not_found".to_string()
        } else {
            "proposals_file_unreadable".to_string()
        }
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_PROPOSALS_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "proposals_file_unreadable".to_string())?;
    if bytes.len() as u64 > MAX_PROPOSALS_FILE_BYTES {
        return Err("proposals_file_too_large".into());
    }
    if sha256_hex(&bytes) != expected_sha256 {
        return Err("proposals_changed".into());
    }
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| "proposals_file_invalid".to_string())?;
    if document["schema"] != PROPOSALS_SCHEMA || document["proposals_id"] != proposals_id {
        return Err("proposals_file_invalid".into());
    }
    let vouchers = document
        .get("vouchers")
        .filter(|vouchers| vouchers.is_array())
        .ok_or_else(|| "proposals_file_invalid".to_string())?;
    let mut resolved = object.clone();
    resolved.remove("proposals_id");
    resolved.remove("proposals_sha256");
    resolved.insert("vouchers".into(), vouchers.clone());
    Ok(Value::Object(resolved))
}

#[cfg(test)]
#[path = "agent_bank_statement_tests.rs"]
mod tests;
