//! The local stdio MCP surface. It uses Bridge's loopback-only Tally XML
//! transport. Import XML is only rendered to a local file: this server never
//! dispatches an import or another write request to Tally.

#[path = "agent_import.rs"]
mod agent_import;

use crate::tally::runtime::RuntimeReadEvidence;
use crate::tally::{
    ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, OutstandingsCurrencyAssertion,
    OutstandingsLoadResult, TallyConfig, TallyRuntime, UnallocatedParty, VerifiedCompanyIdentity,
};
use bridge_tally_protocol::xml_read_profiles::{
    ReadOnlyProfile, ValidatedCompanyName, ValidatedDateRange,
};
use bridge_tally_protocol::{parse_vouchers, TallyCompany, TallyLedger, TallyVoucher};
use bridge_tally_transport::{canonical_loopback_origin, TallyEndpointConfig};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const SERVER_NAME: &str = "bridge-tally";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_EVIDENCE_RECORDS: usize = 256;
const EGRESS_TAIL_CHUNK_BYTES: usize = 64 * 1024;
const MAX_EGRESS_TAIL_BYTES: usize = 256 * 1024;
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2024-11-05"];
const PARTY_NAME_MARKER: &str = "$bridge_agent_party_name";

/// A response-only marker for text that identifies a party. It serializes to
/// an internal object so [`redact_value`] can materialize or mask it without
/// relying on a field-name allowlist.
#[derive(Clone, Debug)]
pub(super) struct PartyName(String);

impl PartyName {
    pub(super) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl Serialize for PartyName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(PARTY_NAME_MARKER, &self.0)?;
        map.end()
    }
}

pub(super) fn party_name(value: impl Into<String>) -> PartyName {
    PartyName::new(value)
}

fn party_name_value(value: String) -> Value {
    serde_json::to_value(party_name(value)).unwrap_or_default()
}

fn mark_party_field(value: &mut Value, field: &str) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let Some(Value::String(name)) = object.remove(field) else {
        return;
    };
    object.insert(field.to_string(), party_name_value(name));
}

fn mark_compliance_party_names(mut compliance: Value) -> Value {
    for field in ["name_on_pan", "bank_account_holder_name", "bank_details"] {
        mark_party_field(&mut compliance, field);
    }
    compliance
}

fn mark_voucher_party_names(mut voucher: Value) -> Value {
    for field in ["party", "party_ledger_name"] {
        mark_party_field(&mut voucher, field);
    }
    if let Some(entries) = voucher.get_mut("amounts").and_then(Value::as_array_mut) {
        for entry in entries {
            mark_party_field(entry, "ledger");
        }
    }
    voucher
}

fn mark_changed_master_party_name(mut master: Value) -> Value {
    mark_party_field(&mut master, "name");
    master
}

fn open_bill_json(bill: &OpenBillRow) -> Value {
    let mut value = serde_json::to_value(bill).unwrap_or_default();
    mark_party_field(&mut value, "party");
    value
}

fn unallocated_party_json(party: &UnallocatedParty) -> Value {
    let mut value = serde_json::to_value(party).unwrap_or_default();
    mark_party_field(&mut value, "party");
    value
}

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
        let port = tally_port(env::var("BRIDGE_TALLY_PORT").ok())?;
        let max_rows = bounded_env("BRIDGE_AGENT_MAX_ROWS", 500, 1, 10_000)?;
        let max_bytes = bounded_env("BRIDGE_AGENT_MAX_BYTES", 200_000, 256, 5_000_000)?;
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

fn tally_port(value: Option<String>) -> Result<u16, String> {
    match value {
        None => Ok(9000),
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| "port_setting_invalid".to_string()),
    }
}

fn bounded_env(name: &str, default: usize, min: usize, max: usize) -> Result<usize, String> {
    match env::var(name) {
        Err(env::VarError::NotPresent) => Ok(default),
        Ok(value) => parse_bounded_limit(name, &value, min, max),
        Err(_) => Err(format!("limit_setting_invalid:{name}")),
    }
}

fn parse_bounded_limit(name: &str, value: &str, min: usize, max: usize) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| (*value >= min) && (*value <= max))
        .ok_or_else(|| format!("limit_setting_invalid:{name}"))
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

fn endpoint_origin(endpoint: &TallyEndpointConfig) -> Result<String, String> {
    canonical_loopback_origin(endpoint).map_err(|_| "endpoint_invalid".to_string())
}

#[derive(Clone, Serialize)]
struct Evidence {
    request_sha256: String,
    response_sha256: String,
    bytes: usize,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    read_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
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

struct EgressContext {
    tool: String,
    args_sha256: String,
    company_guid: Option<String>,
    fields_returned: Vec<String>,
}

struct ToolResponse {
    value: Value,
    egress: EgressContext,
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
            read_at: None,
            duration_ms: None,
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
                "endpoint": endpoint_origin(&self.settings.endpoint)?,
                "loaded_companies": probe.companies,
                "refusal_reason": Value::Null,
            }),
            Evidence {
                request_sha256: sha256_hex(b"runtime.probe"),
                response_sha256: sha256_json(&observed),
                bytes: observed.to_string().len(),
                state: "complete",
                read_at: None,
                duration_ms: None,
                reason_code: None,
            },
        ))
    }

    async fn companies(&self) -> Result<(Vec<TallyCompany>, Evidence), String> {
        let company_list = self
            .runtime
            .fetch_agent_companies(self.tally_config())
            .await
            .map_err(|_| "company_collection_invalid".to_string())?;
        let evidence = Evidence {
            request_sha256: sha256_hex(ReadOnlyProfile::CompanyListV2.render().as_bytes()),
            response_sha256: company_list.response_sha256,
            bytes: company_list.response_bytes,
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        Ok((company_list.companies, evidence))
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

    #[cfg(test)]
    async fn call_tool(&self, name: &str, args: Value) -> Value {
        self.call_tool_response(name, args).await.value
    }

    async fn call_tool_response(&self, name: &str, args: Value) -> ToolResponse {
        let args_sha256 = sha256_json(&args);
        let started = Utc::now();
        let result = self.tool_payload(name, &args).await;
        let (payload, mut evidence, company_guid, _rows, fields, truncated) = match result {
            Ok(outcome) => outcome,
            Err(code) => {
                let evidence = Evidence {
                    request_sha256: sha256_hex(format!("{name}:{args_sha256}").as_bytes()),
                    response_sha256: sha256_hex(code.as_bytes()),
                    bytes: 0,
                    state: "partial",
                    read_at: None,
                    duration_ms: None,
                    reason_code: Some(code.clone()),
                };
                (
                    json!({"error": {"code": code, "message": "Bridge withheld this read."}}),
                    evidence,
                    args.get("company_guid")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    0,
                    Vec::new(),
                    false,
                )
            }
        };
        evidence.read_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        evidence.duration_ms = Some((Utc::now() - started).num_milliseconds().max(0) as u128);
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
        let (response_value, _bytes_truncated, surviving_rows) =
            match enforce_response_byte_cap(response_value, self.settings.max_bytes) {
                Ok(value) => value,
                Err(code) => {
                    return ToolResponse {
                        value: response_too_large(name, &code),
                        egress: EgressContext {
                            tool: name.to_string(),
                            args_sha256,
                            company_guid,
                            fields_returned: fields,
                        },
                    };
                }
            };
        let mut mcp_response = json!({
            "content": [{"type":"text", "text": ""}],
            "structuredContent": response_value,
            "isError": response_value["result"].get("error").is_some(),
        });
        if let Err(code) = enforce_mcp_result_byte_cap(
            &mut mcp_response,
            self.settings.max_bytes,
            name,
            surviving_rows,
        ) {
            return ToolResponse {
                value: response_too_large(name, &code),
                egress: EgressContext {
                    tool: name.to_string(),
                    args_sha256,
                    company_guid,
                    fields_returned: fields,
                },
            };
        }
        let response_value = mcp_response["structuredContent"].clone();
        self.record_evidence(response_value["evidence"].clone());
        ToolResponse {
            value: mcp_response,
            egress: EgressContext {
                tool: name.to_string(),
                args_sha256,
                company_guid,
                fields_returned: fields,
            },
        }
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
            read_at: value
                .get("read_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            duration_ms: value
                .get("duration_ms")
                .and_then(Value::as_u64)
                .map(u128::from),
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

    fn append_framed_egress(
        &self,
        context: EgressContext,
        response: &Value,
        serialized_response: &str,
    ) -> Result<(), String> {
        let structured = response
            .get("result")
            .and_then(|result| result.get("structuredContent"));
        let rows_returned = structured.and_then(response_row_count).unwrap_or_default();
        let truncated = structured
            .and_then(|value| value.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let fields_returned = if structured.is_some() {
            context.fields_returned
        } else {
            Vec::new()
        };
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let receipt = EgressReceipt {
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            tool: &context.tool,
            args_sha256: context.args_sha256,
            company_guid: context.company_guid,
            rows_returned,
            fields_returned,
            bytes_returned: serialized_response.len(),
            enforced_bytes: self.settings.max_bytes,
            response_sha256: sha256_hex(serialized_response.as_bytes()),
            truncated,
            redaction_preset: self.settings.redaction.label(),
        };
        let line = serde_json::to_string(&receipt)
            .map_err(|_| "egress_record_write_failed".to_string())?;
        append_egress_line(&path, &line)
    }

    async fn tool_payload(&self, name: &str, args: &Value) -> Result<ToolOutcome, String> {
        validate_tool_arguments(name, args)?;
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
        let fields = optional_string(args, "fields")?.unwrap_or_else(|| "basic".to_string());
        let compliance = fields == "compliance";
        let (mut ledgers, ledger_evidence) = if compliance {
            let (records, evidence) = self
                .runtime
                .fetch_agent_party_ledger_masters_with_evidence(self.tally_config(), &identity)
                .await
                .map_err(|_| "party_ledger_master_read_failed".to_string())?;
            (
                records
                    .into_iter()
                    .map(|record| {
                        json!({
                            "name": party_name(record.ledger.name),
                            "parent": record.ledger.parent.returned_text(),
                            "opening_balance": record.ledger.opening_balance,
                            "party_gstin": record.ledger.party_gstin.returned_text(),
                            "compliance": mark_compliance_party_names(
                                serde_json::to_value(record.fields).unwrap_or_default(),
                            ),
                        })
                    })
                    .collect::<Vec<_>>(),
                evidence,
            )
        } else {
            let (records, evidence) = self
                .runtime
                .fetch_ledgers_with_evidence(self.tally_config(), &identity)
                .await
                .map_err(|_| "ledger_export_invalid".to_string())?;
            (
                records
                    .into_iter()
                    .map(|ledger| {
                        json!({
                            "name": party_name(ledger.name),
                            "parent": ledger.parent.returned_text(),
                            "opening_balance": ledger.opening_balance,
                        })
                    })
                    .collect::<Vec<_>>(),
                evidence,
            )
        };
        if let Some(group) = optional_string(args, "group")? {
            ledgers.retain(|ledger| ledger["parent"].as_str() == Some(group.as_str()));
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let total = ledgers.len();
        let page = ledgers
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|ledger| redact_value(ledger, self.settings.redaction))
            .collect::<Vec<_>>();
        let page_len = page_length_after_offset(total, offset, limit);
        let truncated = offset.saturating_add(page.len()) < total;
        let result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            combine_evidence(
                company_evidence,
                evidence_from_runtime_read(ledger_evidence),
            ),
            Some(guid.to_string()),
            page_len,
            ledger_fields_returned(compliance),
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
        let requested_ledger = optional_string(args, "ledger")?;
        let resolved_ledger = if let Some(requested) = requested_ledger {
            let ledgers = self
                .runtime
                .fetch_ledgers(self.tally_config(), &identity)
                .await
                .map_err(|_| "ledger_export_invalid".to_string())?;
            Some(resolve_ledger_name(
                ledgers.iter().map(|ledger| ledger.name.as_str()),
                &requested,
            )?)
        } else {
            None
        };
        let request = render_agent_vouchers(
            &company.name,
            &from,
            &to,
            optional_string(args, "voucher_type")?,
            resolved_ledger.clone(),
            None,
        )?;
        let (xml, mut evidence) = self.post_read(&identity, request).await?;
        let mut rows = parse_agent_rows(&xml)?;
        let mut filter_not_honoured = false;
        if let Some(ledger) = resolved_ledger.as_deref() {
            let (filtered_rows, dropped) = filter_voucher_rows_for_ledger(rows, ledger);
            rows = filtered_rows;
            filter_not_honoured = dropped;
        }
        if !window_honoured(&rows, &from, &to) {
            return Err("window_not_honoured".to_string());
        }
        let mut result_state = if filter_not_honoured {
            evidence.state = "partial";
            evidence.reason_code = Some("filter_not_honoured".to_string());
            "partial"
        } else {
            "complete"
        };
        let mut corroboration_reason = filter_not_honoured.then_some("filter_not_honoured");
        if rows.is_empty() {
            let (wider_from, wider_to) = widened_window(&from, &to)?;
            let wider_request = render_agent_vouchers(
                &company.name,
                &wider_from,
                &wider_to,
                optional_string(args, "voucher_type")?,
                resolved_ledger.clone(),
                None,
            )?;
            let (wider_xml, wider_evidence) = self.post_read(&identity, wider_request).await?;
            evidence = combine_evidence(evidence, wider_evidence);
            let wider_rows = parse_agent_rows(&wider_xml)?;
            let (wider_rows, wider_filter_not_honoured) =
                if let Some(ledger) = resolved_ledger.as_deref() {
                    filter_voucher_rows_for_ledger(wider_rows, ledger)
                } else {
                    (wider_rows, false)
                };
            if wider_filter_not_honoured {
                result_state = "partial";
                corroboration_reason = Some("filter_not_honoured");
                evidence.state = "partial";
                evidence.reason_code = Some("filter_not_honoured".to_string());
            }
            if !window_honoured(&wider_rows, &wider_from, &wider_to) {
                return Err("empty_uncorroborated".to_string());
            }
            let high_water = if wider_rows.is_empty() {
                let (high_water_xml, high_water_evidence) = self
                    .post_read(&identity, render_agent_company_high_water(&company.name))
                    .await?;
                evidence = combine_evidence(evidence, high_water_evidence);
                Some(
                    parse_company_high_water(&high_water_xml, guid)?["altvchid"]
                        .as_u64()
                        .ok_or_else(|| "voucher_checkpoint_invalid".to_string())?,
                )
            } else {
                None
            };
            let (partial, reason) =
                corroborate_empty_voucher_window(&wider_rows, &from, &to, high_water)?;
            if partial {
                result_state = "partial";
                evidence.state = "partial";
            }
            if corroboration_reason.is_none() {
                corroboration_reason = reason;
                evidence.reason_code = reason.map(str::to_string);
            }
        }
        if let Some(kind) = optional_string(args, "voucher_type")? {
            rows.retain(|row| row.get("voucher_type") == Some(&Value::String(kind.clone())));
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let total = rows.len();
        let items = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| redact_value(mark_voucher_party_names(row), self.settings.redaction))
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(items.len()) < total;
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"state": result_state, "reason": corroboration_reason, "items": items, "offset": offset, "total": total, "profile": "agent_vouchers_v1_filters"}}),
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
        let (snapshot, snapshot_evidence) = match (
            supplied_voucher_snapshot,
            supplied_master_snapshot,
        ) {
            (Some(voucher), Some(master)) => (
                json!({"altvchid": voucher, "altmstid": master}),
                Evidence {
                    request_sha256: sha256_hex(b"agent_change_snapshot_from_cursor"),
                    response_sha256: sha256_json(&json!({"altvchid": voucher, "altmstid": master})),
                    bytes: 0,
                    state: "complete",
                    read_at: None,
                    duration_ms: None,
                    reason_code: None,
                },
            ),
            (None, None) => {
                let (xml, evidence) = self
                    .post_read(&identity, render_agent_company_high_water(&company.name))
                    .await?;
                let voucher_snapshot = parse_company_high_water(&xml, guid)?["altvchid"]
                    .as_u64()
                    .ok_or_else(|| "voucher_checkpoint_invalid".to_string())?;
                let (master_xml, master_evidence) = self
                    .post_read(
                        &identity,
                        render_agent_master_domain_high_water(&company.name),
                    )
                    .await?;
                (
                    json!({"altvchid": voucher_snapshot, "altmstid": parse_master_domain_high_water(&master_xml)?}),
                    combine_evidence(evidence, master_evidence),
                )
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
        let all_rows = parse_agent_changed_rows(&xml)?;
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
            .map(|master| {
                redact_value(
                    mark_changed_master_party_name(master),
                    self.settings.redaction,
                )
            })
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
        let rows = rows
            .into_iter()
            .map(|row| redact_value(mark_voucher_party_names(row), self.settings.redaction))
            .collect::<Vec<_>>();
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
        let as_of = optional_string(args, "as_of")?
            .as_deref()
            .map(normalized_date)
            .transpose()?
            .unwrap_or_else(tally_host_today);
        let to =
            bridge_tally_core::TallyDate::parse(as_of).map_err(|_| "invalid_as_of".to_string())?;
        let ageing_basis =
            optional_string(args, "ageing_basis")?.unwrap_or_else(|| "due_date".to_string());
        let ageing_anchor = match ageing_basis.as_str() {
            "bill_date" => OutstandingsAgeingAnchor::BillDate,
            "due_date" => OutstandingsAgeingAnchor::DueDate,
            _ => return Err("invalid_ageing_basis".to_string()),
        };
        let (currency, currency_evidence) = self
            .runtime
            .detect_base_currency_with_evidence(self.tally_config(), &identity)
            .await
            .map_err(|_| "company_currency_probe_failed".to_string())?;
        let assertion = match (currency.currency_count, currency.is_inr) {
            (1, true) => OutstandingsCurrencyAssertion::Inr,
            (0, _) => return Err("company_currency_probe_failed".to_string()),
            (1, false) => return Err("company_base_currency_not_inr".to_string()),
            _ => return Err("company_base_currency_undetermined".to_string()),
        };
        let (load, outstandings_evidence) = self
            .runtime
            .fetch_outstandings_with_evidence(
                self.tally_config(),
                &identity,
                to,
                assertion,
                ageing_anchor,
            )
            .await
            .map_err(|_| "native_outstandings_read_failed".to_string())?;
        let top = arg_positive_usize(args, "top", 25)?.min(self.settings.max_rows);
        let bill_offset = arg_usize(args, "offset", 0)?;
        let bill_limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let mut result_evidence = combine_evidence(
            identity_evidence,
            combine_evidence(
                evidence_from_runtime_read(currency_evidence),
                evidence_from_runtime_read(outstandings_evidence),
            ),
        );
        let (result, bills_truncated) = match load {
            OutstandingsLoadResult::Complete {
                report: _,
                statement_open_bills,
                statement_unallocated_by_party,
                ..
            } => {
                let direction =
                    optional_string(args, "direction")?.unwrap_or_else(|| "both".to_string());
                if !matches!(direction.as_str(), "receivable" | "payable" | "both") {
                    return Err("invalid_direction".to_string());
                }
                let all_bills = statement_open_bills
                    .into_iter()
                    .filter(|bill| direction_matches(bill.kind, &direction))
                    .collect::<Vec<_>>();
                let selected_unallocated = statement_unallocated_by_party
                    .into_iter()
                    .filter(|party| direction_matches(party.direction, &direction))
                    .collect::<Vec<_>>();
                let parties = ranked_parties_from_exposure(&all_bills, &selected_unallocated, top)?
                    .into_iter()
                    .map(|party| redact_value(party, self.settings.redaction))
                    .collect::<Vec<_>>();
                let totals = outstanding_totals_from_open_bills(&all_bills)?;
                let ageing_buckets = ageing_buckets_from_open_bills(&all_bills)?;
                let (bills, bills_truncated, next_bill_offset) =
                    paginate_open_bills(all_bills, bill_offset, bill_limit);
                let bills = bills
                    .into_iter()
                    .map(|bill| redact_value(open_bill_json(&bill), self.settings.redaction))
                    .collect::<Vec<_>>();
                let unallocated_count = selected_unallocated.len();
                let unallocated_total = unallocated_total_from_parties(&selected_unallocated)?;
                let (unallocated, unallocated_truncated, next_unallocated_offset) =
                    paginate_open_bills(selected_unallocated, bill_offset, bill_limit);
                let unallocated = unallocated
                    .into_iter()
                    .map(|party| {
                        redact_value(unallocated_party_json(&party), self.settings.redaction)
                    })
                    .collect::<Vec<_>>();
                (
                    json!({"state":"complete", "totals":totals, "ageing_basis": if matches!(ageing_anchor, OutstandingsAgeingAnchor::BillDate) {"bill_date"} else {"due_date"}, "ageing_buckets": ageing_buckets, "top_parties": parties, "open_bills": bills, "offset": bill_offset, "limit": bill_limit, "next_offset": next_bill_offset, "unallocated":{"count": unallocated_count, "amount": unallocated_total, "parties": unallocated, "truncated": unallocated_truncated, "next_offset": next_unallocated_offset}}),
                    bills_truncated || unallocated_truncated,
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
        let (company, identity, mut evidence) = self.verified_company(guid).await?;
        let (ledgers, ledger_evidence) = self.read_movement_ledgers(&identity).await?;
        evidence = combine_evidence(evidence, ledger_evidence);
        let books_from = normalized_date(
            company
                .books_from
                .as_deref()
                .ok_or_else(|| "company_identity_incomplete".to_string())?,
        )?;
        ensure_movement_window_within_books(&from, &books_from)?;
        let pre_window = if books_from < from {
            let before_from = NaiveDate::parse_from_str(&from, "%Y%m%d")
                .map_err(|_| "invalid_date".to_string())?
                .checked_sub_signed(Duration::days(1))
                .ok_or_else(|| "invalid_date".to_string())?
                .format("%Y%m%d")
                .to_string();
            let (rows, read_evidence) = self
                .read_movement_vouchers(&identity, &company.name, books_from.clone(), before_from)
                .await?;
            evidence = combine_evidence(evidence, read_evidence);
            rows
        } else {
            Vec::new()
        };
        let (vouchers, read_evidence) = self
            .read_movement_vouchers(&identity, &company.name, from.clone(), to)
            .await?;
        evidence = combine_evidence(evidence, read_evidence);
        let pre_window = balance_affecting_vouchers(pre_window)?;
        let vouchers = balance_affecting_vouchers(vouchers)?;
        let selected = optional_string(args, "ledger")?
            .map(|name| {
                resolve_ledger_name(ledgers.iter().map(|ledger| ledger.name.as_str()), &name)
            })
            .transpose()?;
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
                    absent_movement_entry_policy(&entry.ledger_name, selected.as_deref())?;
                    continue;
                };
                if let Some(opening) = record.1.as_deref() {
                    record.1 = Some(add_decimal(opening, &entry.amount)?);
                }
            }
        }
        for voucher in &vouchers {
            let mut touched = std::collections::BTreeSet::new();
            for entry in &voucher.ledger_entries {
                let Some(record) = movement.get_mut(&entry.ledger_name) else {
                    absent_movement_entry_policy(&entry.ledger_name, selected.as_deref())?;
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
        let (rows, opening_unobserved) = movement
            .into_iter()
            .map(
                |(name, (parent, opening, debit, credit, _, vouchers_touching))| {
                    ledger_movement_row(
                        LedgerMovementRow {
                            name,
                            parent,
                            opening,
                            debit,
                            credit,
                            vouchers_touching,
                        },
                        books_from < from,
                        self.settings.redaction,
                    )
                },
            )
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .fold(
                (Vec::new(), false),
                |(mut rows, any_partial), (row, partial)| {
                    rows.push(row);
                    (rows, any_partial || partial)
                },
            );
        if opening_unobserved {
            evidence.state = "partial";
            evidence.reason_code = Some("opening_balance_not_observed".to_string());
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", self.settings.max_rows)?.min(self.settings.max_rows);
        let total = rows.len();
        let rows = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(rows.len()) < total;
        let next_offset = truncated.then_some(offset + rows.len());
        let (rows_returned, voucher_rows_observed) = ledger_movement_counts(&rows, &vouchers);
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"state": if opening_unobserved {"partial"} else {"complete"}, "partial_reason": opening_unobserved.then_some("opening_balance_not_observed"), "ledgers": rows, "offset": offset, "next_offset": next_offset, "voucher_rows_observed": voucher_rows_observed, "evidence_method": if books_from < from {"runtime_ledger_opening_plus_pre_window_entries_to_from"} else {"runtime_ledger_opening_at_books_from_plus_literal_window_entries"}}}),
            evidence,
            Some(guid.to_string()),
            rows_returned,
            vec![
                "ledger".into(),
                "opening".into(),
                "debit".into(),
                "credit".into(),
                "closing".into(),
            ],
            truncated,
        ))
    }

    async fn read_movement_ledgers(
        &self,
        identity: &VerifiedCompanyIdentity,
    ) -> Result<(Vec<TallyLedger>, Evidence), String> {
        let (ledgers, evidence) = self
            .runtime
            .fetch_ledgers_with_evidence(self.tally_config(), identity)
            .await
            .map_err(|_| "ledger_movement_read_failed".to_string())?;
        Ok((ledgers, evidence_from_runtime_read(evidence)))
    }

    async fn read_movement_vouchers(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        from: String,
        to: String,
    ) -> Result<(Vec<TallyVoucher>, Evidence), String> {
        let company = ValidatedCompanyName::new(company.to_string())
            .map_err(|_| "company_name_invalid".to_string())?;
        let range =
            ValidatedDateRange::new(from, to).map_err(|_| "invalid_date_range".to_string())?;
        let (xml, evidence) = self
            .post_read(
                identity,
                render_agent_vouchers(
                    company.as_str(),
                    range.from_yyyymmdd(),
                    range.to_yyyymmdd(),
                    None,
                    None,
                    None,
                )?,
            )
            .await?;
        let vouchers =
            parse_vouchers(&xml).map_err(|_| "ledger_movement_read_failed".to_string())?;
        if !movement_voucher_window_honoured(&vouchers, range.from_yyyymmdd(), range.to_yyyymmdd())
        {
            return Err("window_not_honoured".to_string());
        }
        Ok((vouchers, evidence))
    }

    fn read_evidence(&self, args: &Value) -> Result<ToolOutcome, String> {
        let take = arg_positive_usize(args, "limit", 20)?.min(MAX_EVIDENCE_RECORDS);
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
            read_at: None,
            duration_ms: None,
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
        let take = arg_positive_usize(args, "limit", 20)?.min(MAX_EVIDENCE_RECORDS);
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let lines = read_egress_tail(&path, take)?;
        let evidence = Evidence {
            request_sha256: sha256_hex(b"egress_log"),
            response_sha256: sha256_json(&lines),
            bytes: 0,
            state: "complete",
            read_at: None,
            duration_ms: None,
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

fn validate_tool_arguments(name: &str, args: &Value) -> Result<(), String> {
    let arguments = args
        .as_object()
        .ok_or_else(|| "argument_schema_invalid".to_string())?;
    let allowed: &[&str] = match name {
        "tally_status" | "list_companies" | "voucher_schema" => &[],
        "validate_masters" => &["company_guid", "ledgers"],
        "build_import_xml" => &["company_guid", "vouchers"],
        "verify_import" => &["company_guid", "batch_id"],
        "outstandings" => &[
            "company_guid",
            "direction",
            "as_of",
            "ageing_basis",
            "top",
            "offset",
            "limit",
        ],
        "ledger_masters" => &["company_guid", "group", "fields", "offset", "limit"],
        "ledger_movement" => &["company_guid", "from", "to", "ledger"],
        "vouchers" => &[
            "company_guid",
            "from",
            "to",
            "voucher_type",
            "ledger",
            "offset",
            "limit",
        ],
        "changed_since" => &[
            "company_guid",
            "voucher_alter_id",
            "master_alter_id",
            "voucher_snapshot_alter_id",
            "master_snapshot_alter_id",
        ],
        "read_evidence" | "egress_log" => &["limit"],
        _ => return Err("tool_not_found".to_string()),
    };
    if let Some(unknown) = arguments
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(format!("argument_unknown:{unknown}"));
    }
    Ok(())
}

fn append_egress_line(path: &Path, line: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|_| "egress_record_write_failed".to_string())?;
    file.lock_exclusive()
        .map_err(|_| "egress_record_write_failed".to_string())?;
    let write_result = (|| {
        file.write_all(line.as_bytes())
            .map_err(|_| "egress_record_write_failed".to_string())?;
        file.write_all(b"\n")
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
    })();
    let unlock_result = file
        .unlock()
        .map_err(|_| "egress_record_write_failed".to_string());
    write_result.and(unlock_result)
}

fn response_too_large(name: &str, code: &str) -> Value {
    json!({
        "content": [{"type":"text", "text": format!("{name}: read withheld\\n{code}")}],
        "structuredContent": {"error": {"code": code, "message": "Bridge response exceeds the configured byte cap."}},
        "isError": true,
    })
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

fn ensure_movement_window_within_books(from: &str, books_from: &str) -> Result<(), String> {
    (from >= books_from)
        .then_some(())
        .ok_or_else(|| "window_precedes_books_from".to_string())
}

fn ledger_fields_returned(compliance: bool) -> Vec<String> {
    if compliance {
        vec![
            "name".into(),
            "parent".into(),
            "opening_balance".into(),
            "party_gstin".into(),
            "compliance".into(),
        ]
    } else {
        vec!["name".into(), "parent".into(), "opening_balance".into()]
    }
}

fn ledger_lookup_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn resolve_ledger_name<'a>(
    ledger_names: impl Iterator<Item = &'a str>,
    requested: &str,
) -> Result<String, String> {
    let exact = ledger_names
        .filter(|name| ledger_lookup_key(name) == ledger_lookup_key(requested))
        .collect::<Vec<_>>();
    if let Some(name) = exact.iter().find(|name| **name == requested) {
        return Ok((*name).to_string());
    }
    match exact.as_slice() {
        [] => Err("ledger_not_found".to_string()),
        [name] => Ok((*name).to_string()),
        _ => Err("ledger_ambiguous".to_string()),
    }
}

fn filter_voucher_rows_for_ledger(rows: Vec<Value>, ledger: &str) -> (Vec<Value>, bool) {
    let key = ledger_lookup_key(ledger);
    let total = rows.len();
    let rows = rows
        .into_iter()
        .filter(|row| {
            row.get("amounts")
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry
                            .get("ledger")
                            .and_then(Value::as_str)
                            .is_some_and(|name| ledger_lookup_key(name) == key)
                    })
                })
        })
        .collect::<Vec<_>>();
    let dropped = rows.len() != total;
    (rows, dropped)
}

fn ledger_movement_counts<T>(rows: &[Value], vouchers: &[T]) -> (usize, usize) {
    (rows.len(), vouchers.len())
}

fn absent_movement_entry_policy(entry_ledger: &str, selected: Option<&str>) -> Result<(), String> {
    if selected.is_none_or(|ledger| ledger == entry_ledger) {
        Err("ledger_snapshot_drifted".to_string())
    } else {
        Ok(())
    }
}

struct LedgerMovementRow {
    name: String,
    parent: Option<String>,
    opening: Option<String>,
    debit: String,
    credit: String,
    vouchers_touching: usize,
}

fn ledger_movement_row(
    row: LedgerMovementRow,
    starts_after_books_from: bool,
    redaction: Redaction,
) -> Result<(Value, bool), String> {
    let LedgerMovementRow {
        name,
        parent,
        opening,
        debit,
        credit,
        vouchers_touching,
    } = row;
    let opening_unobserved = starts_after_books_from && opening.is_none();
    let closing = if opening_unobserved {
        None
    } else {
        opening
            .as_deref()
            .map(|opening| {
                let debited = add_decimal(opening, &debit)?;
                add_decimal(&debited, &credit)
            })
            .transpose()?
    };
    Ok((
        redact_value(
            json!({
                "ledger": party_name(name),
                "parent": parent,
                "opening": opening,
                "debit": debit,
                "credit": credit,
                "closing": closing,
                "vouchers_touching": vouchers_touching,
                "state": if opening_unobserved {"partial"} else {"complete"},
                "reason": opening_unobserved.then_some("opening_balance_not_observed"),
            }),
            redaction,
        ),
        opening_unobserved,
    ))
}

fn direction_matches(kind: ExposureDirection, requested: &str) -> bool {
    requested == "both" || kind.label().eq_ignore_ascii_case(requested)
}

fn outstanding_totals_from_open_bills(bills: &[OpenBillRow]) -> Result<Value, String> {
    let mut receivable = "0".to_string();
    let mut payable = "0".to_string();
    for bill in bills {
        match bill.kind {
            ExposureDirection::Receivable => {
                receivable = add_decimal(&receivable, bill.amount.as_str())?
            }
            ExposureDirection::Payable => payable = add_decimal(&payable, bill.amount.as_str())?,
        }
    }
    Ok(json!({"receivable": receivable, "payable": payable}))
}

fn ageing_buckets_from_open_bills(bills: &[OpenBillRow]) -> Result<Value, String> {
    let mut days_0_30 = "0".to_string();
    let mut days_31_60 = "0".to_string();
    let mut days_61_90 = "0".to_string();
    let mut days_90_plus = "0".to_string();
    for bill in bills {
        let bucket = match bill.age_days {
            None => &mut days_0_30,
            Some(age) => match age {
                0..=30 => &mut days_0_30,
                31..=60 => &mut days_31_60,
                61..=90 => &mut days_61_90,
                _ => &mut days_90_plus,
            },
        };
        *bucket = add_decimal(bucket, bill.amount.as_str())?;
    }
    Ok(json!({
        "days_0_30": days_0_30,
        "days_31_60": days_31_60,
        "days_61_90": days_61_90,
        "days_90_plus": days_90_plus,
    }))
}

fn unallocated_total_from_parties(parties: &[UnallocatedParty]) -> Result<String, String> {
    parties.iter().try_fold("0".to_string(), |total, party| {
        add_decimal(&total, party.amount.as_str())
    })
}

fn ranked_parties_from_exposure(
    bills: &[OpenBillRow],
    unallocated: &[UnallocatedParty],
    top: usize,
) -> Result<Vec<Value>, String> {
    let mut totals = BTreeMap::<String, (String, String, String, Option<u32>)>::new();
    for bill in bills {
        let entry = totals
            .entry(bill.party.clone())
            .or_insert_with(|| ("0".to_string(), "0".to_string(), "0".to_string(), None));
        match bill.kind {
            ExposureDirection::Receivable => entry.0 = add_decimal(&entry.0, bill.amount.as_str())?,
            ExposureDirection::Payable => entry.1 = add_decimal(&entry.1, bill.amount.as_str())?,
        }
        entry.3 = match (entry.3, bill.age_days) {
            (Some(current), Some(age)) => Some(current.max(age)),
            (current, None) => current,
            (None, Some(age)) => Some(age),
        };
    }
    for party in unallocated {
        let entry = totals
            .entry(party.party.clone())
            .or_insert_with(|| ("0".to_string(), "0".to_string(), "0".to_string(), None));
        entry.2 = add_decimal(&entry.2, party.amount.as_str())?;
    }
    let mut ranked = totals
        .into_iter()
        .map(
            |(party, (receivable, payable, unallocated, oldest_bill_age_days))| {
                let billed = add_decimal(&receivable, &payable)?;
                let outstanding_total = add_decimal(&billed, &unallocated)?;
                let magnitude = bridge_tally_core::ExactDecimal::parse(outstanding_total.clone())
                    .map_err(|_| "outstandings_amount_invalid".to_string())?;
                Ok((
                    party,
                    receivable,
                    payable,
                    billed,
                    unallocated,
                    outstanding_total,
                    oldest_bill_age_days,
                    magnitude,
                ))
            },
        )
        .collect::<Result<Vec<_>, String>>()?;
    ranked.sort_by(|left, right| {
        right
            .7
            .cmp_magnitude(&left.7)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(ranked
        .into_iter()
        .take(top)
        .map(
            |(
                party,
                receivable,
                payable,
                billed,
                unallocated,
                outstanding_total,
                oldest_bill_age_days,
                _,
            )| {
                json!({
                    "party": party_name(party),
                    "receivable": receivable,
                    "payable": payable,
                    "billed": billed,
                    "unallocated": unallocated,
                    "outstanding_total": outstanding_total,
                    "oldest_bill_age_days": oldest_bill_age_days,
                })
            },
        )
        .collect())
}

fn read_egress_tail(path: &Path, take: usize) -> Result<Vec<String>, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("egress_log_unreadable".to_string()),
    };
    file.lock_shared()
        .map_err(|_| "egress_log_unreadable".to_string())?;
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
    let lines = text.lines().rev().take(take).map(str::to_string).collect();
    file.unlock()
        .map_err(|_| "egress_log_unreadable".to_string())?;
    Ok(lines)
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

fn movement_voucher_window_honoured(rows: &[TallyVoucher], from: &str, to: &str) -> bool {
    rows.iter().all(|row| {
        row.date
            .as_deref()
            .and_then(|date| normalized_date(date).ok())
            .is_some_and(|date| date.as_str() >= from && date.as_str() <= to)
    })
}

fn corroborate_empty_voucher_window(
    widened_rows: &[Value],
    from: &str,
    to: &str,
    company_high_water: Option<u64>,
) -> Result<(bool, Option<&'static str>), String> {
    if widened_rows.iter().any(|row| row_in_window(row, from, to)) {
        return Err("window_contradicted".to_string());
    }
    if !widened_rows.is_empty() {
        return Ok((false, None));
    }
    match company_high_water {
        Some(0) => Ok((false, Some("company_has_no_vouchers"))),
        Some(_) => Ok((true, Some("empty_uncorroborated"))),
        None => Err("voucher_checkpoint_invalid".to_string()),
    }
}

fn row_in_window(row: &Value, from: &str, to: &str) -> bool {
    row.get("date")
        .and_then(Value::as_str)
        .is_some_and(|date| date >= from && date <= to)
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
) -> Result<(Value, bool, usize), String> {
    if response.to_string().len() <= max_bytes {
        let rows = response_row_count(&response).unwrap_or_default();
        return Ok((response, false, rows));
    }
    while response.to_string().len() > max_bytes {
        if !truncate_response_items(&mut response)? {
            return Err("agent_response_too_large".to_string());
        }
    }
    let rows = response_row_count(&response).unwrap_or_default();
    Ok((response, true, rows))
}

fn truncate_response_items(response: &mut Value) -> Result<bool, String> {
    let change_axis = ["vouchers", "masters"]
        .into_iter()
        .filter_map(|key| {
            let cursor_key = if key == "vouchers" {
                "next_voucher_alter_id"
            } else {
                "next_master_alter_id"
            };
            let fallback_key = if key == "vouchers" {
                "voucher_alter_id"
            } else {
                "master_alter_id"
            };
            (response["result"][cursor_key].is_u64() && response["result"][fallback_key].is_u64())
                .then(|| {
                    response["result"][key]
                        .as_array()
                        .map(|rows| (key, rows.len()))
                })
                .flatten()
        })
        .max_by_key(|(_, length)| *length)
        .filter(|(_, length)| *length > 0)
        .map(|(key, _)| key);
    if let Some(key) = change_axis {
        let cursor_key = if key == "vouchers" {
            "next_voucher_alter_id"
        } else {
            "next_master_alter_id"
        };
        let fallback_key = if key == "vouchers" {
            "voucher_alter_id"
        } else {
            "master_alter_id"
        };
        let rows = response["result"][key]
            .as_array_mut()
            .expect("non-empty change axis");
        rows.pop();
        let cursor = rows
            .iter()
            .filter_map(|row| row["alter_id"].as_u64())
            .max()
            .or_else(|| response["result"][fallback_key].as_u64())
            .ok_or_else(|| "change_page_cursor_invalid".to_string())?;
        response["truncated"] = Value::Bool(true);
        response["result"][cursor_key] = json!(cursor);
        response["result"]["checkpoint_advanceable"] = Value::Bool(false);
        return Ok(true);
    }
    let requested_offset = response["result"]["offset"].as_u64().unwrap_or(0);
    for key in ["items", "open_bills", "ledgers"] {
        if let Some(items) = response["result"][key]
            .as_array_mut()
            .filter(|items| !items.is_empty())
        {
            items.pop();
            let remaining = items.len();
            response["truncated"] = Value::Bool(true);
            response["result"]["next_offset"] = json!(requested_offset + remaining as u64);
            return Ok(true);
        }
    }
    if let Some(parties) = response["result"]["unallocated"]["parties"]
        .as_array_mut()
        .filter(|parties| !parties.is_empty())
    {
        parties.pop();
        let remaining = parties.len();
        response["truncated"] = Value::Bool(true);
        response["result"]["unallocated"]["truncated"] = Value::Bool(true);
        response["result"]["unallocated"]["next_offset"] =
            json!(requested_offset + remaining as u64);
        return Ok(true);
    }
    Ok(false)
}

fn response_row_count(response: &Value) -> Option<usize> {
    let result = &response["result"];
    if let Some(vouchers) = result["vouchers"].as_array() {
        return Some(vouchers.len() + result["masters"].as_array().map_or(0, Vec::len));
    }
    ["items", "ledgers", "records", "companies", "open_bills"]
        .into_iter()
        .find_map(|key| result[key].as_array().map(Vec::len))
        .or_else(|| result["unallocated"]["parties"].as_array().map(Vec::len))
}

fn set_mcp_content_summary(mcp_response: &mut Value, name: &str, fallback_rows: usize) {
    let structured = &mcp_response["structuredContent"];
    let company = structured["company"]["name"].as_str().unwrap_or("none");
    let rows = response_row_count(structured).unwrap_or(fallback_rows);
    let truncated = structured["truncated"].as_bool().unwrap_or(false);
    let evidence = structured["evidence"]["state"]
        .as_str()
        .unwrap_or("partial");
    mcp_response["content"][0]["text"] = Value::String(format!(
        "{name}: company={company}; rows={rows}; truncated={truncated}; evidence={evidence}"
    ));
}

fn enforce_mcp_result_byte_cap(
    mcp_response: &mut Value,
    max_bytes: usize,
    name: &str,
    fallback_rows: usize,
) -> Result<(), String> {
    set_mcp_content_summary(mcp_response, name, fallback_rows);
    while mcp_response.to_string().len() > max_bytes {
        let structured = mcp_response
            .get_mut("structuredContent")
            .ok_or_else(|| "agent_response_too_large".to_string())?;
        if !truncate_response_items(structured)? {
            return Err("agent_response_too_large".to_string());
        }
        set_mcp_content_summary(mcp_response, name, fallback_rows);
    }
    Ok(())
}

fn enforce_jsonrpc_response_byte_cap(response: &mut Value, max_bytes: usize) -> Result<(), String> {
    while response.to_string().len() > max_bytes {
        let result = response
            .get_mut("result")
            .ok_or_else(|| "agent_response_too_large".to_string())?;
        let structured = result
            .get_mut("structuredContent")
            .ok_or_else(|| "agent_response_too_large".to_string())?;
        if !truncate_response_items(structured)? {
            return Err("agent_response_too_large".to_string());
        }
        set_mcp_content_summary(result, "mcp", 0);
    }
    Ok(())
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
        read_at: None,
        duration_ms: None,
        reason_code: left.reason_code.or(right.reason_code),
    }
}

fn evidence_from_runtime_read(read: RuntimeReadEvidence) -> Evidence {
    Evidence {
        request_sha256: read.request_sha256,
        response_sha256: read.response_sha256,
        bytes: read.bytes,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
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
fn optional_string(args: &Value, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("argument_invalid:{key}")),
    }
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
fn arg_positive_usize(args: &Value, key: &str, default: usize) -> Result<usize, String> {
    let value = arg_usize(args, key, default)?;
    (value > 0)
        .then_some(value)
        .ok_or_else(|| "pagination_invalid".to_string())
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
            if values.len() == 1 {
                if let Some(Value::String(value)) = values.get(PARTY_NAME_MARKER) {
                    return Value::String(if redaction == Redaction::MaskParties {
                        mask(value)
                    } else {
                        value.clone()
                    });
                }
            }
            if redaction == Redaction::DropNarration {
                values.remove("narration");
            }
            for value in values.values_mut() {
                *value = redact_value(std::mem::take(value), redaction);
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
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedVoucher\">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Vouchers\" TYPE=\"Voucher\"><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ISCANCELLED,ISOPTIONAL,ALLLEDGERENTRIES.LIST</FETCH><FILTERS>BridgeAgentChangedVoucher</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn render_agent_changed_masters(company: &str, checkpoint: u64, snapshot: u64) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Masters</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedMaster\">$AlterID &gt; {checkpoint} AND $AlterID &lt;= {snapshot}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Ledgers\" TYPE=\"Ledger\"><FETCH>NAME,PARENT,ALTERID,GUID,MASTERID</FETCH><FILTERS>BridgeAgentChangedMaster</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION><COLLECTION NAME=\"Bridge Agent Changed Groups\" TYPE=\"Group\"><FETCH>NAME,PARENT,ALTERID,GUID,MASTERID</FETCH><FILTERS>BridgeAgentChangedMaster</FILTERS><SORT>Default: $AlterID</SORT></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn render_agent_master_domain_high_water(company: &str) -> String {
    format!("<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Master Domain High Water</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME=\"Bridge Agent Ledger High Water\" TYPE=\"Ledger\"><FETCH>ALTERID</FETCH></COLLECTION><COLLECTION NAME=\"Bridge Agent Group High Water\" TYPE=\"Group\"><FETCH>ALTERID</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>", xml_escape(company))
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
    reader.config_mut().trim_text(false);
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

fn parse_master_domain_high_water(xml: &str) -> Result<u64, String> {
    validate_agent_envelope(xml, "LEDGER")?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut in_domain_row = false;
    let mut tag = String::new();
    let mut high_water = 0_u64;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if matches!(tag.as_str(), "LEDGER" | "GROUP") {
                    in_domain_row = true;
                }
            }
            Ok(quick_xml::events::Event::Text(text)) if in_domain_row && tag == "ALTERID" => {
                let value = text
                    .decode()
                    .map_err(|_| "master_checkpoint_invalid".to_string())?;
                high_water = high_water.max(
                    value
                        .parse::<u64>()
                        .map_err(|_| "master_checkpoint_invalid".to_string())?,
                );
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if matches!(end.as_str(), "LEDGER" | "GROUP") {
                    in_domain_row = false;
                }
                tag.clear();
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
            _ => {}
        }
    }
    Ok(high_water)
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
    reader.config_mut().trim_text(false);
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
                    append_agent_text(fields, &tag, decoded_agent_text(text)?);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                if let Some((_, fields)) = current.as_mut() {
                    append_agent_text(fields, &tag, decoded_agent_reference(reference)?);
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
                        let name = fields
                            .get("NAME")
                            .filter(|name| !name.trim().is_empty())
                            .ok_or_else(|| "change_row_name_invalid".to_string())?;
                        let guid = fields.get("GUID").filter(|value| !value.trim().is_empty());
                        let master_id = fields
                            .get("MASTERID")
                            .filter(|value| !value.trim().is_empty());
                        if guid.is_none() && master_id.is_none() {
                            return Err("change_row_identity_invalid".to_string());
                        }
                        rows.push(json!({"kind": kind.to_ascii_lowercase(), "name": name, "parent": fields.get("PARENT"), "alter_id": alter_id, "guid": guid, "master_id": master_id}));
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
    parse_agent_rows_with_accounting_state(xml, false)
}

fn parse_agent_changed_rows(xml: &str) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, true)
}

fn parse_agent_rows_with_accounting_state(
    xml: &str,
    require_accounting_state: bool,
) -> Result<Vec<Value>, String> {
    // Tally's collection XML varies by release; use a deliberately conservative
    // extractor and never infer a missing field. Unknown collection shapes return
    // an empty result rather than invented records.
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    validate_agent_envelope(xml, "VOUCHER")?;
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut entries = Vec::<Value>::new();
    let mut current_tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if tag == "VOUCHER" {
                    current = Some(BTreeMap::new());
                    entries.clear();
                }
                if tag == "ALLLEDGERENTRIES.LIST" && current.is_some() {
                    entry = Some(BTreeMap::new());
                }
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some(row) = entry.as_mut() {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                } else if let Some(row) = current.as_mut() {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                if let Some(row) = entry.as_mut() {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                } else if let Some(row) = current.as_mut() {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if end == "ALLLEDGERENTRIES.LIST" {
                    if let (Some(_), Some(entry_row)) = (current.as_mut(), entry.take()) {
                        let ledger = entry_row
                            .get("LEDGERNAME")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let amount = entry_row
                            .get("AMOUNT")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let polarity = entry_row
                            .get("ISDEEMEDPOSITIVE")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        entries.push(json!({
                            "ledger": ledger,
                            "amount": amount,
                            "is_deemed_positive": polarity,
                        }));
                    }
                } else if end == "VOUCHER" {
                    if let Some(row) = current.take() {
                        if require_accounting_state
                            && row
                                .get("GUID")
                                .filter(|value| !value.trim().is_empty())
                                .is_none()
                            && row
                                .get("MASTERID")
                                .filter(|value| !value.trim().is_empty())
                                .is_none()
                        {
                            return Err("change_row_identity_invalid".to_string());
                        }
                        let amounts = std::mem::take(&mut entries);
                        let mut parsed = json!({"date": row.get("DATE"), "voucher_number": row.get("VOUCHERNUMBER"), "voucher_type": row.get("VOUCHERTYPENAME"), "party": row.get("PARTYLEDGERNAME"), "narration": row.get("NARRATION"), "guid": row.get("GUID"), "alter_id": row.get("ALTERID").and_then(|v| v.parse::<u64>().ok()), "master_id": row.get("MASTERID"), "amounts": amounts});
                        if require_accounting_state {
                            parsed["cancelled"] =
                                Value::Bool(required_tally_bool(row.get("ISCANCELLED"))?);
                            parsed["optional"] =
                                Value::Bool(required_tally_bool(row.get("ISOPTIONAL"))?);
                        }
                        rows.push(parsed);
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

fn append_agent_text(row: &mut BTreeMap<String, String>, tag: &str, value: String) {
    row.entry(tag.to_string()).or_default().push_str(&value);
}

fn decoded_agent_text(text: quick_xml::events::BytesText<'_>) -> Result<String, String> {
    let decoded = text
        .decode()
        .map_err(|_| "agent_read_protocol_invalid".to_string())?;
    quick_xml::escape::unescape(&decoded)
        .map(|value| value.into_owned())
        .map_err(|_| "agent_read_protocol_invalid".to_string())
}

fn decoded_agent_reference(reference: quick_xml::events::BytesRef<'_>) -> Result<String, String> {
    let reference = reference
        .decode()
        .map_err(|_| "agent_read_protocol_invalid".to_string())?;
    quick_xml::escape::unescape(&format!("&{reference};"))
        .map(|value| value.into_owned())
        .map_err(|_| "agent_read_protocol_invalid".to_string())
}

fn required_tally_bool(value: Option<&String>) -> Result<bool, String> {
    match value.map(String::as_str).map(str::trim) {
        Some("Yes") => Ok(true),
        Some("No") => Ok(false),
        _ => Err("voucher_accounting_state_not_observed".to_string()),
    }
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
    let mut stdout = tokio::io::stdout();
    serve_stdio(server, stdin, &mut stdout).await
}

async fn serve_stdio<R, W>(server: Server, reader: R, stdout: &mut W) -> Result<(), String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = reader.lines();
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
        let mut egress = None;
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
            "tools/call" => match params.get("name").and_then(Value::as_str) {
                Some(name) => {
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let tool_response = server.call_tool_response(name, arguments).await;
                egress = Some(tool_response.egress);
                Ok(tool_response.value)
                }
                None => Err("tool_name_required".to_string()),
            },
            _ => Err("method_not_found".to_string()),
        };
        if let Some(id) = id {
            let mut response = match result {
                Ok(result) => json!({"jsonrpc":"2.0","id":id.clone(),"result":result}),
                Err(code) => {
                    let error_code = if code == "method_not_found" {
                        -32601
                    } else {
                        -32602
                    };
                    json!({"jsonrpc":"2.0","id":id.clone(),"error":{"code":error_code,"message":code}})
                }
            };
            if method == "tools/call"
                && enforce_jsonrpc_response_byte_cap(&mut response, server.settings.max_bytes)
                    .is_err()
            {
                response = json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"agent_response_too_large"}});
            }
            let serialized_response = format!("{response}\n");
            if let Some(egress) = egress {
                server.append_framed_egress(egress, &response, &serialized_response)?;
            }
            stdout
                .write_all(serialized_response.as_bytes())
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
    use tokio::io::AsyncReadExt;

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
                json!({"party":party_name("Acme Party"), "narration":"private"}),
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
            json!({"proof":{"name":party_name("Acme Party")},"changed":{"ledger":party_name("Cash Ledger")}}),
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
    fn tool_arguments_reject_unknown_keys_before_tool_dispatch() {
        assert_eq!(
            validate_tool_arguments(
                "vouchers",
                &json!({"company_guid":"company", "from":"2026-09-01", "to":"2026-09-02", "ledgre":"Cash"}),
            ),
            Err("argument_unknown:ledgre".to_string())
        );
    }

    #[test]
    fn mask_parties_walks_every_tool_sample_response_without_leaking_party_names() {
        let known_parties = ["Customer One", "Supplier Two", "PAN Holder", "Entry Ledger"];
        let samples = BTreeMap::from([
            ("tally_status", json!({"product":"TallyPrime"})),
            (
                "list_companies",
                json!({"companies":[{"name":"Bridge Books"}]}),
            ),
            ("voucher_schema", json!({"schema":{"type":"object"}})),
            (
                "validate_masters",
                json!({"masters":[{"requested":party_name("Customer One"),"exact_live_spelling":party_name("Customer One"),"candidates":[party_name("Customer One")]}]}),
            ),
            (
                "build_import_xml",
                json!({"masters":[{"requested":party_name("Supplier Two"),"candidates":[party_name("Supplier Two")]}]}),
            ),
            ("verify_import", json!({"vouchers":[]})),
            (
                "ledger_masters",
                json!({"items":[{"name":party_name("Customer One"),"compliance":mark_compliance_party_names(json!({"name_on_pan":"PAN Holder","bank_account_holder_name":"PAN Holder","bank_details":"PAN Holder"}))}]}),
            ),
            (
                "vouchers",
                mark_voucher_party_names(
                    json!({"party":"Customer One","party_ledger_name":"Customer One","amounts":[{"ledger":"Entry Ledger"}]}),
                ),
            ),
            (
                "changed_since",
                json!({"masters":[mark_changed_master_party_name(json!({"name":"Supplier Two"}))]}),
            ),
            (
                "outstandings",
                json!({"top_parties":[{"party":party_name("Customer One")}],"open_bills":[{"party":party_name("Supplier Two")}],"unallocated":{"parties":[{"party":party_name("Supplier Two")}]}}),
            ),
            (
                "ledger_movement",
                json!({"ledgers":[{"ledger":party_name("Entry Ledger")}]}),
            ),
            ("read_evidence", json!({"records":[]})),
            ("egress_log", json!({"records":[]})),
        ]);
        assert_eq!(samples.len(), 13);
        for (tool, sample) in samples {
            let redacted = redact_value(sample, Redaction::MaskParties);
            assert_no_known_party_name(&redacted, &known_parties, tool);
        }
    }

    fn assert_no_known_party_name(value: &Value, known_parties: &[&str], tool: &str) {
        match value {
            Value::String(text) => assert!(
                known_parties.iter().all(|party| !text.contains(party)),
                "{tool} leaked a party name: {text}"
            ),
            Value::Array(values) => {
                for value in values {
                    assert_no_known_party_name(value, known_parties, tool);
                }
            }
            Value::Object(values) => {
                assert!(
                    !values.contains_key(PARTY_NAME_MARKER),
                    "{tool} returned an unmaterialized PartyName marker"
                );
                for value in values.values() {
                    assert_no_known_party_name(value, known_parties, tool);
                }
            }
            _ => {}
        }
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
    async fn malformed_tool_name_is_in_band_and_the_same_session_serves_the_next_request() {
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
        let (mut client, server_io) = tokio::io::duplex(4_096);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let serve = tokio::spawn(async move {
            serve_stdio(server, BufReader::new(server_read), &mut server_write).await
        });
        client
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\",\"params\":{}}\n",
            )
            .await
            .expect("write requests");
        client.shutdown().await.expect("close request stream");
        let mut output = String::new();
        client
            .read_to_string(&mut output)
            .await
            .expect("read responses");
        serve
            .await
            .expect("server task")
            .expect("session remains healthy");
        let responses = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("JSON-RPC response"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], -32602);
        assert_eq!(responses[0]["error"]["message"], "tool_name_required");
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], json!({}));
    }

    #[tokio::test]
    async fn egress_receipt_uses_the_final_jsonrpc_replacement_when_only_the_envelope_exceeds_cap()
    {
        let directory = tempfile::tempdir().expect("temporary agent directory");
        let base_settings = Settings {
            endpoint: TallyEndpointConfig {
                host: "127.0.0.1".to_string(),
                port: 9,
            },
            data_dir: directory.path().to_path_buf(),
            max_rows: 10,
            max_bytes: 200_000,
            redaction: Redaction::None,
            import_enabled: false,
        };
        let unframed = Server::new(base_settings.clone())
            .call_tool("voucher_schema", json!({}))
            .await;
        let server = Server::new(Settings {
            max_bytes: unframed.to_string().len(),
            ..base_settings
        });
        let (mut client, server_io) = tokio::io::duplex(16_384);
        let (server_read, mut server_write) = tokio::io::split(server_io);
        let serve = tokio::spawn(async move {
            serve_stdio(server, BufReader::new(server_read), &mut server_write).await
        });
        client
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"voucher_schema\",\"arguments\":{}}}\n",
            )
            .await
            .expect("write tool request");
        client.shutdown().await.expect("close request stream");
        let mut output = String::new();
        client
            .read_to_string(&mut output)
            .await
            .expect("read response");
        serve
            .await
            .expect("server task")
            .expect("session remains healthy");

        let response: Value = serde_json::from_str(output.trim_end()).expect("JSON-RPC response");
        assert_eq!(response["error"]["message"], "agent_response_too_large");
        let receipt_line = fs::read_to_string(directory.path().join("agent-egress.jsonl"))
            .expect("egress receipt");
        let receipt: Value = serde_json::from_str(receipt_line.trim_end()).expect("receipt JSON");
        assert_eq!(receipt["bytes_returned"], output.len());
        assert_eq!(receipt["response_sha256"], sha256_hex(output.as_bytes()));
        assert_eq!(receipt["rows_returned"], 0);
        assert_eq!(receipt["fields_returned"], json!([]));
        assert_eq!(receipt["truncated"], false);
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
        for key in ["limit", "top"] {
            let mut args = json!({});
            args[key] = json!(0);
            assert_eq!(
                arg_positive_usize(&args, key, 20),
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
    fn optional_filters_reject_non_string_values_before_widening_a_read() {
        assert_eq!(
            optional_string(&json!({"ledger": 42}), "ledger"),
            Err("argument_invalid:ledger".to_string())
        );
        assert_eq!(
            optional_string(&json!({"voucher_type": []}), "voucher_type"),
            Err("argument_invalid:voucher_type".to_string())
        );
        assert_eq!(optional_string(&json!({}), "ledger"), Ok(None));
    }

    #[test]
    fn configured_agent_limits_reject_malformed_or_out_of_range_values() {
        for value in ["not-a-number", "0", "10001"] {
            assert_eq!(
                parse_bounded_limit("BRIDGE_AGENT_MAX_ROWS", value, 1, 10_000),
                Err("limit_setting_invalid:BRIDGE_AGENT_MAX_ROWS".to_string())
            );
        }
        assert_eq!(
            parse_bounded_limit("BRIDGE_AGENT_MAX_BYTES", "256", 256, 5_000_000),
            Ok(256)
        );
    }

    #[test]
    fn concurrent_egress_appends_leave_two_parseable_json_lines() {
        let directory = tempfile::tempdir().expect("temporary egress directory");
        let path = directory.path().join("agent-egress.jsonl");
        let first = path.clone();
        let second = path.clone();
        let first = std::thread::spawn(move || append_egress_line(&first, r#"{"tool":"one"}"#));
        let second = std::thread::spawn(move || append_egress_line(&second, r#"{"tool":"two"}"#));
        first.join().expect("first writer").expect("first append");
        second
            .join()
            .expect("second writer")
            .expect("second append");
        let lines = fs::read_to_string(path)
            .expect("egress file")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("parseable receipt"))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().any(|line| line["tool"] == "one"));
        assert!(lines.iter().any(|line| line["tool"] == "two"));
    }

    #[test]
    fn egress_tail_waits_for_an_exclusive_append_lock() {
        let directory = tempfile::tempdir().expect("temporary egress directory");
        let path = directory.path().join("agent-egress.jsonl");
        let mut writer = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("writer");
        writer.lock_exclusive().expect("writer lock");
        writer
            .write_all(b"{\"tool\":\"complete\"}\n")
            .expect("complete row");
        let reader_path = path.clone();
        let reader = std::thread::spawn(move || read_egress_tail(&reader_path, 1));
        std::thread::sleep(std::time::Duration::from_millis(20));
        writer.unlock().expect("writer unlock");
        assert_eq!(
            reader.join().expect("reader").expect("tail"),
            vec!["{\"tool\":\"complete\"}"]
        );
    }

    #[test]
    fn voucher_parser_keeps_pipe_characters_inside_structured_ledger_names() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><DATE>20260901</DATE><ALLLEDGERENTRIES.LIST><LEDGERNAME>A|B</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        let rows = parse_agent_rows(xml).expect("voucher rows");
        assert_eq!(rows[0]["amounts"][0]["ledger"], "A|B");
        assert_eq!(rows[0]["amounts"][0]["amount"], "-10");
    }

    #[test]
    fn voucher_and_changed_parsers_reject_incomplete_ledger_entries() {
        for entry in [
            "<AMOUNT>-10</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
            "<LEDGERNAME>Expense</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE>",
            "<LEDGERNAME>Expense</LEDGERNAME><AMOUNT>-10</AMOUNT>",
        ] {
            let xml = format!(
                "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-guid</GUID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST>{entry}</ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>"
            );
            assert_eq!(
                parse_agent_rows(&xml),
                Err("agent_read_protocol_invalid".to_string())
            );
            assert_eq!(
                parse_agent_changed_rows(&xml),
                Err("agent_read_protocol_invalid".to_string())
            );
        }
    }

    #[test]
    fn voucher_parsers_decode_entities_in_vouchers_and_change_feeds() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-guid</GUID><VOUCHERNUMBER>R&amp;D</VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME>R&amp;D</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        let vouchers = parse_agent_rows(xml).expect("voucher rows");
        assert_eq!(vouchers[0]["voucher_number"], "R&D");
        assert_eq!(vouchers[0]["amounts"][0]["ledger"], "R&D");
        let changed = parse_agent_changed_rows(xml).expect("changed voucher rows");
        assert_eq!(changed[0]["voucher_number"], "R&D");
        assert_eq!(changed[0]["amounts"][0]["ledger"], "R&D");
    }

    #[test]
    fn voucher_parsers_preserve_entity_adjacent_whitespace() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-guid</GUID><VOUCHERNUMBER> before&amp;after </VOUCHERNUMBER><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><ALLLEDGERENTRIES.LIST><LEDGERNAME> Input&amp;CGST </LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        let vouchers = parse_agent_rows(xml).expect("voucher rows");
        assert_eq!(vouchers[0]["voucher_number"], " before&after ");
        assert_eq!(vouchers[0]["amounts"][0]["ledger"], " Input&CGST ");
        let changed = parse_agent_changed_rows(xml).expect("changed voucher rows");
        assert_eq!(changed[0]["voucher_number"], " before&after ");
        assert_eq!(changed[0]["amounts"][0]["ledger"], " Input&CGST ");
    }

    #[test]
    fn voucher_ledger_filter_drops_mixed_response_rows_that_do_not_match_live_spelling() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><VOUCHERNUMBER>keep</VOUCHERNUMBER><ALLLEDGERENTRIES.LIST><LEDGERNAME>R and D</LEDGERNAME><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE><AMOUNT>-10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER><VOUCHER><VOUCHERNUMBER>drop</VOUCHERNUMBER><ALLLEDGERENTRIES.LIST><LEDGERNAME>Sales</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>10</AMOUNT></ALLLEDGERENTRIES.LIST></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        let rows = parse_agent_rows(xml).expect("mixed voucher rows");
        let (rows, filter_not_honoured) = filter_voucher_rows_for_ledger(rows, "R-and_D");
        assert!(filter_not_honoured);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["voucher_number"], "keep");
    }

    #[test]
    fn changed_voucher_rows_require_typed_accounting_state() {
        let complete = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-guid</GUID><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>Yes</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        let rows = parse_agent_changed_rows(complete).expect("changed voucher rows");
        assert_eq!(rows[0]["cancelled"], false);
        assert_eq!(rows[0]["optional"], true);

        let absent = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><GUID>voucher-guid</GUID><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(
            parse_agent_changed_rows(absent),
            Err("voucher_accounting_state_not_observed".to_string())
        );
    }

    #[test]
    fn changed_voucher_rows_require_a_stable_identity() {
        let missing = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(
            parse_agent_changed_rows(missing),
            Err("change_row_identity_invalid".to_string())
        );

        let master_id = "<ENVELOPE><BODY><DATA><COLLECTION><VOUCHER><MASTERID>3</MASTERID><ALTERID>3</ALTERID><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL></VOUCHER></COLLECTION></DATA></BODY></ENVELOPE>";
        assert!(parse_agent_changed_rows(master_id).is_ok());
    }

    #[tokio::test]
    async fn stored_evidence_records_have_individual_timestamps_and_durations() {
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
        server.call_tool("voucher_schema", json!({})).await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        server.call_tool("voucher_schema", json!({})).await;
        let records = server.evidence.lock().expect("evidence records");
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record.duration_ms.is_some()));
        assert_ne!(records[0].read_at, records[1].read_at);
    }

    #[test]
    fn status_endpoint_uses_the_transport_canonical_loopback_origin() {
        for (host, expected) in [
            ("127.0.0.1", "http://127.0.0.1:9000"),
            ("::1", "http://[::1]:9000"),
        ] {
            assert_eq!(
                endpoint_origin(&TallyEndpointConfig {
                    host: host.to_string(),
                    port: 9000,
                }),
                Ok(expected.to_string())
            );
        }
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

    #[test]
    fn ledger_master_release_shapes_keep_gstin_compliance_only() {
        assert_eq!(
            ledger_fields_returned(false),
            vec!["name", "parent", "opening_balance"]
        );
        assert_eq!(
            ledger_fields_returned(true),
            vec![
                "name",
                "parent",
                "opening_balance",
                "party_gstin",
                "compliance"
            ]
        );
    }

    #[test]
    fn ledger_lookup_prefers_exact_live_spelling_and_rejects_ambiguous_matches() {
        let names = ["Bank Charges", "Sales-Ledger"];
        assert_eq!(
            resolve_ledger_name(names.into_iter(), "bank_charges"),
            Ok("Bank Charges".to_string())
        );
        assert_eq!(
            resolve_ledger_name(names.into_iter(), "missing ledger"),
            Err("ledger_not_found".to_string())
        );
        let ambiguous = ["A-B", "AB"];
        assert_eq!(
            resolve_ledger_name(ambiguous.into_iter(), "a_b"),
            Err("ledger_ambiguous".to_string())
        );
        assert_eq!(
            resolve_ledger_name(ambiguous.into_iter(), "AB"),
            Ok("AB".to_string())
        );
    }

    #[test]
    fn tally_port_defaults_only_when_the_environment_value_is_absent() {
        assert_eq!(tally_port(None), Ok(9000));
        assert_eq!(tally_port(Some("9001".to_string())), Ok(9001));
        assert_eq!(
            tally_port(Some("not-a-port".to_string())),
            Err("port_setting_invalid".to_string())
        );
    }

    #[test]
    fn ledger_movement_receipt_counts_three_ledgers_from_two_vouchers() {
        let rows = vec![
            json!({"ledger":"Bank"}),
            json!({"ledger":"Expense"}),
            json!({"ledger":"Income"}),
        ];
        let vouchers = [(), ()];
        assert_eq!(ledger_movement_counts(&rows, &vouchers), (3, 2));
    }

    #[test]
    fn ledger_movement_refuses_snapshot_drift_except_unselected_entries() {
        assert_eq!(
            absent_movement_entry_policy("Created Later", None),
            Err("ledger_snapshot_drifted".to_string())
        );
        assert_eq!(
            absent_movement_entry_policy("Cash", Some("Cash")),
            Err("ledger_snapshot_drifted".to_string())
        );
        assert_eq!(
            absent_movement_entry_policy("Created Later", Some("Cash")),
            Ok(())
        );
    }

    #[test]
    fn only_change_feed_responses_may_trim_cursor_rows() {
        for (tool, key) in [
            ("verify_import", "vouchers"),
            ("validate_masters", "masters"),
        ] {
            let response = json!({"result": {key: [{"value": "x".repeat(512)}]}});
            assert_eq!(
                enforce_response_byte_cap(response, 128),
                Err("agent_response_too_large".to_string()),
                "{tool} must not have rows trimmed without a change-feed cursor"
            );
        }
    }

    #[test]
    fn ledger_movement_evidence_changes_when_a_voucher_response_changes() {
        let evidence = |response| Evidence {
            request_sha256: "request".into(),
            response_sha256: sha256_hex(response),
            bytes: response.len(),
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        let baseline = combine_evidence(evidence(b"ledger"), evidence(b"voucher-one"));
        let changed = combine_evidence(evidence(b"ledger"), evidence(b"voucher-two"));
        assert_ne!(baseline.response_sha256, changed.response_sha256);
    }

    #[test]
    fn ledger_masters_evidence_changes_when_a_ledger_response_changes() {
        let identity = Evidence {
            request_sha256: "company-request".into(),
            response_sha256: "company-response".into(),
            bytes: 10,
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        let read = |response| {
            evidence_from_runtime_read(RuntimeReadEvidence::paired(
                "ledger-request",
                sha256_hex(response),
                response.len(),
            ))
        };
        let baseline = combine_evidence(identity.clone(), read(b"ledger-one"));
        let changed = combine_evidence(identity, read(b"ledger-two"));

        assert_ne!(baseline.response_sha256, changed.response_sha256);
        assert!(changed.bytes > 10, "the ledger read bytes are retained");
    }

    #[test]
    fn ledger_movement_rejects_a_window_before_the_observed_books_from() {
        assert_eq!(
            ensure_movement_window_within_books("20260331", "20260401"),
            Err("window_precedes_books_from".to_string())
        );
        assert_eq!(
            ensure_movement_window_within_books("20260401", "20260401"),
            Ok(())
        );
    }

    #[test]
    fn ledger_movement_opening_export_is_pinned_to_admitted_books_from() {
        use bridge_tally_protocol::native_outstandings::{
            render_native_ledger_export_request, NativeLedgerExportPeriod,
            NativeLedgerExportPeriodError,
        };
        use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;

        let books_from = bridge_tally_core::TallyDate::parse("20260401").expect("BooksFrom");
        let last_voucher = bridge_tally_core::TallyDate::parse("20260915").expect("last voucher");
        let matching = NativeLedgerExportPeriod::new(
            DateBoundaryProfile::EducationRestricted,
            books_from,
            last_voucher.clone(),
        )
        .expect("an admitted BOOKSFROM period");
        let request = render_native_ledger_export_request("Book", &matching);
        assert!(request.contains("<SVFROMDATE TYPE=\"Date\">20260401</SVFROMDATE>"));

        assert_eq!(
            NativeLedgerExportPeriod::new(
                DateBoundaryProfile::EducationRestricted,
                bridge_tally_core::TallyDate::parse("20260415").expect("mismatched date"),
                last_voucher,
            ),
            Err(NativeLedgerExportPeriodError::UnsupportedBoundary),
            "an unsupported opening boundary is rejected before Tally can substitute its display period",
        );
    }

    #[test]
    fn ledger_movement_marks_an_unobserved_post_books_opening_partial() {
        let (row, partial) = ledger_movement_row(
            LedgerMovementRow {
                name: "Customer A".to_string(),
                parent: Some("Sundry Debtors".to_string()),
                opening: None,
                debit: "10".to_string(),
                credit: "0".to_string(),
                vouchers_touching: 1,
            },
            true,
            Redaction::None,
        )
        .expect("partial movement row");
        assert!(partial);
        assert_eq!(row["opening"], Value::Null);
        assert_eq!(row["closing"], Value::Null);
        assert_eq!(row["state"], "partial");
        assert_eq!(row["reason"], "opening_balance_not_observed");
    }

    #[test]
    fn outstandings_top_ranking_uses_all_open_bills_not_the_report_cap() {
        let bills = (1..=12)
            .map(|index| OpenBillRow {
                party: format!("Party {index:02}"),
                reference: format!("REF-{index}"),
                bill_date: "20260901".to_string(),
                due_date: "20260901".to_string(),
                amount: bridge_tally_core::ExactDecimal::parse(index.to_string())
                    .expect("synthetic amount"),
                age_days: Some(index),
                kind: ExposureDirection::Receivable,
            })
            .collect::<Vec<_>>();
        let ranked = ranked_parties_from_exposure(&bills, &[], 12).expect("ranked parties");
        assert_eq!(ranked.len(), 12);
        let ranked = redact_value(json!({"parties": ranked}), Redaction::None);
        assert_eq!(ranked["parties"][0]["party"], "Party 12");
        assert_eq!(ranked["parties"][11]["party"], "Party 01");
    }

    #[test]
    fn outstandings_top_ranking_includes_wholly_unallocated_parties() {
        let bills = vec![OpenBillRow {
            party: "Billed Party".to_string(),
            reference: "B-1".to_string(),
            bill_date: "20260901".to_string(),
            due_date: "20260901".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse("40".to_string())
                .expect("synthetic amount"),
            age_days: Some(1),
            kind: ExposureDirection::Receivable,
        }];
        let unallocated = vec![UnallocatedParty {
            party: "Unallocated Party".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse("100".to_string())
                .expect("synthetic amount"),
            direction: ExposureDirection::Receivable,
        }];

        let ranked = redact_value(
            json!({"parties": ranked_parties_from_exposure(&bills, &unallocated, 2).expect("ranking")}),
            Redaction::None,
        );
        assert_eq!(ranked["parties"][0]["party"], "Unallocated Party");
        assert_eq!(ranked["parties"][0]["billed"], "0");
        assert_eq!(ranked["parties"][0]["unallocated"], "100");
        assert_eq!(ranked["parties"][0]["outstanding_total"], "100");
    }

    #[test]
    fn payable_outstandings_views_exclude_mixed_receivable_rows() {
        let bill = |party: &str, amount: &str, age_days, kind| OpenBillRow {
            party: party.to_string(),
            reference: format!("{party}-REF"),
            bill_date: "20260901".to_string(),
            due_date: "20260901".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse(amount.to_string())
                .expect("synthetic amount"),
            age_days: Some(age_days),
            kind,
        };
        let mixed_bills = vec![
            bill("Customer A", "100", 12, ExposureDirection::Receivable),
            bill("Supplier B", "200", 45, ExposureDirection::Payable),
        ];
        let payable_bills = mixed_bills
            .into_iter()
            .filter(|bill| direction_matches(bill.kind, "payable"))
            .collect::<Vec<_>>();
        assert_eq!(payable_bills.len(), 1);
        assert_eq!(payable_bills[0].party, "Supplier B");
        assert_eq!(
            outstanding_totals_from_open_bills(&payable_bills).expect("payable totals"),
            json!({"receivable":"0", "payable":"200"})
        );
        assert_eq!(
            ageing_buckets_from_open_bills(&payable_bills).expect("payable ageing"),
            json!({"days_0_30":"0", "days_31_60":"200", "days_61_90":"0", "days_90_plus":"0"})
        );
        let ranked = redact_value(
            json!({"parties": ranked_parties_from_exposure(&payable_bills, &[], 10).expect("payable ranking")}),
            Redaction::None,
        );
        assert_eq!(ranked["parties"][0]["party"], "Supplier B");
        let mixed_unallocated = vec![
            UnallocatedParty {
                party: "Customer A".to_string(),
                amount: bridge_tally_core::ExactDecimal::parse("30".to_string())
                    .expect("synthetic amount"),
                direction: ExposureDirection::Receivable,
            },
            UnallocatedParty {
                party: "Supplier B".to_string(),
                amount: bridge_tally_core::ExactDecimal::parse("40".to_string())
                    .expect("synthetic amount"),
                direction: ExposureDirection::Payable,
            },
        ];
        let payable_unallocated = mixed_unallocated
            .into_iter()
            .filter(|party| direction_matches(party.direction, "payable"))
            .collect::<Vec<_>>();
        assert_eq!(payable_unallocated.len(), 1);
        assert_eq!(payable_unallocated[0].party, "Supplier B");
        assert_eq!(
            unallocated_total_from_parties(&payable_unallocated).expect("payable unallocated"),
            "40"
        );
    }

    #[test]
    fn future_due_open_bills_remain_in_the_first_ageing_bucket() {
        let bill = OpenBillRow {
            party: "Customer".to_string(),
            reference: "FUTURE".to_string(),
            bill_date: "20260910".to_string(),
            due_date: "20260910".to_string(),
            amount: bridge_tally_core::ExactDecimal::parse("25".to_string()).expect("amount"),
            age_days: None,
            kind: ExposureDirection::Receivable,
        };
        assert_eq!(
            ageing_buckets_from_open_bills(&[bill]).expect("ageing buckets"),
            json!({"days_0_30":"25", "days_31_60":"0", "days_61_90":"0", "days_90_plus":"0"})
        );
    }

    #[test]
    fn unallocated_parties_are_bounded_without_dropping_the_aggregate() {
        let (page, truncated, next_offset) =
            paginate_open_bills(vec![json!({"party":"A"}), json!({"party":"B"})], 0, 1);
        assert_eq!(page.len(), 1);
        assert!(truncated);
        assert_eq!(next_offset, Some(1));

        let mut response = json!({
            "result": {
                "unallocated": {
                    "count": 2,
                    "amount": "30",
                    "parties": [json!({"party":"A"}), json!({"party":"B"})],
                    "truncated": false,
                    "next_offset": null,
                }
            }
        });
        assert!(truncate_response_items(&mut response).expect("trims unallocated parties"));
        assert_eq!(response["result"]["unallocated"]["count"], 2);
        assert_eq!(response["result"]["unallocated"]["amount"], "30");
        assert_eq!(
            response["result"]["unallocated"]["parties"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(response["result"]["unallocated"]["next_offset"], 1);
    }

    #[test]
    fn byte_trimming_keeps_each_cursor_relative_to_its_requested_offset() {
        for key in ["items", "open_bills", "ledgers"] {
            let mut response = json!({"result": {"offset": 500}});
            response["result"][key] = json!(vec![json!({"row": 1}); 101]);
            assert!(truncate_response_items(&mut response).expect("trims result page"));
            assert_eq!(response["result"][key].as_array().map(Vec::len), Some(100));
            assert_eq!(response["result"]["next_offset"], 600);
        }

        let mut response = json!({
            "result": {
                "offset": 500,
                "unallocated": {
                    "parties": vec![json!({"party": "Supplier"}); 101],
                    "next_offset": 601,
                }
            }
        });
        assert!(truncate_response_items(&mut response).expect("trims unallocated page"));
        assert_eq!(
            response["result"]["unallocated"]["parties"]
                .as_array()
                .map(Vec::len),
            Some(100)
        );
        assert_eq!(response["result"]["unallocated"]["next_offset"], 600);
    }

    #[test]
    fn byte_trimming_change_feed_rows_stops_checkpoint_advancement() {
        let mut response = json!({"result": {
            "voucher_alter_id": 10, "master_alter_id": 20,
            "next_voucher_alter_id": 12, "next_master_alter_id": 22,
            "checkpoint_advanceable": true,
            "vouchers": [{"alter_id": 11}, {"alter_id": 12}],
            "masters": [{"alter_id": 21}]
        }});
        assert!(truncate_response_items(&mut response).expect("trims vouchers"));
        assert_eq!(
            response["result"]["vouchers"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(response["result"]["next_voucher_alter_id"], 11);
        assert_eq!(response["result"]["checkpoint_advanceable"], false);
        assert!(response["truncated"].as_bool().expect("truncated"));
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
        assert_eq!(
            parse_agent_changed_masters("<ENVELOPE><BODY><DATA><COLLECTION><GROUP><ALTERID>3</ALTERID></GROUP></COLLECTION></DATA></BODY></ENVELOPE>"),
            Err("change_row_name_invalid".to_string())
        );
        assert_eq!(
            parse_agent_changed_masters("<ENVELOPE><BODY><DATA><COLLECTION><LEDGER><NAME>   </NAME><ALTERID>3</ALTERID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>"),
            Err("change_row_name_invalid".to_string())
        );
    }

    #[test]
    fn changed_master_parser_decodes_entity_fragments() {
        let rows = parse_agent_changed_masters("<ENVELOPE><BODY><DATA><COLLECTION><LEDGER><NAME>R&amp;D</NAME><PARENT>Income &amp; Expense</PARENT><ALTERID>3</ALTERID><GUID>ledger-guid</GUID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>").expect("changed master");
        assert_eq!(rows[0]["name"], "R&D");
        assert_eq!(rows[0]["parent"], "Income & Expense");
    }

    #[test]
    fn changed_master_rows_require_guid_or_master_id() {
        let missing_identity = "<ENVELOPE><BODY><DATA><COLLECTION><LEDGER><NAME>Cash</NAME><ALTERID>3</ALTERID></LEDGER></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(
            parse_agent_changed_masters(missing_identity),
            Err("change_row_identity_invalid".to_string())
        );
        assert!(render_agent_changed_masters("BRIDGE SYNTHETIC BOOK", 2, 3)
            .contains("<FETCH>NAME,PARENT,ALTERID,GUID,MASTERID</FETCH>"));
    }

    #[test]
    fn master_domain_high_water_ignores_unsupported_master_types() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION><LEDGER><ALTERID>4</ALTERID></LEDGER><GROUP><ALTERID>7</ALTERID></GROUP><STOCKITEM><ALTERID>99</ALTERID></STOCKITEM></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(parse_master_domain_high_water(xml), Ok(7));
        let request = render_agent_master_domain_high_water("Book");
        assert!(request.contains("TYPE=\"Ledger\""));
        assert!(request.contains("TYPE=\"Group\""));
        assert!(!request.contains("ALTMSTID"));
    }

    #[test]
    fn response_byte_cap_covers_the_serialized_jsonrpc_result_and_keeps_content_short() {
        let input = json!({"truncated":false,"result":{"offset":0,"items":[{"name":"first"},{"name":"second"}]}});
        let cap = json!({"truncated":true,"result":{"offset":0,"items":[{"name":"first"}],"next_offset":1}}).to_string().len();
        let (bounded, truncated, rows) =
            enforce_response_byte_cap(input, cap).expect("one row fits");
        assert!(truncated);
        assert_eq!(rows, 1);
        assert_eq!(bounded["result"]["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(bounded["result"]["next_offset"], 1);
        assert_eq!(
            enforce_response_byte_cap(json!({"result":{"items":[{"name":"x".repeat(500)}]}}), 32),
            Err("agent_response_too_large".to_string())
        );

        let one_item = json!({
            "content":[{"type":"text","text":""}],
            "structuredContent":{"company":{"name":"Book"},"evidence":{"state":"complete"},"truncated":true,"result":{"offset":0,"items":[{"name":"first"}],"next_offset":1}},
            "isError":false,
        });
        let mut one_item = one_item;
        enforce_mcp_result_byte_cap(&mut one_item, 10_000, "vouchers", 1).expect("summary");
        let cap = json!({"jsonrpc":"2.0","id":1,"result":one_item})
            .to_string()
            .len();
        let mut response = json!({
            "jsonrpc":"2.0",
            "id":1,
            "result":{
                "content":[{"type":"text","text":""}],
                "structuredContent":{"company":{"name":"Book"},"evidence":{"state":"complete"},"truncated":false,"result":{"offset":0,"items":[{"name":"first"},{"name":"second"}]}},
                "isError":false,
            }
        });
        enforce_mcp_result_byte_cap(&mut response["result"], 10_000, "vouchers", 2)
            .expect("summary");
        enforce_jsonrpc_response_byte_cap(&mut response, cap).expect("one page row fits");
        assert!(response.to_string().len() <= cap);
        assert_eq!(
            response["result"]["structuredContent"]["result"]["items"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        assert!(!response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("structuredContent"));
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

    #[test]
    fn ledger_movement_rejects_an_out_of_range_voucher_before_aggregation() {
        let voucher = bridge_tally_protocol::TallyVoucher {
            id: None,
            date: Some("20260915".to_string()),
            voucher_type: Some("Payment".to_string()),
            voucher_number: None,
            party_ledger_name: None,
            cancelled: Some(false),
            optional: Some(false),
            ledger_entry_count: Some(0),
            ledger_entries: Vec::new(),
        };
        assert!(!movement_voucher_window_honoured(
            &[voucher],
            "20260901",
            "20260902"
        ));
    }

    #[test]
    fn empty_voucher_window_corroboration_handles_all_three_control_branches() {
        assert_eq!(
            corroborate_empty_voucher_window(
                &[json!({"date":"20260831"}), json!({"date":"20260903"})],
                "20260901",
                "20260902",
                None,
            ),
            Ok((false, None))
        );
        assert_eq!(
            corroborate_empty_voucher_window(
                &[json!({"date":"20260901"})],
                "20260901",
                "20260902",
                None,
            ),
            Err("window_contradicted".to_string())
        );
        assert_eq!(
            corroborate_empty_voucher_window(&[], "20260901", "20260902", Some(0)),
            Ok((false, Some("company_has_no_vouchers")))
        );
        assert_eq!(
            corroborate_empty_voucher_window(&[], "20260901", "20260902", Some(1)),
            Ok((true, Some("empty_uncorroborated")))
        );
    }

    #[tokio::test]
    async fn simulator_company_read_records_evidence_while_down_endpoint_is_typed() {
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
