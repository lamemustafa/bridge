//! The local stdio MCP surface. It uses Bridge's loopback-only Tally XML
//! transport. Posting is opt-in and requires an independent native confirmation.

use crate::local_files::directory::{ensure_private_directory, DirectoryAdmissionError};
use crate::local_files::file as local_file;
use crate::local_files::paths::default_data_dir;

#[path = "agent_import.rs"]
mod agent_import;
pub use crate::tally::approved_import::run_confirmation;
pub(crate) use agent_import::desktop_journal_review as desktop_journal;

#[path = "agent_catalog.rs"]
mod catalog;
#[cfg(test)]
use catalog::tool_definitions;
use catalog::validate_tool_arguments;
#[path = "agent_protocol.rs"]
pub(super) mod agent_protocol;
#[path = "agent_receipt_fields.rs"]
mod agent_receipt_fields;
#[path = "agent_delivery.rs"]
mod delivery;
use agent_protocol::serve_stdio;
#[path = "agent_company.rs"]
mod company;
use company::*;
#[path = "agent_changes.rs"]
mod changes;
#[path = "agent_ledgers.rs"]
mod ledgers;
#[path = "agent_outstandings.rs"]
mod outstandings;
#[path = "agent_presence.rs"]
mod presence;
#[path = "agent_vouchers.rs"]
mod vouchers;
#[cfg(test)]
use outstandings::*;
#[path = "agent_movement.rs"]
mod movement;
#[path = "agent_trial_balance.rs"]
mod trial_balance;
#[cfg(test)]
use movement::parse_movement_vouchers;
#[path = "agent_responses.rs"]
mod responses;
use responses::*;
#[path = "agent_voucher_parse.rs"]
mod voucher_parse;
use voucher_parse::*;
#[path = "agent_read_validation.rs"]
mod read_validation;
use read_validation::validate_agent_envelope;
#[path = "agent_change_parse.rs"]
mod change_parse;
use change_parse::*;
#[path = "agent_read_profiles.rs"]
mod read_profiles;
use read_profiles::*;
#[path = "agent_movement_math.rs"]
mod movement_math;
use movement_math::*;
#[path = "agent_egress.rs"]
mod egress;
use egress::{append_egress_line, read_egress_tail};

use crate::tally::runtime::RuntimeReadEvidence;
use crate::tally::{
    ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, OutstandingsLoadResult, TallyConfig,
    TallyRuntime, UnallocatedParty, VerifiedCompanyIdentity,
};
use bridge_tally_protocol::xml_read_profiles::{
    ReadOnlyProfile, ValidatedCompanyName, ValidatedDateRange,
};
use bridge_tally_protocol::{TallyCompany, TallyLedger};
use bridge_tally_transport::{canonical_loopback_origin, TallyEndpointConfig};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
#[cfg(test)]
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

const SERVER_NAME: &str = "bridge-tally";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_EVIDENCE_RECORDS: usize = 256;
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
    writes_enabled: bool,
}

impl Settings {
    fn from_env() -> Result<Self, String> {
        let host = match env::var("BRIDGE_TALLY_HOST") {
            Ok(host) => host,
            Err(env::VarError::NotPresent) => "localhost".to_string(),
            Err(_) => return Err("host_setting_invalid".to_string()),
        };
        let port = tally_port(match env::var("BRIDGE_TALLY_PORT") {
            Ok(port) => Some(port),
            Err(env::VarError::NotPresent) => None,
            Err(_) => return Err("port_setting_invalid".to_string()),
        })?;
        let endpoint = TallyEndpointConfig { host, port };
        canonical_loopback_origin(&endpoint).map_err(|_| "host_setting_invalid".to_string())?;
        let redaction = Redaction::from_env()?;
        let max_rows = bounded_env("BRIDGE_AGENT_MAX_ROWS", 500, 1, 10_000)?;
        let max_bytes = bounded_env("BRIDGE_AGENT_MAX_BYTES", 200_000, 256, 5_000_000)?;
        // The transport remains the authoritative hard cap.  The agent cap only
        // narrows it and is applied to the response before parsing/returning.
        let data_dir = env::var_os("BRIDGE_AGENT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);
        // Published artifact paths must be representable in the JSON response.
        // Admit the resolved override or default before creating any local state.
        if data_dir.to_str().is_none() {
            return Err("agent_data_dir_encoding_invalid".to_string());
        }
        ensure_private_directory(&data_dir).map_err(|error| match error {
            DirectoryAdmissionError::Unavailable => "agent_data_dir_unavailable".to_string(),
            #[cfg(unix)]
            DirectoryAdmissionError::Permissions => "agent_data_dir_permissions_failed".to_string(),
        })?;
        let writes_enabled = enabled_setting("BRIDGE_AGENT_ENABLE_WRITES")?;
        Ok(Self {
            endpoint,
            data_dir,
            max_rows,
            max_bytes,
            redaction,
            import_enabled: enabled_setting("BRIDGE_AGENT_ENABLE_IMPORT")? || writes_enabled,
            writes_enabled,
        })
    }
}

fn enabled_setting(name: &str) -> Result<bool, String> {
    match env::var(name) {
        Err(env::VarError::NotPresent) => Ok(false),
        Ok(value) if matches!(value.as_str(), "1" | "true") => Ok(true),
        Ok(value) if matches!(value.as_str(), "0" | "false") => Ok(false),
        _ => Err("boolean_setting_invalid".into()),
    }
}

fn tally_port(value: Option<String>) -> Result<u16, String> {
    match value {
        None => Ok(9000),
        Some(value) => value
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| "port_setting_invalid".to_string()),
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

fn endpoint_origin(endpoint: &TallyEndpointConfig) -> Result<String, String> {
    canonical_loopback_origin(endpoint).map_err(|_| "endpoint_invalid".to_string())
}

#[derive(Clone, Debug, Serialize)]
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

fn receipt_tool_identity(tool: &str) -> (&str, Option<String>) {
    if catalog::registered_tool_definitions(true, true)
        .as_array()
        .is_some_and(|tools| tools.iter().any(|definition| definition["name"] == tool))
    {
        (tool, None)
    } else {
        ("unknown", Some(sha256_hex(tool.as_bytes())))
    }
}

struct EgressContext {
    evidence: Option<Evidence>,
    tool: String,
    args_sha256: String,
    company_guid: Option<String>,
}

struct ToolResponse {
    value: Value,
    egress: EgressContext,
    recovery_batch_id: Option<String>,
}

#[derive(Default)]
struct EvidenceStore {
    records: Vec<Evidence>,
    evicted: bool,
}

struct Server {
    settings: Settings,
    runtime: TallyRuntime,
    evidence: Arc<Mutex<EvidenceStore>>,
}

struct ToolOutcome {
    payload: Value,
    evidence: Evidence,
    company_guid: Option<String>,
    truncated: bool,
}

#[derive(Debug)]
struct ToolFailure {
    code: String,
    evidence: Option<Box<Evidence>>,
}

impl From<String> for ToolFailure {
    fn from(code: String) -> Self {
        Self {
            code,
            evidence: None,
        }
    }
}

impl ToolFailure {
    fn from_runtime(code: &str, error: anyhow::Error) -> Self {
        let code = if let Some(error) = error.chain().find_map(|cause| {
            cause.downcast_ref::<crate::tally::runtime::TrialBalanceReadError>()
        }) {
            error.safe_code()
        } else if error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<crate::tally::runtime::OpeningBoundaryObservationError>(),
                Some(crate::tally::runtime::OpeningBoundaryObservationError::Period(_))
            )
        }) || error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<crate::tally::connection::PartyLedgerMasterSourceValidationError>(),
                Some(
                    crate::tally::connection::PartyLedgerMasterSourceValidationError::MasterPeriod
                        | crate::tally::connection::PartyLedgerMasterSourceValidationError::BalancePeriod
                )
            )
        }) {
            "opening_period_not_honoured"
        } else if error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<crate::tally::runtime::OpeningBoundaryObservationError>(),
                Some(crate::tally::runtime::OpeningBoundaryObservationError::Changed)
            )
        }) {
            "opening_boundary_profile_changed"
        } else if error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<crate::tally::runtime::OpeningBoundaryObservationError>(),
                Some(crate::tally::runtime::OpeningBoundaryObservationError::Unobserved)
            )
        }) {
            "financial_read_profile_unqualified"
        } else {
            code
        };
        let evidence = error.chain().find_map(|cause| {
            cause
                .downcast_ref::<crate::tally::runtime::RuntimeReadFailure>()
                // Empty hashes are the runtime's absent-source sentinel. A real
                // zero-byte response still has commitments and must survive.
                .filter(|failure| {
                    !failure.evidence.request_sha256.is_empty()
                        || !failure.evidence.response_sha256.is_empty()
                })
                .map(|failure| Box::new(evidence_from_runtime_read(failure.evidence.clone())))
        });
        Self {
            code: code.to_string(),
            evidence,
        }
    }

    // The caller owns only observations completed before its failing child.
    // Combining prior first preserves read order without counting a read twice.
    fn with_prior_evidence(mut self, prior: Evidence) -> Self {
        self.evidence = Some(Box::new(match self.evidence {
            Some(current) => combine_evidence(prior, *current),
            None => prior,
        }));
        self
    }
}

impl Server {
    fn new(settings: Settings) -> Self {
        Self {
            settings,
            runtime: TallyRuntime::default(),
            evidence: Arc::new(Mutex::new(EvidenceStore::default())),
        }
    }

    fn tally_config(&self) -> TallyConfig {
        self.settings.endpoint.clone()
    }

    async fn post_read(
        &self,
        identity: &VerifiedCompanyIdentity,
        request: String,
    ) -> Result<(String, Evidence), ToolFailure> {
        let admitted = crate::tally::agent_read_request::AgentReadRequest::parse(request.clone())
            .map_err(|error| error.to_string())?;
        let request_sha256 = sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
            &request,
        ));
        let response = self
            .runtime
            .fetch_agent_read(self.tally_config(), identity, admitted)
            .await
            .map_err(|error| ToolFailure::from_runtime("agent_runtime_read_failed", error))?;
        let evidence = Evidence {
            request_sha256,
            response_sha256: response.encoded_sha256,
            // Both accepted source bodies from the stable paired read; auxiliary
            // health and identity guards are outside this source commitment.
            bytes: response.encoded_bytes.saturating_mul(2),
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        Ok((response.body, evidence))
    }

    #[cfg(test)]
    async fn call_tool(&self, name: &str, args: Value) -> Value {
        let response = self.call_tool_response(name, args).await;
        // In-process tests consume the inner result; production records evidence
        // only after final JSON-RPC framing and receipt-stage substitutions.
        if let Some(evidence) = response.egress.evidence {
            self.record_evidence(evidence);
        }
        response.value
    }

    async fn call_tool_response(&self, name: &str, args: Value) -> ToolResponse {
        let started = Utc::now();
        let result = self.tool_payload(name, &args).await;
        self.finish_tool_response(name, &args, started, result)
    }

    fn finish_tool_response(
        &self,
        name: &str,
        args: &Value,
        started: chrono::DateTime<Utc>,
        result: Result<ToolOutcome, ToolFailure>,
    ) -> ToolResponse {
        let args_sha256 = sha256_json(args);
        let ToolOutcome {
            payload,
            mut evidence,
            company_guid,
            truncated,
        } = match result {
            Ok(outcome) => outcome,
            Err(ToolFailure { code, evidence }) => {
                let mut evidence = evidence.map(|value| *value).unwrap_or_else(|| Evidence {
                    request_sha256: sha256_hex(format!("{name}:{args_sha256}").as_bytes()),
                    response_sha256: sha256_hex(code.as_bytes()),
                    bytes: 0,
                    state: "partial",
                    read_at: None,
                    duration_ms: None,
                    reason_code: Some(code.clone()),
                });
                evidence.state = "partial";
                evidence.reason_code = Some(code.clone());
                ToolOutcome {
                    payload: json!({"error": {"code": code, "message": "Bridge refused this operation."}}),
                    evidence,
                    company_guid: args
                        .get("company_guid")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    truncated: false,
                }
            }
        };
        // A successfully persisted batch must remain recoverable even when its
        // normal result cannot fit the MCP or JSON-RPC framing budget.
        let recovery_batch_id = payload["result"]["batch_id"].as_str().map(str::to_string);
        let batch_error_code = recovery_batch_id.as_ref().and_then(|_| {
            payload["result"]["error"]["code"]
                .as_str()
                .map(str::to_string)
        });
        evidence.read_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
        evidence.duration_ms = Some((Utc::now() - started).num_milliseconds().max(0) as u128);
        let response_value = redact_tool_response(
            name,
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
                    evidence.state = "partial";
                    evidence.reason_code = Some(code.clone());
                    return ToolResponse {
                        recovery_batch_id,
                        value: response_too_large(
                            name,
                            batch_error_code.as_deref().unwrap_or(&code),
                        ),
                        egress: EgressContext {
                            evidence: Some(evidence),
                            tool: name.to_string(),
                            args_sha256,
                            company_guid,
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
            evidence.state = "partial";
            evidence.reason_code = Some(code.clone());
            return ToolResponse {
                recovery_batch_id,
                value: response_too_large(name, batch_error_code.as_deref().unwrap_or(&code)),
                egress: EgressContext {
                    evidence: Some(evidence),
                    tool: name.to_string(),
                    args_sha256,
                    company_guid,
                },
            };
        }
        ToolResponse {
            recovery_batch_id,
            value: mcp_response,
            egress: EgressContext {
                evidence: Some(evidence),
                tool: name.to_string(),
                args_sha256,
                company_guid,
            },
        }
    }

    fn record_evidence(&self, evidence: Evidence) {
        let mut store = self.evidence.lock().expect("evidence mutex");
        store.records.push(evidence);
        if store.records.len() > MAX_EVIDENCE_RECORDS {
            store.records.remove(0);
            store.evicted = true;
        }
    }

    async fn tool_payload(&self, name: &str, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        if name == "changed_since" {
            return Err("changed_since_unqualified".to_string().into());
        }
        if name == "build_import_xml" {
            self.import_enabled()?;
        }
        if name == "post_import" && !self.settings.writes_enabled {
            return Err("import_posting_disabled".to_string().into());
        }
        validate_tool_arguments(name, args)?;
        match name {
            "tally_status" => {
                let (result, evidence) = self.status().await?;
                Ok(ToolOutcome {
                    payload: json!({"result": result}),
                    evidence,
                    company_guid: None,
                    truncated: false,
                })
            }
            "list_companies" => {
                let (companies, evidence) = self.companies().await?;
                let flagged = companies
                    .iter()
                    .map(|company| company_json(company, &companies))
                    .collect::<Vec<_>>();
                Ok(ToolOutcome {
                    payload: json!({"result": {"companies": flagged}}),
                    evidence,
                    company_guid: None,
                    truncated: false,
                })
            }
            "voucher_schema" => self.voucher_schema().map_err(Into::into),
            "validate_masters" => self.validate_masters(args).await,
            "post_import" => self.post_import(args).await,
            "build_import_xml" => {
                self.import_enabled()?;
                self.build_import_xml(args).await
            }
            "verify_import" => self.verify_import(args).await,
            "ledger_masters" => self.ledger_masters(args).await,
            "vouchers" => self.vouchers(args).await,
            "voucher_presence" => self.voucher_presence(args).await,
            "changed_since" => self.changed_since(args).await,
            "outstandings" => self.outstandings(args).await,
            "ledger_movement" => self.ledger_movement(args).await,
            "trial_balance" => self.trial_balance(args).await,
            "read_evidence" => self.read_evidence(args).map_err(Into::into),
            "egress_log" => self.egress_log(args).map_err(Into::into),
            _ => Err("tool_not_found".to_string().into()),
        }
    }

    fn import_enabled(&self) -> Result<(), String> {
        self.settings
            .import_enabled
            .then_some(())
            .ok_or_else(|| "import_unverified_on_live_tally".to_string())
    }

    fn read_evidence(&self, args: &Value) -> Result<ToolOutcome, String> {
        let take = arg_positive_usize(args, "limit", 20)?
            .min(self.settings.max_rows)
            .min(MAX_EVIDENCE_RECORDS);
        let store = self
            .evidence
            .lock()
            .map_err(|_| "evidence_store_unavailable".to_string())?;
        let truncated = store.evicted || store.records.len() > take;
        let values = store
            .records
            .iter()
            .rev()
            .take(take)
            .cloned()
            .collect::<Vec<_>>();
        let evidence = Evidence {
            request_sha256: sha256_hex(b"read_evidence"),
            response_sha256: sha256_json(&values),
            bytes: 0,
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        Ok(ToolOutcome {
            payload: json!({"result": {"records": values}}),
            evidence,
            company_guid: None,
            truncated,
        })
    }

    fn egress_log(&self, args: &Value) -> Result<ToolOutcome, String> {
        let take = arg_positive_usize(args, "limit", 20)?
            .min(self.settings.max_rows)
            .min(MAX_EVIDENCE_RECORDS);
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let tail = read_egress_tail(&path, take)?;
        let evidence = Evidence {
            request_sha256: sha256_hex(b"egress_log"),
            response_sha256: sha256_json(&tail.records),
            bytes: 0,
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        Ok(ToolOutcome {
            payload: json!({"result": {"records": tail.records}}),
            evidence,
            company_guid: None,
            truncated: tail.truncated,
        })
    }
}

/// Desktop-only projection of the same selected-ledger voucher operation used
/// by MCP. The caller has already performed the normal desktop company-tuple
/// verification; this function owns no extra Tally request beyond the shared
/// operation itself.
pub(crate) struct DesktopSelectedVoucherRequest {
    pub(crate) ledger: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
}

pub(crate) async fn desktop_selected_vouchers(
    runtime: &TallyRuntime,
    config: TallyConfig,
    identity: VerifiedCompanyIdentity,
    request: DesktopSelectedVoucherRequest,
) -> Result<Value, String> {
    let DesktopSelectedVoucherRequest {
        ledger,
        from,
        to,
        offset,
        limit,
    } = request;
    let server = Server::with_runtime(
        Settings {
            endpoint: config,
            data_dir: default_data_dir(),
            max_rows: 500,
            max_bytes: 200_000,
            // A local desktop view follows the existing desktop policy. MCP
            // continues to apply its independently configured response policy.
            redaction: Redaction::None,
            import_enabled: false,
            writes_enabled: false,
        },
        runtime.clone(),
    );
    let normalized_from = normalized_date(&from)?;
    let normalized_to = normalized_date(&to)?;
    let company_guid = identity.company_guid().to_string();
    let company = TallyCompany {
        name: identity.display_name().to_string(),
        guid: Some(company_guid.clone()),
        company_number: Some(identity.company_number().to_string()),
        books_from: Some(identity.books_from_yyyymmdd().to_string()),
    };
    let outcome = vouchers::selected_voucher_operation_for_verified(
        &server,
        &json!({
            "company_guid": company_guid,
            "from": normalized_from,
            "to": normalized_to,
            "ledger": ledger,
            "offset": offset,
            "limit": limit,
        }),
        vouchers::VoucherOperationScope {
            guid: company_guid,
            from: normalized_from,
            to: normalized_to,
            company,
            identity,
            initial_evidence: None,
        },
    )
    .await
    .map_err(|failure| failure.code)?;
    let response = json!({
        "company": outcome.payload["company"].clone(),
        "read_at": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        "evidence": outcome.evidence,
        "truncated": outcome.truncated,
        "result": outcome.payload["result"].clone(),
    });
    enforce_response_byte_cap(response, server.settings.max_bytes).map(
        |(mut bounded, byte_truncated, _)| {
            if byte_truncated {
                bounded["truncated"] = Value::Bool(true);
            }
            bounded
        },
    )
}

fn ledger_master_fields(fields: &str) -> Result<bool, String> {
    match fields {
        "basic" => Ok(false),
        "compliance" => Ok(true),
        _ => Err("argument_invalid:fields".to_string()),
    }
}

fn response_too_large(name: &str, code: &str) -> Value {
    let message = if code == "agent_response_too_large" {
        "Bridge response exceeds the configured byte cap."
    } else {
        "Bridge withheld this response; recover the retained batch before continuing."
    };
    json!({
        "content": [{"type":"text", "text": format!("{name}: read withheld\\n{code}")}],
        "structuredContent": {"error": {"code": code, "message": message}},
        "isError": true,
    })
}

fn page_is_truncated(total: usize, offset: usize, page_len: usize) -> bool {
    offset.saturating_add(page_len) < total
}

fn ensure_movement_window_within_books(from: &str, books_from: &str) -> Result<(), String> {
    (from >= books_from)
        .then_some(())
        .ok_or_else(|| "window_precedes_books_from".to_string())
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
    let requested_key = ledger_lookup_key(requested);
    let exact = ledger_names
        .filter(|name| ledger_lookup_key(name) == requested_key)
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

fn filter_voucher_rows_for_ledger(rows: Vec<Value>, ledger: &str) -> Vec<Value> {
    rows.into_iter()
        .filter(|row| {
            row.get("amounts")
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry
                            .get("ledger")
                            .and_then(Value::as_str)
                            .is_some_and(|name| name == ledger)
                    })
                })
        })
        .collect()
}

fn validate_then_filter_voucher_rows(
    rows: Vec<Value>,
    from: &str,
    to: &str,
    selected_ledger: Option<&str>,
) -> Result<Vec<Value>, String> {
    if !window_honoured(&rows, from, to) {
        return Err("window_not_honoured".to_string());
    }
    match selected_ledger {
        Some(ledger) => Ok(filter_voucher_rows_for_ledger(rows, ledger)),
        None => Ok(rows),
    }
}

fn window_honoured(rows: &[Value], from: &str, to: &str) -> bool {
    rows.iter().all(|row| {
        row.get("date")
            .and_then(Value::as_str)
            .is_some_and(|date| date >= from && date <= to)
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
        // Boundary-day rows only prove that this wider read returned *some*
        // data. They provide no independent cardinality for the nonempty
        // response, so they cannot promote the original empty window.
        return Ok((true, Some("nonempty_uncorroborated")));
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
        .or_else(|| SUPPORTED_PROTOCOL_VERSIONS.first().copied())
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

fn redact_tool_response(tool: &str, value: Value, redaction: Redaction) -> Value {
    // Only this local tool produces the server-defined input schema. Its
    // property names describe accepted input; they are not accounting data.
    let schema = (tool == "voucher_schema" && redaction == Redaction::DropNarration)
        .then(|| value.pointer("/result/schema").cloned())
        .flatten();
    let mut redacted = redact_value(value, redaction);
    if let Some(schema) = schema {
        redacted["result"]["schema"] = schema;
    }
    redacted
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub async fn run_stdio() -> Result<(), String> {
    let server = Server::new(Settings::from_env()?);
    let stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    serve_stdio(server, stdin, &mut stdout).await
}

fn attach_build_egress_failure(response: &mut Value) -> bool {
    let batch_id = response["result"]["structuredContent"]["result"]["batch_id"].clone();
    if !batch_id.is_string() {
        return false;
    }
    response["result"]["structuredContent"]["result"] = json!({
        "batch_id": batch_id,
        "egress_recorded": false,
        "error": {"code": "egress_record_write_failed", "message": "The local batch was built, but its egress receipt was not recorded."}
    });
    response["result"]["structuredContent"]["evidence"]["state"] = json!("partial");
    response["result"]["structuredContent"]["evidence"]["reason_code"] =
        json!("egress_record_write_failed");
    set_mcp_content_json(&mut response["result"]);
    response["result"]["isError"] = Value::Bool(true);
    true
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_financial_profile_tests.rs"]
mod financial_profile_tests;
