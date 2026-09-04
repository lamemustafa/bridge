//! The local stdio MCP surface. It uses Bridge's loopback-only Tally XML
//! transport. Import XML is only rendered to a local file: this server never
//! dispatches an import or another write request to Tally.

#[path = "agent_import.rs"]
mod agent_import;

use crate::tally::{
    OutstandingsAgeingAnchor, OutstandingsCurrencyAssertion, OutstandingsLoadResult, TallyConfig,
    TallyRuntime, VerifiedCompanyIdentity,
};
use bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile;
use bridge_tally_protocol::TallyCompany;
use bridge_tally_transport::TallyEndpointConfig;
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVER_NAME: &str = "bridge-tally";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_EVIDENCE_RECORDS: usize = 256;
const EGRESS_TAIL_CHUNK_BYTES: usize = 64 * 1024;
const MAX_EGRESS_TAIL_BYTES: usize = 256 * 1024;
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2024-11-05"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Redaction {
    None,
    MaskParties,
    DropNarration,
}

impl Redaction {
    fn from_env() -> Result<Self, String> {
        match env::var("BRIDGE_AGENT_REDACTION") {
            Ok(value) => Self::from_setting(Some(&value)),
            Err(env::VarError::NotPresent) => Self::from_setting(None),
            Err(_) => Err("redaction_setting_invalid".to_string()),
        }
    }

    fn from_setting(value: Option<&str>) -> Result<Self, String> {
        value.map_or(Ok(Self::None), Self::parse)
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "mask_parties" => Ok(Self::MaskParties),
            "drop_narration" => Ok(Self::DropNarration),
            _ => Err("redaction_setting_invalid".to_string()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::MaskParties => "mask_parties",
            Self::DropNarration => "drop_narration",
        }
    }
}

#[derive(Clone)]
struct Settings {
    endpoint: TallyEndpointConfig,
    data_dir: PathBuf,
    max_rows: usize,
    max_bytes: usize,
    redaction: Redaction,
    import_enabled: bool,
}

impl Settings {
    fn from_env() -> Result<Self, String> {
        let host = env::var("BRIDGE_TALLY_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = env::var("BRIDGE_TALLY_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(9000);
        let max_rows = bounded_env("BRIDGE_AGENT_MAX_ROWS", 500, 1, 10_000);
        let max_bytes = bounded_env("BRIDGE_AGENT_MAX_BYTES", 200_000, 256, 5_000_000);
        // The transport remains the authoritative hard cap.  The agent cap only
        // narrows it and is applied to the response before parsing/returning.
        let data_dir = env::var_os("BRIDGE_AGENT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        fs::create_dir_all(&data_dir).map_err(|_| "agent_data_dir_unavailable".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o700))
                .map_err(|_| "agent_data_dir_permissions_failed".to_string())?;
        }
        Ok(Self {
            endpoint: TallyEndpointConfig { host, port },
            data_dir,
            max_rows,
            max_bytes,
            redaction: Redaction::from_env()?,
            import_enabled: env::var("BRIDGE_AGENT_ENABLE_IMPORT").as_deref() == Ok("1"),
        })
    }
}

fn bounded_env(name: &str, default: usize, min: usize, max: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (*value >= min) && (*value <= max))
        .unwrap_or(default)
}

fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data).join("Bridge").join("agent");
        }
        if let Some(app_data) = env::var_os("APPDATA") {
            return PathBuf::from(app_data).join("Bridge").join("agent");
        }
        return PathBuf::from("Bridge").join("agent");
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Bridge");
        }
    }
    env::temp_dir().join("bridge")
}

#[derive(Clone, Serialize)]
struct Evidence {
    request_sha256: String,
    response_sha256: String,
    bytes: usize,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
}

#[derive(Serialize)]
struct EgressReceipt<'a> {
    ts: String,
    tool: &'a str,
    args_sha256: String,
    company_guid: Option<String>,
    rows_returned: usize,
    fields_returned: Vec<String>,
    bytes_returned: usize,
    enforced_bytes: usize,
    response_sha256: String,
    truncated: bool,
    redaction_preset: &'a str,
}

struct Server {
    settings: Settings,
    runtime: TallyRuntime,
    evidence: Arc<Mutex<Vec<Evidence>>>,
}

type ToolOutcome = (Value, Evidence, Option<String>, usize, Vec<String>, bool);

impl Server {
    fn new(settings: Settings) -> Self {
        Self {
            settings,
            runtime: TallyRuntime::default(),
            evidence: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn tally_config(&self) -> TallyConfig {
        self.settings.endpoint.clone()
    }

    async fn post_read(
        &self,
        identity: &VerifiedCompanyIdentity,
        request: String,
    ) -> Result<(String, Evidence), String> {
        // This is the final in-process boundary: all Tally traffic emitted by
        // bridge-mcp must be a read. Keeping it here makes an accidental future
        // call site fail closed before bytes leave the loopback transport.
        if request
            .to_ascii_lowercase()
            .contains("<tallyrequest>import")
        {
            return Err("agent_write_dispatch_forbidden".to_string());
        }
        let request_sha256 = sha256_hex(request.as_bytes());
        let response = self
            .runtime
            .fetch_agent_read(self.tally_config(), identity, request)
            .await
            .map_err(|_| "agent_runtime_read_failed".to_string())?;
        let evidence = Evidence {
            request_sha256,
            response_sha256: sha256_hex(response.as_bytes()),
            bytes: response.len(),
            state: "complete",
            reason_code: None,
        };
        Ok((response, evidence))
    }

    async fn status(&self) -> Result<(Value, Evidence), String> {
        let probe = self
            .runtime
            .probe(self.tally_config())
            .await
            .map_err(|_| "status_probe_unavailable".to_string())?;
        let observed = serde_json::to_value(&probe)
            .map_err(|_| "status_probe_observation_invalid".to_string())?;
        Ok((
            json!({
                "product": serde_json::to_value(&probe.connection.product).unwrap_or_else(|_| json!("not_observed")),
                "release": probe.profile.release,
                "education_mode": probe.profile.mode,
                "endpoint": format!("http://{}:{}", self.settings.endpoint.host, self.settings.endpoint.port),
                "loaded_companies": probe.companies,
                "refusal_reason": Value::Null,
            }),
            Evidence {
                request_sha256: sha256_hex(b"runtime.probe"),
                response_sha256: sha256_json(&observed),
                bytes: observed.to_string().len(),
                state: "complete",
                reason_code: None,
            },
        ))
    }

    async fn companies(&self) -> Result<(Vec<TallyCompany>, Evidence), String> {
        let companies = self
            .runtime
            .fetch_companies(self.tally_config())
            .await
            .map_err(|_| "company_collection_invalid".to_string())?;
        let evidence = Evidence {
            request_sha256: sha256_hex(ReadOnlyProfile::CompanyListV2.render().as_bytes()),
            response_sha256: sha256_json(&companies),
            bytes: 0,
            state: "complete",
            reason_code: None,
        };
        Ok((companies, evidence))
    }

    async fn verified_company(
        &self,
        guid: &str,
    ) -> Result<(TallyCompany, VerifiedCompanyIdentity, Evidence), String> {
        if guid.trim().is_empty() {
            return Err("company_guid_required".to_string());
        }
        let (companies, evidence) = self.companies().await?;
        let observed_companies = companies.clone();
        let matches = companies
            .into_iter()
            .filter(|company| {
                company
                    .guid
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(guid))
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(if matches.is_empty() {
                "company_identity_not_found".to_string()
            } else {
                "company_identity_ambiguous".to_string()
            });
        }
        let company = matches.into_iter().next().expect("one checked above");
        let company_number = company
            .company_number
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "company_identity_incomplete".to_string())?;
        let books_from = company
            .books_from
            .clone()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "company_identity_incomplete".to_string())?;
        let identity = VerifiedCompanyIdentity::from_observed_companies(
            company.name.clone(),
            guid.to_string(),
            company_number,
            books_from,
            &observed_companies,
        )
        .map_err(|error| {
            match error {
                crate::tally::VerifiedCompanyIdentityError::Missing => "company_identity_not_found",
                crate::tally::VerifiedCompanyIdentityError::DuplicateTuple => {
                    "company_identity_ambiguous"
                }
                crate::tally::VerifiedCompanyIdentityError::DisplayScopeAmbiguous => {
                    "company_display_scope_ambiguous"
                }
            }
            .to_string()
        })?;
        Ok((company, identity, evidence))
    }

    async fn call_tool(&self, name: &str, args: Value) -> Value {
        let args_sha256 = sha256_json(&args);
        let started = Utc::now();
        let result = self.tool_payload(name, &args).await;
        let (payload, evidence, company_guid, rows, fields, truncated) = match result {
            Ok(outcome) => outcome,
            Err(code) => {
                let evidence = Evidence {
                    request_sha256: sha256_hex(format!("{name}:{args_sha256}").as_bytes()),
                    response_sha256: sha256_hex(code.as_bytes()),
                    bytes: 0,
                    state: "partial",
                    reason_code: Some(code.clone()),
                };
                (
                    json!({"error": {"code": code, "message": "Bridge withheld this read."}}),
                    evidence,
                    arg_string(&args, "company_guid"),
                    0,
                    Vec::new(),
                    false,
                )
            }
        };
        let response_value = redact_value(
            json!({
                "company": payload.get("company").cloned().unwrap_or_else(|| json!({"state":"not_company_scoped"})),
                "read_at": started.to_rfc3339_opts(SecondsFormat::Millis, true),
                "evidence": evidence,
                "truncated": truncated,
                "result": payload.get("result").cloned().unwrap_or(payload),
            }),
            self.settings.redaction,
        );
        let (response_value, bytes_truncated) = match enforce_response_byte_cap(
            response_value,
            self.settings.max_bytes,
        ) {
            Ok(value) => value,
            Err(code) => {
                return json!({
                    "content": [{"type":"text", "text": format!("{name}: read withheld\\n{code}")}],
                    "structuredContent": {"error": {"code": code, "message": "Bridge response exceeds the configured byte cap."}},
                    "isError": true,
                });
            }
        };
        let truncated = truncated || bytes_truncated;
        let response_sha256 = sha256_json(&response_value);
        self.record_evidence(response_value["evidence"].clone());
        if let Err(code) = self.append_egress(EgressReceipt {
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            tool: name,
            args_sha256,
            company_guid,
            rows_returned: rows,
            fields_returned: fields,
            bytes_returned: response_value.to_string().len(),
            enforced_bytes: self.settings.max_bytes,
            response_sha256,
            truncated,
            redaction_preset: self.settings.redaction.label(),
        }) {
            return json!({
                "content": [{"type":"text", "text": format!("{name}: read withheld\\n{code}")}],
                "structuredContent": {"error": {"code": code, "message": "Bridge could not record egress."}},
                "isError": true,
            });
        }
        let summary = if response_value.get("result").is_some()
            && response_value["result"].get("error").is_none()
        {
            format!("{name}: read completed")
        } else {
            format!("{name}: read withheld")
        };
        json!({
            "content": [{"type":"text", "text": format!("{summary}\n{}", response_value)}],
            "structuredContent": response_value,
            "isError": response_value["result"].get("error").is_some(),
        })
    }

    fn record_evidence(&self, value: Value) {
        let evidence = Evidence {
            request_sha256: value
                .get("request_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            response_sha256: value
                .get("response_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            bytes: value
                .get("bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize,
            state: if value.get("state").and_then(Value::as_str) == Some("complete") {
                "complete"
            } else {
                "partial"
            },
            reason_code: value
                .get("reason_code")
                .and_then(Value::as_str)
                .map(str::to_string),
        };
        {
            let mut records = self.evidence.lock().expect("evidence mutex");
            records.push(evidence);
            if records.len() > MAX_EVIDENCE_RECORDS {
                records.remove(0);
            }
        }
    }

    fn append_egress(&self, receipt: EgressReceipt<'_>) -> Result<(), String> {
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|_| "egress_record_write_failed".to_string())?;
        writeln!(
            file,
            "{}",
            serde_json::to_string(&receipt).map_err(|_| "egress_record_write_failed".to_string())?
        )
        .map_err(|_| "egress_record_write_failed".to_string())?;
        file.sync_data()
            .map_err(|_| "egress_record_write_failed".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "egress_record_write_failed".to_string())?;
        }
        Ok(())
    }

    async fn tool_payload(&self, name: &str, args: &Value) -> Result<ToolOutcome, String> {
        match name {
            "tally_status" => {
                let (result, evidence) = self.status().await?;
                Ok((json!({"result": result}), evidence, None, 0, vec![], false))
            }
            "list_companies" => {
                let (companies, evidence) = self.companies().await?;
                let flagged = companies
                    .iter()
                    .map(|company| company_json(company, &companies))
                    .collect::<Vec<_>>();
                Ok((
                    json!({"result": {"companies": flagged}}),
                    evidence,
                    None,
                    companies.len(),
                    vec![
                        "name".into(),
                        "guid".into(),
                        "company_number".into(),
                        "books_from".into(),
                    ],
                    false,
                ))
            }
            "voucher_schema" => self.voucher_schema(),
            "validate_masters" => self.validate_masters(args).await,
            "build_import_xml" => {
                self.import_enabled()?;
                self.build_import_xml(args).await
            }
            "verify_import" => {
                self.import_enabled()?;
                self.verify_import(args).await
            }
            "ledger_masters" => self.ledger_masters(args).await,
            "vouchers" => self.vouchers(args).await,
            "changed_since" => self.changed_since(args).await,
            "outstandings" => self.outstandings(args).await,
            "ledger_movement" => self.ledger_movement(args).await,
            "read_evidence" => self.read_evidence(args),
            "egress_log" => self.egress_log(args),
            _ => Err("tool_not_found".to_string()),
        }
    }

    fn import_enabled(&self) -> Result<(), String> {
        self.settings
            .import_enabled
            .then_some(())
            .ok_or_else(|| "import_unverified_on_live_tally".to_string())
    }

    async fn ledger_masters(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, company_evidence) = self.verified_company(guid).await?;
        let compliance = args.get("fields").and_then(Value::as_str) == Some("compliance");
        let mut ledgers = if compliance {
            self.runtime
                .fetch_agent_party_ledger_masters(self.tally_config(), &identity)
                .await
                .map_err(|_| "party_ledger_master_read_failed".to_string())?
                .into_iter()
                .map(|record| {
                    json!({
                        "name": record.ledger.name,
                        "parent": record.ledger.parent.returned_text(),
                        "opening_balance": record.ledger.opening_balance,
                        "party_gstin": record.ledger.party_gstin.returned_text(),
                        "compliance": record.fields,
                    })
                })
                .collect::<Vec<_>>()
        } else {
            self.runtime
                .fetch_ledgers(self.tally_config(), &identity)
                .await
                .map_err(|_| "ledger_export_invalid".to_string())?
                .into_iter()
                .map(|ledger| {
                    json!({
                        "name": ledger.name,
                        "parent": ledger.parent.returned_text(),
                        "opening_balance": ledger.opening_balance,
                        "party_gstin": ledger.party_gstin.returned_text(),
                    })
                })
                .collect::<Vec<_>>()
        };
        if let Some(group) = arg_string(args, "group") {
            ledgers.retain(|ledger| ledger["parent"].as_str() == Some(group.as_str()));
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit = arg_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let total = ledgers.len();
        let page = ledgers
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|ledger| redact_value(ledger, self.settings.redaction))
            .collect::<Vec<_>>();
        let page_len = page_length_after_offset(total, offset, limit);
        let truncated = offset.saturating_add(page.len()) < total;
        let result = json!({"items": page, "offset": offset, "total": total, "fields": args.get("fields").and_then(Value::as_str).unwrap_or("basic"), "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            company_evidence,
            Some(guid.to_string()),
            page_len,
            if compliance {
                vec![
                    "name".into(),
                    "parent".into(),
                    "opening_balance".into(),
                    "compliance".into(),
                ]
            } else {
                vec!["name".into(), "parent".into(), "opening_balance".into()]
            },
            truncated,
        ))
    }

    async fn vouchers(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        if from > to {
            return Err("invalid_date_range".to_string());
        }
        let (company, identity, identity_evidence) = self.verified_company(guid).await?;
        let request = render_agent_vouchers(
            &company.name,
            &from,
            &to,
            arg_string(args, "voucher_type"),
            arg_string(args, "ledger"),
            None,
        )?;
        let (xml, evidence) = self.post_read(&identity, request).await?;
        let mut rows = parse_agent_rows(&xml)?;
        if !window_honoured(&rows, &from, &to) {
            return Err("window_not_honoured".to_string());
        }
        if rows.is_empty() {
            let (wider_from, wider_to) = widened_window(&from, &to)?;
            let wider_request = render_agent_vouchers(
                &company.name,
                &wider_from,
                &wider_to,
                arg_string(args, "voucher_type"),
                arg_string(args, "ledger"),
                None,
            )?;
            let (wider_xml, _) = self.post_read(&identity, wider_request).await?;
            let wider_rows = parse_agent_rows(&wider_xml)?;
            if !window_honoured(&wider_rows, &wider_from, &wider_to) {
                return Err("empty_uncorroborated".to_string());
            }
        }
        if let Some(kind) = arg_string(args, "voucher_type") {
            rows.retain(|row| row.get("voucher_type") == Some(&Value::String(kind.clone())));
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit = arg_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let total = rows.len();
        let items = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| redact_value(row, self.settings.redaction))
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(items.len()) < total;
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"items": items, "offset": offset, "total": total, "profile": "agent_vouchers_v1_filters"}}),
            combine_evidence(identity_evidence, evidence),
            Some(guid.to_string()),
            items.len(),
            vec![
                "date".into(),
                "voucher_number".into(),
                "voucher_type".into(),
                "party".into(),
                "amounts".into(),
                "narration".into(),
                "guid".into(),
                "alter_id".into(),
                "master_id".into(),
            ],
            truncated,
        ))
    }

    async fn changed_since(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let voucher_alter_id = checkpoint_arg(args, "voucher_alter_id")?
            .or(checkpoint_arg(args, "alter_id")?)
            .unwrap_or(0);
        let master_alter_id = checkpoint_arg(args, "master_alter_id")?
            .or(checkpoint_arg(args, "alter_id")?)
            .unwrap_or(0);
        let (company, identity, identity_evidence) = self.verified_company(guid).await?;
        let supplied_voucher_snapshot = checkpoint_arg(args, "voucher_snapshot_alter_id")?;
        let supplied_master_snapshot = checkpoint_arg(args, "master_snapshot_alter_id")?;
        let (snapshot, snapshot_evidence) =
            match (supplied_voucher_snapshot, supplied_master_snapshot) {
                (Some(voucher), Some(master)) => (
                    json!({"altvchid": voucher, "altmstid": master}),
                    Evidence {
                        request_sha256: sha256_hex(b"agent_change_snapshot_from_cursor"),
                        response_sha256: sha256_json(
                            &json!({"altvchid": voucher, "altmstid": master}),
                        ),
                        bytes: 0,
                        state: "complete",
                        reason_code: None,
                    },
                ),
                (None, None) => {
                    let (xml, evidence) = self
                        .post_read(&identity, render_agent_company_high_water(&company.name))
                        .await?;
                    (parse_company_high_water(&xml, guid)?, evidence)
                }
                _ => return Err("change_snapshot_incomplete".to_string()),
            };
        let voucher_snapshot = snapshot["altvchid"]
            .as_u64()
            .ok_or_else(|| "voucher_checkpoint_invalid".to_string())?;
        let master_snapshot = snapshot["altmstid"]
            .as_u64()
            .ok_or_else(|| "master_checkpoint_invalid".to_string())?;
        if voucher_alter_id > voucher_snapshot || master_alter_id > master_snapshot {
            return Err("change_checkpoint_exceeds_snapshot".to_string());
        }
        let request =
            render_agent_changed_vouchers(&company.name, voucher_alter_id, voucher_snapshot);
        let (xml, evidence) = self.post_read(&identity, request).await?;
        let all_rows = parse_agent_rows(&xml)?;
        let (rows, voucher_truncated, truncated_voucher_cursor) = stable_change_page(
            all_rows,
            voucher_alter_id,
            voucher_snapshot,
            self.settings.max_rows,
        )?;
        let (master_xml, master_evidence) = self
            .post_read(
                &identity,
                render_agent_changed_masters(&company.name, master_alter_id, master_snapshot),
            )
            .await?;
        let all_masters = parse_agent_changed_masters(&master_xml)?;
        let (masters, master_truncated, truncated_master_cursor) = stable_change_page(
            all_masters,
            master_alter_id,
            master_snapshot,
            self.settings.max_rows,
        )?;
        let masters = masters
            .into_iter()
            .map(|master| redact_value(master, self.settings.redaction))
            .collect::<Vec<_>>();
        let voucher_checkpoint_advanceable = checkpoint_advanceable(
            rows.iter().filter_map(|row| row["alter_id"].as_u64()).max(),
            voucher_alter_id,
            voucher_snapshot,
            voucher_truncated,
        );
        let master_checkpoint_advanceable = checkpoint_advanceable(
            masters
                .iter()
                .filter_map(|row| row["alter_id"].as_u64())
                .max(),
            master_alter_id,
            master_snapshot,
            master_truncated,
        );
        let checkpoint_advanceable =
            voucher_checkpoint_advanceable && master_checkpoint_advanceable;
        let next_voucher_alter_id = if voucher_truncated {
            truncated_voucher_cursor
        } else if voucher_checkpoint_advanceable {
            voucher_snapshot
        } else {
            voucher_alter_id
        };
        let next_master_alter_id = if master_truncated {
            truncated_master_cursor
        } else if master_checkpoint_advanceable {
            master_snapshot
        } else {
            master_alter_id
        };
        let returned_rows = rows.len() + masters.len();
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"vouchers": rows, "masters": masters, "voucher_alter_id": voucher_alter_id, "master_alter_id": master_alter_id, "voucher_snapshot_alter_id": voucher_snapshot, "master_snapshot_alter_id": master_snapshot, "next_voucher_alter_id": next_voucher_alter_id, "next_master_alter_id": next_master_alter_id, "checkpoint_advanceable": checkpoint_advanceable, "checkpoint_reason": (!checkpoint_advanceable).then_some("scan_not_correlated_to_company_high_water"), "deletion_detection": "unsupported_alterid_does_not_observe_deletions", "current_company_high_water": snapshot}}),
            combine_evidence(
                combine_evidence(identity_evidence, evidence),
                combine_evidence(master_evidence, snapshot_evidence),
            ),
            Some(guid.to_string()),
            returned_rows,
            vec!["alter_id".into(), "name".into(), "kind".into()],
            voucher_truncated || master_truncated,
        ))
    }

    async fn outstandings(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, identity_evidence) = self.verified_company(guid).await?;
        let as_of = args
            .get("as_of")
            .and_then(Value::as_str)
            .map(normalized_date)
            .transpose()?
            .unwrap_or_else(tally_host_today);
        let to =
            bridge_tally_core::TallyDate::parse(as_of).map_err(|_| "invalid_as_of".to_string())?;
        let ageing_anchor = match args
            .get("ageing_basis")
            .and_then(Value::as_str)
            .unwrap_or("due_date")
        {
            "bill_date" => OutstandingsAgeingAnchor::BillDate,
            "due_date" => OutstandingsAgeingAnchor::DueDate,
            _ => return Err("invalid_ageing_basis".to_string()),
        };
        let currency = self
            .runtime
            .detect_base_currency(self.tally_config(), &identity)
            .await
            .map_err(|_| "company_currency_probe_failed".to_string())?;
        let assertion = match (currency.currency_count, currency.is_inr) {
            (1, true) => OutstandingsCurrencyAssertion::Inr,
            (0, _) => return Err("company_currency_probe_failed".to_string()),
            (1, false) => return Err("company_base_currency_not_inr".to_string()),
            _ => return Err("company_base_currency_undetermined".to_string()),
        };
        let load = self
            .runtime
            .fetch_outstandings(self.tally_config(), &identity, to, assertion, ageing_anchor)
            .await
            .map_err(|_| "native_outstandings_read_failed".to_string())?;
        let top = arg_usize(args, "top", 25)?.min(self.settings.max_rows);
        let bill_offset = arg_usize(args, "offset", 0)?;
        let bill_limit =
            arg_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let mut result_evidence = identity_evidence;
        let (result, bills_truncated) = match load {
            OutstandingsLoadResult::Complete {
                report,
                statement_open_bills,
                statement_unallocated_by_party,
                unallocated_total,
                ..
            } => {
                let direction = args
                    .get("direction")
                    .and_then(Value::as_str)
                    .unwrap_or("both");
                if !matches!(direction, "receivable" | "payable" | "both") {
                    return Err("invalid_direction".to_string());
                }
                let all_bills = statement_open_bills
                    .into_iter()
                    .filter(|bill| {
                        direction == "both" || bill.kind.label().eq_ignore_ascii_case(direction)
                    })
                    .collect::<Vec<_>>();
                let (bills, bills_truncated, next_bill_offset) =
                    paginate_open_bills(all_bills, bill_offset, bill_limit);
                let bills = bills
                    .into_iter()
                    .map(|bill| {
                        redact_value(
                            serde_json::to_value(bill).unwrap_or_default(),
                            self.settings.redaction,
                        )
                    })
                    .collect::<Vec<_>>();
                let parties = report
                    .top_parties
                    .into_iter()
                    .take(top)
                    .map(|party| {
                        redact_value(
                            serde_json::to_value(party).unwrap_or_default(),
                            self.settings.redaction,
                        )
                    })
                    .collect::<Vec<_>>();
                let unallocated = statement_unallocated_by_party
                    .into_iter()
                    .map(|party| {
                        redact_value(
                            serde_json::to_value(party).unwrap_or_default(),
                            self.settings.redaction,
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    json!({"state":"complete", "totals":{"receivable": report.receivable_total, "payable": report.payable_total}, "ageing_basis": if matches!(ageing_anchor, OutstandingsAgeingAnchor::BillDate) {"bill_date"} else {"due_date"}, "ageing_buckets": report.ageing, "top_parties": parties, "open_bills": bills, "offset": bill_offset, "limit": bill_limit, "next_offset": next_bill_offset, "unallocated":{"count": unallocated.len(), "amount": unallocated_total, "parties": unallocated}}),
                    bills_truncated,
                )
            }
            OutstandingsLoadResult::Partial { reason, .. } => {
                result_evidence.state = "partial";
                result_evidence.reason_code = Some(reason.reason_code.clone());
                (
                    json!({"state":"partial", "partial_reason": reason.reason_code}),
                    false,
                )
            }
        };
        let rows = result["open_bills"].as_array().map_or(0, Vec::len);
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            result_evidence,
            Some(guid.to_string()),
            rows,
            vec!["party".into(), "reference".into(), "amount".into()],
            bills_truncated,
        ))
    }

    async fn ledger_movement(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        if from > to {
            return Err("invalid_date_range".to_string());
        }
        let (company, identity, evidence) = self.verified_company(guid).await?;
        let ledgers = self
            .runtime
            .fetch_ledgers(self.tally_config(), &identity)
            .await
            .map_err(|_| "ledger_movement_read_failed".to_string())?;
        let books_from = normalized_date(
            company
                .books_from
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?;
        let pre_window = if books_from < from {
            let before_from = NaiveDate::parse_from_str(&from, "%Y%m%d")
                .map_err(|_| "invalid_date".to_string())?
                .checked_sub_signed(Duration::days(1))
                .ok_or_else(|| "invalid_date".to_string())?
                .format("%Y%m%d")
                .to_string();
            self.runtime
                .fetch_vouchers(
                    self.tally_config(),
                    &identity,
                    books_from.clone(),
                    before_from,
                )
                .await
                .map_err(|_| "ledger_movement_read_failed".to_string())?
        } else {
            Vec::new()
        };
        let vouchers = self
            .runtime
            .fetch_vouchers(self.tally_config(), &identity, from.clone(), to)
            .await
            .map_err(|_| "ledger_movement_read_failed".to_string())?;
        let pre_window = balance_affecting_vouchers(pre_window)?;
        let vouchers = balance_affecting_vouchers(vouchers)?;
        let selected = arg_string(args, "ledger");
        let mut movement = BTreeMap::<
            String,
            (
                Option<String>,
                Option<String>,
                String,
                String,
                String,
                usize,
            ),
        >::new();
        for ledger in ledgers {
            if selected.as_deref().is_none_or(|name| name == ledger.name) {
                movement.insert(
                    ledger.name,
                    (
                        ledger.parent.returned_text().map(str::to_string),
                        ledger.opening_balance,
                        "0".into(),
                        "0".into(),
                        "0".into(),
                        0,
                    ),
                );
            }
        }
        for voucher in &pre_window {
            for entry in &voucher.ledger_entries {
                let Some(record) = movement.get_mut(&entry.ledger_name) else {
                    continue;
                };
                let opening = record.1.as_deref().unwrap_or("0");
                record.1 = Some(add_decimal(opening, &entry.amount)?);
            }
        }
        for voucher in &vouchers {
            let mut touched = std::collections::BTreeSet::new();
            for entry in &voucher.ledger_entries {
                let Some(record) = movement.get_mut(&entry.ledger_name) else {
                    continue;
                };
                let amount = bridge_tally_core::ExactDecimal::parse(entry.amount.clone())
                    .map_err(|_| "voucher_amount_invalid".to_string())?;
                let magnitude = amount
                    .abs()
                    .map_err(|_| "voucher_amount_invalid".to_string())?
                    .as_str()
                    .to_string();
                if entry.is_deemed_positive {
                    record.2 = add_decimal(&record.2, &format!("-{magnitude}"))?;
                } else {
                    record.3 = add_decimal(&record.3, &magnitude)?;
                }
                touched.insert(entry.ledger_name.clone());
            }
            for ledger in touched {
                if let Some(record) = movement.get_mut(&ledger) {
                    record.5 += 1;
                }
            }
        }
        let rows = movement.into_iter().map(|(name, (parent, opening, debit, credit, _, vouchers_touching))| -> Result<Value, String> {
            let closing = opening.as_deref().map(|opening| {
                let debited = add_decimal(opening, &debit)?;
                add_decimal(&debited, &credit)
            }).transpose()?;
            Ok(redact_value(json!({"ledger": name, "parent": parent, "opening": opening, "debit": debit, "credit": credit, "closing": closing, "vouchers_touching": vouchers_touching}), self.settings.redaction))
        }).collect::<Result<Vec<_>, _>>()?;
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"ledgers": rows, "voucher_rows_observed": vouchers.len(), "evidence_method": if books_from < from {"runtime_ledger_opening_plus_pre_window_entries_to_from"} else {"runtime_ledger_opening_at_books_from_plus_literal_window_entries"}}}),
            evidence,
            Some(guid.to_string()),
            vouchers.len(),
            vec![
                "ledger".into(),
                "opening".into(),
                "debit".into(),
                "credit".into(),
                "closing".into(),
            ],
            false,
        ))
    }

    fn read_evidence(&self, args: &Value) -> Result<ToolOutcome, String> {
        let take = arg_usize(args, "limit", 20)?.min(MAX_EVIDENCE_RECORDS);
        let records = self
            .evidence
            .lock()
            .map_err(|_| "evidence_store_unavailable".to_string())?;
        let values = records.iter().rev().take(take).cloned().collect::<Vec<_>>();
        let evidence = Evidence {
            request_sha256: sha256_hex(b"read_evidence"),
            response_sha256: sha256_json(&values),
            bytes: 0,
            state: "complete",
            reason_code: None,
        };
        Ok((
            json!({"result": {"records": values}}),
            evidence,
            None,
            values.len(),
            vec![
                "request_sha256".into(),
                "response_sha256".into(),
                "bytes".into(),
                "state".into(),
            ],
            false,
        ))
    }

    fn egress_log(&self, args: &Value) -> Result<ToolOutcome, String> {
        let take = arg_usize(args, "limit", 20)?.min(MAX_EVIDENCE_RECORDS);
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let lines = read_egress_tail(&path, take)?;
        let evidence = Evidence {
            request_sha256: sha256_hex(b"egress_log"),
            response_sha256: sha256_json(&lines),
            bytes: 0,
            state: "complete",
            reason_code: None,
        };
        Ok((
            json!({"result": {"records": lines}}),
            evidence,
            None,
            lines.len(),
            vec![
                "ts".into(),
                "tool".into(),
                "args_sha256".into(),
                "response_sha256".into(),
            ],
            false,
        ))
    }
}

fn company_json(company: &TallyCompany, all: &[TallyCompany]) -> Value {
    let guid = company.guid.clone();
    let duplicate_guid = guid.as_deref().is_some_and(|candidate| {
        all.iter()
            .filter(|other| {
                other
                    .guid
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(candidate))
            })
            .count()
            > 1
    });
    let missing = [
        ("name", Some(company.name.as_str())),
        ("guid", company.guid.as_deref()),
        ("company_number", company.company_number.as_deref()),
        ("books_from", company.books_from.as_deref()),
    ]
    .into_iter()
    .find_map(|(field, value)| {
        value
            .filter(|value| !value.trim().is_empty())
            .is_none()
            .then_some(field)
    });
    json!({"name": company.name, "guid": guid, "company_number": company.company_number, "books_from": company.books_from, "identity_state": if duplicate_guid {"ambiguous_duplicate_guid"} else if missing.is_some() {"incomplete_tuple"} else {"verified_tuple"}, "missing_field": missing})
}

/// A balance must never be derived from a voucher whose accounting effect is
/// cancelled or optional. Missing flags are equally unsafe: the agent has no
/// evidence that the row belongs in a financial aggregation.
fn balance_affecting_vouchers(
    vouchers: Vec<bridge_tally_protocol::TallyVoucher>,
) -> Result<Vec<bridge_tally_protocol::TallyVoucher>, String> {
    vouchers
        .into_iter()
        .map(|voucher| match (voucher.cancelled, voucher.optional) {
            (Some(false), Some(false)) => Ok(Some(voucher)),
            (Some(true), _) | (_, Some(true)) => Ok(None),
            _ => Err("voucher_accounting_state_not_observed".to_string()),
        })
        .filter_map(Result::transpose)
        .collect()
}

fn page_length_after_offset(total: usize, offset: usize, limit: usize) -> usize {
    total.saturating_sub(offset).min(limit)
}

fn page_is_truncated(total: usize, offset: usize, page_len: usize) -> bool {
    offset.saturating_add(page_len) < total
}

fn read_egress_tail(path: &Path, take: usize) -> Result<Vec<String>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("egress_log_unreadable".to_string()),
    };
    let file_len = file
        .metadata()
        .map_err(|_| "egress_log_unreadable".to_string())?
        .len();
    let mut position = file_len;
    let mut scanned = 0_usize;
    let mut newlines = 0_usize;
    let mut chunks = Vec::new();
    while position > 0 && scanned < MAX_EGRESS_TAIL_BYTES && newlines < take.saturating_add(1) {
        let chunk_size = EGRESS_TAIL_CHUNK_BYTES
            .min(MAX_EGRESS_TAIL_BYTES - scanned)
            .min(position as usize);
        position -= chunk_size as u64;
        file.seek(SeekFrom::Start(position))
            .map_err(|_| "egress_log_unreadable".to_string())?;
        let mut chunk = vec![0_u8; chunk_size];
        file.read_exact(&mut chunk)
            .map_err(|_| "egress_log_unreadable".to_string())?;
        newlines += chunk.iter().filter(|byte| **byte == b'\n').count();
        scanned += chunk_size;
        chunks.push(chunk);
    }
    chunks.reverse();
    let bytes = chunks.concat();
    let text = std::str::from_utf8(&bytes).map_err(|_| "egress_log_unreadable".to_string())?;
    let text = if position > 0 {
        text.split_once('\n').map_or("", |(_, tail)| tail)
    } else {
        text
    };
    Ok(text.lines().rev().take(take).map(str::to_string).collect())
}

fn paginate_open_bills<T>(
    bills: Vec<T>,
    offset: usize,
    limit: usize,
) -> (Vec<T>, bool, Option<usize>) {
    let total = bills.len();
    let page = bills
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect::<Vec<_>>();
    let truncated = page_is_truncated(total, offset, page.len());
    let next_offset = truncated.then_some(offset + page.len());
    (page, truncated, next_offset)
}

fn window_honoured(rows: &[Value], from: &str, to: &str) -> bool {
    rows.iter().all(|row| {
        row.get("date")
            .and_then(Value::as_str)
            .is_some_and(|date| date >= from && date <= to)
    })
}

fn widened_window(from: &str, to: &str) -> Result<(String, String), String> {
    let from = NaiveDate::parse_from_str(from, "%Y%m%d").map_err(|_| "invalid_date".to_string())?;
    let to = NaiveDate::parse_from_str(to, "%Y%m%d").map_err(|_| "invalid_date".to_string())?;
    Ok((
        from.checked_sub_signed(Duration::days(1))
            .ok_or_else(|| "empty_uncorroborated".to_string())?
            .format("%Y%m%d")
            .to_string(),
        to.checked_add_signed(Duration::days(1))
            .ok_or_else(|| "empty_uncorroborated".to_string())?
            .format("%Y%m%d")
            .to_string(),
    ))
}

/// Tally is local to the Bridge host; accounting-day defaults therefore use
/// that host's calendar rather than UTC's potentially different calendar day.
pub(super) fn tally_host_today() -> String {
    format_tally_date(Local::now())
}

fn format_tally_date<Tz: TimeZone>(now: DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    now.format("%Y%m%d").to_string()
}

fn enforce_response_byte_cap(
    mut response: Value,
    max_bytes: usize,
) -> Result<(Value, bool), String> {
    if response.to_string().len() <= max_bytes {
        return Ok((response, false));
    }
    let offset = response["result"]["offset"].as_u64().unwrap_or(0);
    let Some(items) = response["result"]["items"].as_array() else {
        return Err("agent_response_too_large".to_string());
    };
    if items.is_empty() {
        return Err("agent_response_too_large".to_string());
    }
    response["truncated"] = Value::Bool(true);
    while response["result"]["items"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
        && response.to_string().len() > max_bytes
    {
        let remaining = {
            let items = response["result"]["items"]
                .as_array_mut()
                .expect("items checked above");
            items.pop();
            items.len()
        };
        response["result"]["next_offset"] = json!(offset + remaining as u64);
    }
    if response["result"]["items"]
        .as_array()
        .is_none_or(Vec::is_empty)
        || response.to_string().len() > max_bytes
    {
        return Err("agent_response_too_large".to_string());
    }
    Ok((response, true))
}

fn combine_evidence(left: Evidence, right: Evidence) -> Evidence {
    Evidence {
        request_sha256: sha256_hex(
            format!("{}:{}", left.request_sha256, right.request_sha256).as_bytes(),
        ),
        response_sha256: sha256_hex(
            format!("{}:{}", left.response_sha256, right.response_sha256).as_bytes(),
        ),
        bytes: left.bytes + right.bytes,
        state: if left.state == "complete" && right.state == "complete" {
            "complete"
        } else {
            "partial"
        },
        reason_code: left.reason_code.or(right.reason_code),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn negotiate_protocol(requested: &str) -> Result<&'static str, String> {
    SUPPORTED_PROTOCOL_VERSIONS
        .into_iter()
        .find(|version| *version == requested)
        .ok_or_else(|| "unsupported_protocol_version".to_string())
}

fn sha256_json<T: Serialize>(value: &T) -> String {
    sha256_hex(serde_json::to_vec(value).unwrap_or_default().as_slice())
}
fn arg_string(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}
fn required_string<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key}_required"))
}
fn arg_usize(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    match args.get(key) {
        None => Ok(default),
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| "pagination_invalid".to_string()),
        Some(_) => Err("pagination_invalid".to_string()),
    }
}
fn normalized_date(value: &str) -> Result<String, String> {
    let value = value.replace('-', "");
    bridge_tally_core::TallyDate::parse(value.clone()).map_err(|_| "invalid_date".to_string())?;
    Ok(value)
}

fn add_decimal(left: &str, right: &str) -> Result<String, String> {
    let left = bridge_tally_core::ExactDecimal::parse(left.to_string())
        .map_err(|_| "voucher_amount_invalid".to_string())?;
    let right = bridge_tally_core::ExactDecimal::parse(right.to_string())
        .map_err(|_| "voucher_amount_invalid".to_string())?;
    left.checked_add(&right)
        .map(|value| value.as_str().to_string())
        .map_err(|_| "voucher_amount_invalid".to_string())
}

fn redact_value(mut value: Value, redaction: Redaction) -> Value {
    match &mut value {
        Value::Array(values) => {
            for value in values {
                *value = redact_value(std::mem::take(value), redaction);
            }
        }
        Value::Object(values) => {
            if redaction == Redaction::DropNarration {
                values.remove("narration");
            }
            for (key, value) in values {
                if redaction == Redaction::MaskParties
                    && matches!(
                        key.as_str(),
                        "party" | "ledger" | "name" | "party_ledger_name"
                    )
                    && value.is_string()
                {
                    *value = Value::String(mask(value.as_str().unwrap_or_default()));
                } else {
                    *value = redact_value(std::mem::take(value), redaction);
                }
            }
        }
        _ => {}
    }
    value
}

fn mask(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= 4 {
        return "…".to_string();
    }
    format!(
        "{}{}…{}{}",
        chars[0],
        chars[1],
        chars[chars.len() - 2],
        chars[chars.len() - 1]
    )
}

/// New agent-only profile. The literal `$Date` filter is intentionally
/// separate from SVFROMDATE/SVTODATE: those variables do not restrict
/// collection membership on every supported Tally build.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TdlStringValue(String);

impl TdlStringValue {
    fn new(value: String) -> Result<Self, String> {
        let safe = |character: char| {
            character.is_ascii_alphanumeric()
                || matches!(character, ' ' | '_' | '-' | '.' | '/' | '&' | '(' | ')')
        };
        if value.is_empty()
            || value.chars().any(|character| {
                character.is_control()
                    || matches!(character, '\"' | '\'' | '$' | '#' | ':')
                    || !safe(character)
            })
        {
            return Err("tdl_string_value_invalid".to_string());
        }
        Ok(Self(value))
    }

    fn xml(&self) -> String {
        xml_escape(&self.0)
    }
}

fn render_agent_vouchers(
    company: &str,
    from: &str,
    to: &str,
    voucher_type: Option<String>,
    ledger: Option<String>,
    alter_id: Option<u64>,
) -> Result<String, String> {
    let type_filter = voucher_type
        .map(TdlStringValue::new)
        .transpose()?
        .map(|value| format!(" AND $VoucherTypeName = \"{}\"", value.xml()))
        .unwrap_or_default();
    let ledger_filter = ledger
        .map(TdlStringValue::new)
        .transpose()?
        .map(|value| format!(" AND $AllLedgerEntries.LedgerName = \"{}\"", value.xml()))
        .unwrap_or_default();
    let alter_filter = alter_id
        .map(|value| format!(" AND $AlterID > {value}"))
        .unwrap_or_default();
    Ok(format!("<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{type_filter}{ledger_filter}{alter_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" TYPE=\"Voucher\"><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ALLLEDGERENTRIES.LIST</FETCH><FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>", xml_escape(company)))
}

fn render_agent_changed_vouchers(company: &str, checkpoint: u64, snapshot: u64) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedVoucher\">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Vouchers\" TYPE=\"Voucher\"><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ALLLEDGERENTRIES.LIST</FETCH><FILTERS>BridgeAgentChangedVoucher</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn render_agent_changed_masters(company: &str, checkpoint: u64, snapshot: u64) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Masters</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedMaster\">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Ledgers\" TYPE=\"Ledger\"><FETCH>NAME,PARENT,ALTERID,MASTERID</FETCH><FILTERS>BridgeAgentChangedMaster</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION><COLLECTION NAME=\"Bridge Agent Changed Groups\" TYPE=\"Group\"><FETCH>NAME,PARENT,ALTERID,MASTERID</FETCH><FILTERS>BridgeAgentChangedMaster</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn render_agent_company_high_water(company: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Company High Water</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME=\"Bridge Agent Company High Water\" TYPE=\"Company\"><FETCH>GUID,ALTVCHID,ALTMSTID</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn validate_agent_envelope(xml: &str, expected_row: &str) -> Result<(), String> {
    let trimmed = xml.trim();
    if trimmed.is_empty()
        || !trimmed.starts_with("<ENVELOPE")
        || trimmed.contains("<LINEERROR")
        || trimmed.contains("<ERROR")
        || trimmed.contains("<RESPONSE")
        || (!trimmed.contains(&format!("<{expected_row}")) && !trimmed.contains("<COLLECTION"))
    {
        return Err("agent_read_protocol_invalid".to_string());
    }
    Ok(())
}

fn parse_company_high_water(xml: &str, expected_guid: &str) -> Result<Value, String> {
    validate_agent_envelope(xml, "COMPANY")?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut rows = Vec::<BTreeMap<String, String>>::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if name == "COMPANY" {
                    current = Some(BTreeMap::new());
                }
                tag = name;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let (Some(row), Ok(value)) = (current.as_mut(), text.decode()) {
                    row.insert(tag.clone(), value.into_owned());
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                if String::from_utf8_lossy(event.name().as_ref()).eq_ignore_ascii_case("COMPANY") {
                    if let Some(row) = current.take() {
                        rows.push(row);
                    }
                }
                tag.clear();
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
            _ => {}
        }
    }
    let row = rows
        .into_iter()
        .find(|row| {
            row.get("GUID")
                .is_some_and(|guid| guid.eq_ignore_ascii_case(expected_guid))
        })
        .ok_or_else(|| "company_high_water_identity_absent".to_string())?;
    Ok(json!({
        "altvchid": observed_checkpoint(row.get("ALTVCHID"), "voucher")?,
        "altmstid": observed_checkpoint(row.get("ALTMSTID"), "master")?,
    }))
}

fn checkpoint_arg(args: &Value, field: &str) -> Result<Option<u64>, String> {
    match args.get(field) {
        None => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| "checkpoint_invalid".to_string()),
        Some(Value::String(value)) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| "checkpoint_invalid".to_string()),
        Some(_) => Err("checkpoint_invalid".to_string()),
    }
}

fn checkpoint_advanceable(
    returned_max: Option<u64>,
    requested_checkpoint: u64,
    company_high_water: u64,
    truncated: bool,
) -> bool {
    !truncated && returned_max.unwrap_or(requested_checkpoint) >= company_high_water
}

fn observed_checkpoint(value: Option<&String>, axis: &str) -> Result<u64, String> {
    value
        .ok_or_else(|| format!("{axis}_checkpoint_not_observed"))?
        .parse::<u64>()
        .map_err(|_| format!("{axis}_checkpoint_invalid"))
}

fn parse_agent_changed_masters(xml: &str) -> Result<Vec<Value>, String> {
    validate_agent_envelope(xml, "LEDGER")?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut rows = Vec::new();
    let mut current: Option<(String, BTreeMap<String, String>)> = None;
    let mut tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let next = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if matches!(next.as_str(), "LEDGER" | "GROUP") {
                    current = Some((next.clone(), BTreeMap::new()));
                }
                tag = next;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some((_, fields)) = current.as_mut() {
                    if let Ok(value) = text.decode() {
                        fields.insert(tag.clone(), value.into_owned());
                    }
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if matches!(end.as_str(), "LEDGER" | "GROUP") {
                    if let Some((kind, fields)) = current.take() {
                        let alter_id = fields
                            .get("ALTERID")
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or_else(|| "change_row_alterid_invalid".to_string())?;
                        rows.push(json!({"kind": kind.to_ascii_lowercase(), "name": fields.get("NAME"), "parent": fields.get("PARENT"), "alter_id": alter_id, "master_id": fields.get("MASTERID")}));
                    }
                }
                tag.clear();
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
            _ => {}
        }
    }
    Ok(rows)
}

fn validate_change_row_alter_ids(rows: &[Value]) -> Result<(), String> {
    if rows
        .iter()
        .any(|row| row.get("alter_id").and_then(Value::as_u64).is_none())
    {
        Err("change_row_alterid_invalid".to_string())
    } else {
        Ok(())
    }
}

fn stable_change_page(
    mut rows: Vec<Value>,
    checkpoint: u64,
    snapshot: u64,
    max_rows: usize,
) -> Result<(Vec<Value>, bool, u64), String> {
    if checkpoint > snapshot {
        return Err("change_checkpoint_exceeds_snapshot".to_string());
    }
    validate_change_row_alter_ids(&rows)?;
    rows.retain(|row| {
        row["alter_id"]
            .as_u64()
            .is_some_and(|alter_id| alter_id > checkpoint && alter_id <= snapshot)
    });
    rows.sort_by_key(|row| row["alter_id"].as_u64());
    let truncated = rows.len() > max_rows;
    rows.truncate(max_rows);
    let next_cursor = if truncated {
        rows.last()
            .and_then(|row| row["alter_id"].as_u64())
            .ok_or_else(|| "change_page_cursor_invalid".to_string())?
    } else {
        snapshot
    };
    Ok((rows, truncated, next_cursor))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_agent_rows(xml: &str) -> Result<Vec<Value>, String> {
    // Tally's collection XML varies by release; use a deliberately conservative
    // extractor and never infer a missing field. Unknown collection shapes return
    // an empty result rather than invented records.
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut rows = Vec::new();
    validate_agent_envelope(xml, "VOUCHER")?;
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut current_tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if tag == "VOUCHER" {
                    current = Some(BTreeMap::new());
                }
                if tag == "ALLLEDGERENTRIES.LIST" && current.is_some() {
                    entry = Some(BTreeMap::new());
                }
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some(row) = entry.as_mut() {
                    if let Ok(text) = text.decode() {
                        row.insert(current_tag.clone(), text.into_owned());
                    }
                } else if let Some(row) = current.as_mut() {
                    if let Ok(text) = text.decode() {
                        row.insert(current_tag.clone(), text.into_owned());
                    }
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if end == "ALLLEDGERENTRIES.LIST" {
                    if let (Some(row), Some(entry_row)) = (current.as_mut(), entry.take()) {
                        if let (Some(ledger), Some(amount)) =
                            (entry_row.get("LEDGERNAME"), entry_row.get("AMOUNT"))
                        {
                            let polarity = entry_row
                                .get("ISDEEMEDPOSITIVE")
                                .map(String::as_str)
                                .unwrap_or("not_observed");
                            row.entry("AMOUNTS".to_string())
                                .and_modify(|amounts| {
                                    amounts.push('|');
                                    amounts.push_str(&format!("{ledger}|{amount}|{polarity}"));
                                })
                                .or_insert_with(|| format!("{ledger}|{amount}|{polarity}"));
                        }
                    }
                } else if end == "VOUCHER" {
                    if let Some(row) = current.take() {
                        let amounts = row.get("AMOUNTS").map(|items| items.split('|').collect::<Vec<_>>().chunks(3).filter(|chunk| chunk.len() == 3).map(|chunk| json!({"ledger":chunk[0],"amount":chunk[1],"is_deemed_positive":chunk[2]})).collect::<Vec<_>>()).unwrap_or_default();
                        rows.push(json!({"date": row.get("DATE"), "voucher_number": row.get("VOUCHERNUMBER"), "voucher_type": row.get("VOUCHERTYPENAME"), "party": row.get("PARTYLEDGERNAME"), "narration": row.get("NARRATION"), "guid": row.get("GUID"), "alter_id": row.get("ALTERID").and_then(|v| v.parse::<u64>().ok()), "master_id": row.get("MASTERID"), "amounts": amounts}));
                    }
                }
                current_tag.clear();
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
            _ => {}
        }
    }
    Ok(rows)
}

fn tool_definitions(import_enabled: bool) -> Value {
    let names = [
        "tally_status",
        "list_companies",
        "voucher_schema",
        "validate_masters",
        "build_import_xml",
        "verify_import",
        "outstandings",
        "ledger_masters",
        "ledger_movement",
        "vouchers",
        "changed_since",
        "read_evidence",
        "egress_log",
    ];
    Value::Array(
        names
            .into_iter()
            .filter(|name| import_enabled || !matches!(*name, "build_import_xml" | "verify_import"))
            .map(|name| {
                let (description, input_schema) = match name {
                    "voucher_schema" => (
                        "Return the fail-closed local voucher-file schema; no Tally request is sent.",
                        json!({"type":"object", "additionalProperties":false}),
                    ),
                    "validate_masters" => (
                        "Read the selected company's live ledger catalogue and report exact, near-miss, or missing names.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","ledgers"], "properties":{"company_guid":{"type":"string"},"ledgers":{"type":"array","items":{"type":"string"}}}}),
                    ),
                    "build_import_xml" => (
                        "Validate and write a local Tally voucher import file. This never dispatches import XML to Tally.",
                        agent_import::voucher_input_schema(),
                    ),
                    "verify_import" => (
                        "Read back a manually imported local batch and write Proof-of-Post files. This never dispatches import XML to Tally.",
                        json!({"type":"object", "additionalProperties":false, "required":["company_guid","batch_id"], "properties":{"company_guid":{"type":"string"},"batch_id":{"type":"string"}}}),
                    ),
                    "tally_status" => (
                        "Return loopback endpoint status and observed loaded-company identity tuples.",
                        json!({"type":"object","additionalProperties":false}),
                    ),
                    "list_companies" => (
                        "Return observed company tuples and identity ambiguity flags.",
                        json!({"type":"object","additionalProperties":false}),
                    ),
                    "outstandings" => (
                        "Return paired native receivable/payable totals, ageing, top-party ranking, and paginated open bills. `top` applies only to party ranking; use offset and limit for bills.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"direction":{"type":"string","enum":["receivable","payable","both"],"default":"both"},"as_of":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ageing_basis":{"type":"string","enum":["bill_date","due_date"],"default":"due_date"},"top":{"type":"integer","minimum":1,"default":25},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "ledger_masters" => (
                        "Return verified ledger masters; compliance includes paired party-master observations.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"group":{"type":"string"},"fields":{"type":"string","enum":["basic","compliance"],"default":"basic"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "ledger_movement" => (
                        "Return literal-window ledger opening, exact debit/credit movement, closing, and touched-voucher count.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ledger":{"type":"string"}}}),
                    ),
                    "vouchers" => (
                        "Return bounded, literal-window voucher evidence with curated metadata and redaction applied.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","from","to"],"properties":{"company_guid":{"type":"string","minLength":1},"from":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"to":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"voucher_type":{"type":"string"},"ledger":{"type":"string"},"offset":{"type":"integer","minimum":0,"default":0},"limit":{"type":"integer","minimum":1,"default":500}}}),
                    ),
                    "changed_since" => (
                        "Return snapshot-pinned AlterID voucher and master evidence. Continue a truncated scan with both returned AlterID cursors and snapshot values; deletion detection remains unsupported.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"voucher_alter_id":{"type":"integer","minimum":0,"default":0},"master_alter_id":{"type":"integer","minimum":0,"default":0},"voucher_snapshot_alter_id":{"type":"integer","minimum":0},"master_snapshot_alter_id":{"type":"integer","minimum":0}}}),
                    ),
                    "read_evidence" | "egress_log" => (
                        "Return bounded local metadata-only read evidence or egress receipts.",
                        json!({"type":"object","additionalProperties":false,"properties":{"limit":{"type":"integer","minimum":1,"default":20}}}),
                    ),
                    _ => (
                        "Bridge read-only Tally tool",
                        json!({"type":"object", "additionalProperties": false}),
                    ),
                };
                json!({"name": name, "description": description, "inputSchema": input_schema})
            })
            .collect(),
    )
}

pub async fn run_stdio() -> Result<(), String> {
    let server = Server::new(Settings::from_env()?);
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|_| "stdio_read_failed".to_string())?
    {
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => {
                stdout.write_all(b"{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await.map_err(|_| "stdio_write_failed".to_string())?;
                continue;
            }
        };
        let id = request.get("id").cloned();
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .ok_or_else(|| "initialize_protocol_version_required".to_string())
                .and_then(negotiate_protocol)
                .map(|protocol_version| json!({"protocolVersion": protocol_version, "capabilities": {"tools": {}}, "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}})),
            "notifications/initialized" => {
                if id.is_none() {
                    continue;
                }
                Ok(json!({}))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions(server.settings.import_enabled)})),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "tool_name_required".to_string())?;
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Ok(server.call_tool(name, arguments).await)
            }
            _ => Err("method_not_found".to_string()),
        };
        if let Some(id) = id {
            let response = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err(code) => {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":code}})
                }
            };
            stdout
                .write_all(format!("{response}\n").as_bytes())
                .await
                .map_err(|_| "stdio_write_failed".to_string())?;
            stdout
                .flush()
                .await
                .map_err(|_| "stdio_flush_failed".to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tally_protocol_simulator::{
        Fixture, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
    };

    fn company_collection_xml() -> String {
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY NAME=\"BRIDGE SYNTHETIC BOOK\"><GUID>00000000-0000-4000-8000-000000000001</GUID><COMPANYNUMBER>1</COMPANYNUMBER><BOOKSFROM>20260401</BOOKSFROM></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>".to_string()
    }

    fn settings(address: std::net::SocketAddr, data_dir: PathBuf) -> Settings {
        Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: address.port(),
            },
            data_dir,
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::MaskParties,
            import_enabled: false,
        }
    }

    #[test]
    fn agent_voucher_profile_uses_literal_filters_and_redaction_never_reveals_party() {
        let request = render_agent_vouchers(
            "BRIDGE SYNTHETIC BOOK",
            "20260401",
            "20260430",
            None,
            None,
            Some(99),
        )
        .expect("safe profile");
        assert!(request.contains("<FILTERS>BridgeAgentWindow</FILTERS>"));
        assert!(request.contains("$AlterID > 99"));
        assert_eq!(
            redact_value(
                json!({"party":"Acme Party", "narration":"private"}),
                Redaction::MaskParties
            )["party"],
            "Ac…ty"
        );
        assert!(tool_definitions(true)
            .as_array()
            .is_some_and(|tools| tools.len() == 13));
        let definitions = tool_definitions(true);
        let outstandings = definitions
            .as_array()
            .and_then(|tools| tools.iter().find(|tool| tool["name"] == "outstandings"))
            .expect("outstandings tool definition");
        assert_eq!(
            outstandings["inputSchema"]["required"],
            json!(["company_guid"])
        );
        assert_eq!(
            outstandings["inputSchema"]["properties"]["direction"]["enum"],
            json!(["receivable", "payable", "both"])
        );
        for attack in ["Party \" Name", "$$SysName:XML", "Party:Name"] {
            assert_eq!(
                TdlStringValue::new(attack.to_string()),
                Err("tdl_string_value_invalid".to_string())
            );
        }
        let recursively_redacted = redact_value(
            json!({"proof":{"name":"Acme Party"},"changed":{"ledger":"Cash Ledger"}}),
            Redaction::MaskParties,
        );
        assert_eq!(recursively_redacted["proof"]["name"], "Ac…ty");
        assert_eq!(recursively_redacted["changed"]["ledger"], "Ca…er");
        assert_eq!(negotiate_protocol("2024-11-05"), Ok("2024-11-05"));
        assert_eq!(
            negotiate_protocol("2023-01-01"),
            Err("unsupported_protocol_version".to_string())
        );
        assert!(
            parse_agent_rows("<ENVELOPE><BODY><RESPONSE>bad</RESPONSE></BODY></ENVELOPE>").is_err()
        );
    }

    #[test]
    fn redaction_setting_defaults_only_when_unset_and_rejects_unknown_values() {
        assert_eq!(Redaction::from_setting(None), Ok(Redaction::None));
        assert_eq!(Redaction::parse("none"), Ok(Redaction::None));
        assert_eq!(Redaction::parse("mask_parties"), Ok(Redaction::MaskParties));
        assert_eq!(
            Redaction::parse("drop_narration"),
            Ok(Redaction::DropNarration)
        );
        assert_eq!(
            Redaction::parse("mask_everything"),
            Err("redaction_setting_invalid".to_string())
        );
    }

    #[tokio::test]
    async fn pagination_rejects_present_invalid_values_in_helpers_and_row_tools() {
        assert_eq!(arg_usize(&json!({}), "limit", 20), Ok(20));
        for value in [json!(-1), json!(1.5), json!("10")] {
            assert_eq!(
                arg_usize(&json!({"limit": value}), "limit", 20),
                Err("pagination_invalid".to_string())
            );
        }

        let directory = tempfile::tempdir().expect("temporary agent directory");
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: 9,
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let response = server.call_tool("egress_log", json!({"limit": "10"})).await;
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "pagination_invalid"
        );
    }

    #[test]
    fn egress_log_tail_reads_only_the_last_bounded_chunks() {
        let directory = tempfile::tempdir().expect("temporary agent directory");
        let path = directory.path().join("agent-egress.jsonl");
        let mut body = String::new();
        for index in 0..12_000 {
            body.push_str(&format!(
                "{{\"index\":{index},\"padding\":\"xxxxxxxxxxxxxxxxxxxxxxxx\"}}\n"
            ));
        }
        assert!(body.len() > EGRESS_TAIL_CHUNK_BYTES);
        fs::write(&path, body).expect("large egress fixture");
        let tail = read_egress_tail(&path, 2).expect("bounded tail");
        assert_eq!(tail.len(), 2);
        assert!(tail[0].contains("11999"));
        assert!(tail[1].contains("11998"));
    }

    #[tokio::test]
    async fn imports_are_hidden_and_refused_without_explicit_live_evidence_opt_in() {
        let disabled_tools = tool_definitions(false);
        let names = disabled_tools
            .as_array()
            .expect("tool list")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(!names.contains(&"build_import_xml"));
        assert!(tool_definitions(true)
            .as_array()
            .expect("tool list")
            .iter()
            .any(|tool| tool["name"] == "build_import_xml"));
        let directory = tempfile::tempdir().expect("temporary agent directory");
        let server = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: 9,
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let response = server.call_tool("build_import_xml", json!({})).await;
        assert_eq!(
            response["structuredContent"]["result"]["error"]["code"],
            "import_unverified_on_live_tally"
        );
    }

    #[test]
    fn ledger_movement_excludes_cancelled_and_optional_vouchers_and_rejects_unknown_flags() {
        let voucher = |cancelled, optional| bridge_tally_protocol::TallyVoucher {
            id: None,
            date: Some("20260901".to_string()),
            voucher_type: Some("Payment".to_string()),
            voucher_number: None,
            party_ledger_name: None,
            cancelled,
            optional,
            ledger_entry_count: Some(0),
            ledger_entries: Vec::new(),
        };
        assert!(
            balance_affecting_vouchers(vec![voucher(Some(true), Some(false))])
                .expect("cancelled is excluded")
                .is_empty()
        );
        assert!(
            balance_affecting_vouchers(vec![voucher(Some(false), Some(true))])
                .expect("optional is excluded")
                .is_empty()
        );
        assert_eq!(
            balance_affecting_vouchers(vec![voucher(None, Some(false))]),
            Err("voucher_accounting_state_not_observed".to_string())
        );
    }

    #[test]
    fn paginated_egress_rows_returned_is_the_final_page_length() {
        assert_eq!(page_length_after_offset(11, 10, 500), 1);
        assert_eq!(page_length_after_offset(11, 11, 500), 0);
    }

    #[test]
    fn open_bill_paging_marks_remaining_rows_and_returns_a_cursor() {
        let (page, truncated, next_offset) = paginate_open_bills(vec!["a", "b", "c"], 0, 2);
        assert_eq!(page, vec!["a", "b"]);
        assert!(truncated);
        assert_eq!(next_offset, Some(2));

        let (last_page, last_truncated, last_next_offset) =
            paginate_open_bills(vec!["a", "b", "c"], 2, 2);
        assert_eq!(last_page, vec!["c"]);
        assert!(!last_truncated);
        assert_eq!(last_next_offset, None);
    }

    #[test]
    fn changed_since_checkpoints_round_trip_as_numbers_with_numeric_string_compatibility() {
        let observed =
            observed_checkpoint(Some(&"42".to_string()), "voucher").expect("numeric high water");
        let response = json!({"next_voucher_alter_id": observed});
        assert_eq!(
            checkpoint_arg(&response, "next_voucher_alter_id"),
            Ok(Some(42))
        );
        assert_eq!(
            checkpoint_arg(&json!({"voucher_alter_id":"42"}), "voucher_alter_id"),
            Ok(Some(42))
        );
        assert_eq!(
            checkpoint_arg(&json!({"voucher_alter_id":"bad"}), "voucher_alter_id"),
            Err("checkpoint_invalid".to_string())
        );
    }

    #[test]
    fn change_feed_snapshot_cursor_excludes_rows_inserted_after_first_page() {
        let row = |alter_id| json!({"alter_id": alter_id});
        let (first_page, first_truncated, first_cursor) =
            stable_change_page(vec![row(3), row(1), row(2)], 0, 3, 2).expect("first page");
        assert!(first_truncated);
        assert_eq!(
            first_page
                .iter()
                .map(|row| row["alter_id"].as_u64())
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2)]
        );
        assert_eq!(first_cursor, 2);

        // ALTERID 4 is inserted after page one. The original high-water 3 pins
        // page two, so ID 3 is still returned and the new row cannot shift it.
        let (second_page, second_truncated, second_cursor) =
            stable_change_page(vec![row(4), row(3)], first_cursor, 3, 2)
                .expect("snapshot-pinned second page");
        assert!(!second_truncated);
        assert_eq!(second_page, vec![row(3)]);
        assert_eq!(second_cursor, 3);

        let voucher_request = render_agent_changed_vouchers("BRIDGE SYNTHETIC BOOK", 2, 3);
        let master_request = render_agent_changed_masters("BRIDGE SYNTHETIC BOOK", 2, 3);
        for request in [voucher_request, master_request] {
            assert!(request.contains("$AlterID &gt; 2 AND $AlterID &lt;= 3"));
            assert!(request.contains("<SORT>Default: $AlterID</SORT>"));
        }
        let definitions = tool_definitions(false);
        let changed_since = definitions
            .as_array()
            .and_then(|tools| tools.iter().find(|tool| tool["name"] == "changed_since"))
            .expect("changed-since schema");
        assert!(changed_since["inputSchema"]["properties"]["offset"].is_null());
    }

    #[test]
    fn truncated_change_feed_never_advances_the_checkpoint() {
        assert!(!checkpoint_advanceable(Some(12), 0, 12, true));
        assert!(!checkpoint_advanceable(Some(11), 0, 12, false));
        assert!(checkpoint_advanceable(Some(12), 0, 12, false));
    }

    #[test]
    fn change_scan_rejects_any_missing_or_malformed_row_alter_id() {
        assert_eq!(
            validate_change_row_alter_ids(&[json!({"alter_id":null})]),
            Err("change_row_alterid_invalid".to_string())
        );
        assert_eq!(
            parse_agent_changed_masters("<ENVELOPE><BODY><DATA><COLLECTION><LEDGER><NAME>Bad</NAME><ALTERID>nope</ALTERID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"),
            Err("change_row_alterid_invalid".to_string())
        );
    }

    #[test]
    fn response_byte_cap_truncates_trailing_rows_and_refuses_a_single_oversized_row() {
        let input = json!({"truncated":false,"result":{"offset":0,"items":[{"name":"first"},{"name":"second"}]}});
        let cap = json!({"truncated":true,"result":{"offset":0,"items":[{"name":"first"}],"next_offset":1}}).to_string().len();
        let (bounded, truncated) = enforce_response_byte_cap(input, cap).expect("one row fits");
        assert!(truncated);
        assert_eq!(bounded["result"]["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(bounded["result"]["next_offset"], 1);
        assert_eq!(
            enforce_response_byte_cap(json!({"result":{"items":[{"name":"x".repeat(500)}]}}), 32),
            Err("agent_response_too_large".to_string())
        );
    }

    #[test]
    fn accounting_day_uses_tally_host_local_calendar_at_utc_midnight() {
        let east = chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("offset");
        let local = east
            .with_ymd_and_hms(2026, 9, 5, 0, 15, 0)
            .single()
            .expect("local timestamp");
        assert_eq!(format_tally_date(local), "20260905");
    }

    #[test]
    fn voucher_window_rejects_out_of_range_rows_and_requires_a_wider_empty_check() {
        assert!(!window_honoured(
            &[json!({"date":"20260915"})],
            "20260901",
            "20260902"
        ));
        assert!(window_honoured(&[], "20260901", "20260902"));
        assert_eq!(
            widened_window("20260901", "20260902"),
            Ok(("20260831".to_string(), "20260903".to_string()))
        );
    }

    #[tokio::test]
    async fn simulator_company_read_records_evidence_and_egress_while_down_endpoint_is_typed() {
        let plan = ScenarioPlan::new(Fixture::SyntheticXml(company_collection_xml()))
            .with_encoding(WireEncoding::Utf16Le)
            .with_framing(ResponseFraming::ContentLength);
        let simulator = SequenceSimulator::spawn(vec![plan]).expect("synthetic loopback server");
        let directory = tempfile::tempdir().expect("temporary agent directory");
        let server = Server::new(settings(
            simulator.address(),
            directory.path().to_path_buf(),
        ));
        let response = server.call_tool("list_companies", json!({})).await;
        assert_eq!(
            response["structuredContent"]["result"]["companies"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert!(response["structuredContent"]["evidence"]["request_sha256"].is_string());
        assert!(directory.path().join("agent-egress.jsonl").exists());
        assert_eq!(simulator.finish().expect("simulator result").len(), 1);

        let down = Server::new(Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: 9,
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        });
        let down_response = down.call_tool("tally_status", json!({})).await;
        assert!(down_response["structuredContent"]["result"]["error"]["code"].is_string());
        let receipts = fs::read_to_string(directory.path().join("agent-egress.jsonl"))
            .expect("egress receipts");
        assert_eq!(receipts.lines().count(), 2);
    }

    #[tokio::test]
    async fn tally_status_uses_the_runtime_probe_observation() {
        let simulator = SequenceSimulator::spawn(vec![
            ScenarioPlan::new(Fixture::ProductStatus(
                tally_protocol_simulator::ProductStatus::TallyPrime,
            )),
            ScenarioPlan::new(Fixture::SyntheticXml(company_collection_xml()))
                .with_encoding(WireEncoding::Utf16Le)
                .with_framing(ResponseFraming::ContentLength),
        ])
        .expect("synthetic loopback server");
        let directory = tempfile::tempdir().expect("temporary agent directory");
        let server = Server::new(settings(
            simulator.address(),
            directory.path().to_path_buf(),
        ));
        let response = server.call_tool("tally_status", json!({})).await;
        assert_eq!(
            response["structuredContent"]["result"]["product"],
            "TallyPrime"
        );
        assert_eq!(
            response["structuredContent"]["evidence"]["state"],
            "complete"
        );
        assert_eq!(simulator.finish().expect("simulator result").len(), 2);
    }
}
