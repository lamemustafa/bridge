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
use bridge_tally_protocol::{parse_companies_from_collection, TallyCompany};
use bridge_tally_transport::{TallyEndpointConfig, TallyHttpTransport, TransportPolicy};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVER_NAME: &str = "bridge-tally";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_EVIDENCE_RECORDS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Redaction {
    None,
    MaskParties,
    DropNarration,
}

impl Redaction {
    fn from_env() -> Self {
        match env::var("BRIDGE_AGENT_REDACTION").as_deref() {
            Ok("mask_parties") => Self::MaskParties,
            Ok("drop_narration") => Self::DropNarration,
            _ => Self::None,
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
}

impl Settings {
    fn from_env() -> Result<Self, String> {
        let host = env::var("BRIDGE_TALLY_HOST").unwrap_or_else(|_| "localhost".to_string());
        let port = env::var("BRIDGE_TALLY_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(9000);
        let max_rows = bounded_env("BRIDGE_AGENT_MAX_ROWS", 500, 1, 10_000);
        // The transport remains the authoritative hard cap.  The agent cap only
        // narrows it and is applied to the response before parsing/returning.
        let max_bytes = bounded_env("BRIDGE_AGENT_MAX_BYTES", 200_000, 1, 32 * 1024 * 1024);
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
            redaction: Redaction::from_env(),
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

    fn transport(&self) -> Result<TallyHttpTransport, String> {
        let policy = TransportPolicy {
            xml_response_max_bytes: self.settings.max_bytes,
            ..TransportPolicy::default()
        };
        TallyHttpTransport::with_policy(self.settings.endpoint.clone(), policy)
            .map_err(|error| format!("endpoint_configuration_invalid:{error}"))
    }

    async fn post_read(&self, request: String) -> Result<(String, Evidence), String> {
        // This is the final in-process boundary: all Tally traffic emitted by
        // bridge-mcp must be a read. Keeping it here makes an accidental future
        // call site fail closed before bytes leave the loopback transport.
        if request
            .to_ascii_lowercase()
            .contains("<tallyrequest>import")
        {
            return Err("agent_write_dispatch_forbidden".to_string());
        }
        let transport = self.transport()?;
        let response = transport
            .post_xml_decoded(request)
            .await
            .map_err(safe_transport_error)?;
        let request_sha256 = response
            .request_body_sha256()
            .ok_or_else(|| "request_commitment_missing".to_string())?
            .to_string();
        let evidence = Evidence {
            request_sha256,
            response_sha256: response.encoded_sha256().to_string(),
            bytes: response.encoded_bytes(),
            state: "complete",
            reason_code: None,
        };
        Ok((response.into_text(), evidence))
    }

    async fn status(&self) -> Result<(Value, Evidence), String> {
        let transport = self.transport()?;
        let response = transport
            .get_status_decoded()
            .await
            .map_err(safe_transport_error)?;
        let bytes = response.encoded_bytes();
        let response_sha256 = response.encoded_sha256().to_string();
        let text = response.into_text();
        let (companies, company_evidence) = self.companies().await?;
        Ok((
            json!({
                "product": product_name(&text),
                "release": "not_observed",
                "education_mode": "not_observed",
                "endpoint": transport.canonical_origin().map_err(safe_transport_error)?,
                "loaded_companies": companies,
                "refusal_reason": Value::Null,
            }),
            combine_evidence(
                Evidence {
                    request_sha256: sha256_hex(b"GET /status"),
                    response_sha256,
                    bytes,
                    state: "complete",
                    reason_code: None,
                },
                company_evidence,
            ),
        ))
    }

    async fn companies(&self) -> Result<(Vec<TallyCompany>, Evidence), String> {
        let (xml, evidence) = self
            .post_read(ReadOnlyProfile::CompanyListV2.render())
            .await?;
        let companies = parse_companies_from_collection(&xml)
            .map_err(|_| "company_collection_invalid".to_string())?;
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
        let response_value = json!({
            "company": payload.get("company").cloned().unwrap_or_else(|| json!({"state":"not_company_scoped"})),
            "read_at": started.to_rfc3339_opts(SecondsFormat::Millis, true),
            "evidence": evidence,
            "truncated": truncated,
            "result": payload.get("result").cloned().unwrap_or(payload),
        });
        let response_sha256 = sha256_json(&response_value);
        self.record_evidence(response_value["evidence"].clone());
        self.append_egress(EgressReceipt {
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            tool: name,
            args_sha256,
            company_guid,
            rows_returned: rows,
            fields_returned: fields,
            bytes_returned: response_value.to_string().len(),
            response_sha256,
            truncated,
            redaction_preset: self.settings.redaction.label(),
        });
        let summary = if response_value.get("result").is_some() {
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

    fn append_egress(&self, receipt: EgressReceipt<'_>) {
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(
                file,
                "{}",
                serde_json::to_string(&receipt).unwrap_or_default()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
            }
        }
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
            "build_import_xml" => self.build_import_xml(args).await,
            "verify_import" => self.verify_import(args).await,
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
        let offset = arg_usize(args, "offset", 0);
        let limit = arg_usize(args, "limit", self.settings.max_rows).min(self.settings.max_rows);
        let total = ledgers.len();
        let page = ledgers
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|ledger| redact_value(ledger, self.settings.redaction))
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(page.len()) < total;
        let result = json!({"items": page, "offset": offset, "total": total, "fields": args.get("fields").and_then(Value::as_str).unwrap_or("basic"), "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            company_evidence,
            Some(guid.to_string()),
            total.min(limit),
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
        let (company, _identity, identity_evidence) = self.verified_company(guid).await?;
        let request = render_agent_vouchers(
            &company.name,
            &from,
            &to,
            arg_string(args, "voucher_type"),
            arg_string(args, "ledger"),
            None,
        );
        let (xml, evidence) = self.post_read(request).await?;
        let mut rows = parse_agent_rows(&xml);
        if let Some(kind) = arg_string(args, "voucher_type") {
            rows.retain(|row| row.get("voucher_type") == Some(&Value::String(kind.clone())));
        }
        let offset = arg_usize(args, "offset", 0);
        let limit = arg_usize(args, "limit", self.settings.max_rows).min(self.settings.max_rows);
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
        let alter_id = args
            .get("alter_id")
            .and_then(Value::as_u64)
            .ok_or_else(|| "alter_id_required".to_string())?;
        let (company, _identity, identity_evidence) = self.verified_company(guid).await?;
        let request = render_agent_vouchers(
            &company.name,
            "19000101",
            "29991231",
            None,
            None,
            Some(alter_id),
        );
        let (xml, evidence) = self.post_read(request).await?;
        let rows = parse_agent_rows(&xml)
            .into_iter()
            .filter(|row| {
                row.get("alter_id")
                    .and_then(Value::as_u64)
                    .is_some_and(|id| id > alter_id)
            })
            .take(self.settings.max_rows)
            .collect::<Vec<_>>();
        let (master_xml, master_evidence) = self
            .post_read(render_agent_changed_masters(&company.name, alter_id))
            .await?;
        let masters = parse_agent_changed_masters(&master_xml)
            .into_iter()
            .filter(|master| master["alter_id"].as_u64().is_some_and(|id| id > alter_id))
            .take(self.settings.max_rows)
            .collect::<Vec<_>>();
        let (extent_xml, extent_evidence) = self
            .post_read(render_agent_company_high_water(&company.name))
            .await?;
        let high_water = json!({
            "altvchid": xml_scalar(&extent_xml, "ALTVCHID"),
            "altmstid": xml_scalar(&extent_xml, "ALTMSTID"),
        });
        let returned_rows = rows.len() + masters.len();
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"vouchers": rows, "masters": masters, "deletion_detection": "unsupported_alterid_does_not_observe_deletions", "current_company_high_water": high_water}}),
            combine_evidence(
                combine_evidence(identity_evidence, evidence),
                combine_evidence(master_evidence, extent_evidence),
            ),
            Some(guid.to_string()),
            returned_rows,
            vec!["alter_id".into(), "name".into(), "kind".into()],
            false,
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
            .unwrap_or_else(|| Utc::now().format("%Y%m%d").to_string());
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
        let top = arg_usize(args, "top", 25).min(self.settings.max_rows);
        let result = match load {
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
                let bills = statement_open_bills
                    .into_iter()
                    .filter(|bill| {
                        direction == "both" || bill.kind.label().eq_ignore_ascii_case(direction)
                    })
                    .take(top)
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
                json!({"state":"complete", "totals":{"receivable": report.receivable_total, "payable": report.payable_total}, "ageing_basis": if matches!(ageing_anchor, OutstandingsAgeingAnchor::BillDate) {"bill_date"} else {"due_date"}, "ageing_buckets": report.ageing, "top_parties": parties, "open_bills": bills, "unallocated":{"count": statement_unallocated_by_party.len(), "amount": unallocated_total, "parties": statement_unallocated_by_party}})
            }
            OutstandingsLoadResult::Partial { reason, .. } => {
                json!({"state":"partial", "partial_reason": reason.reason_code})
            }
        };
        let rows = result["open_bills"].as_array().map_or(0, Vec::len);
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            identity_evidence,
            Some(guid.to_string()),
            rows.min(top),
            vec!["party".into(), "reference".into(), "amount".into()],
            false,
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
        let vouchers = self
            .runtime
            .fetch_vouchers(self.tally_config(), &identity, from, to)
            .await
            .map_err(|_| "ledger_movement_read_failed".to_string())?;
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
                    record.2 = add_decimal(&record.2, &magnitude)?;
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
                add_decimal(&debited, &format!("-{credit}"))
            }).transpose()?;
            Ok(json!({"ledger": name, "parent": parent, "opening": opening, "debit": debit, "credit": credit, "closing": closing, "vouchers_touching": vouchers_touching}))
        }).collect::<Result<Vec<_>, _>>()?;
        Ok((
            json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"ledgers": rows, "voucher_rows_observed": vouchers.len(), "evidence_method": "paired_runtime_ledger_openings_plus_literal_window_voucher_entries"}}),
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
        let take = arg_usize(args, "limit", 20).min(MAX_EVIDENCE_RECORDS);
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
        let take = arg_usize(args, "limit", 20).min(MAX_EVIDENCE_RECORDS);
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let lines = fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .rev()
            .take(take)
            .map(str::to_string)
            .collect::<Vec<_>>();
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

fn safe_transport_error(error: bridge_tally_transport::TallyTransportError) -> String {
    format!("transport_{}", error.safe_code())
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
    json!({"name": company.name, "guid": guid, "company_number": company.company_number, "books_from": company.books_from, "identity_state": if duplicate_guid {"ambiguous_duplicate_guid"} else {"verified_tuple"}})
}

fn product_name(status: &str) -> &'static str {
    if status.contains("TallyPrime") {
        "TallyPrime"
    } else if status.contains("Tally ERP 9") {
        "Tally ERP 9"
    } else {
        "Unknown"
    }
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
fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default)
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
    if redaction == Redaction::MaskParties {
        for key in ["party", "ledger"] {
            if let Some(text) = value.get(key).and_then(Value::as_str) {
                value[key] = Value::String(mask(text));
            }
        }
    }
    if redaction == Redaction::DropNarration {
        if let Some(object) = value.as_object_mut() {
            object.remove("narration");
        }
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
fn render_agent_vouchers(
    company: &str,
    from: &str,
    to: &str,
    voucher_type: Option<String>,
    ledger: Option<String>,
    alter_id: Option<u64>,
) -> String {
    let type_filter = voucher_type
        .map(|value| format!(" AND $VoucherTypeName = \"{}\"", xml_escape(&value)))
        .unwrap_or_default();
    let ledger_filter = ledger
        .map(|value| format!(" AND $PartyLedgerName = \"{}\"", xml_escape(&value)))
        .unwrap_or_default();
    let alter_filter = alter_id
        .map(|value| format!(" AND $AlterID > {value}"))
        .unwrap_or_default();
    format!("<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Vouchers</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"{type_filter}{ledger_filter}{alter_filter}</SYSTEM><COLLECTION NAME=\"Bridge Agent Vouchers\" TYPE=\"Voucher\"><FETCH>DATE,VOUCHERNUMBER,VOUCHERTYPENAME,PARTYLEDGERNAME,NARRATION,GUID,ALTERID,MASTERID,ALLLEDGERENTRIES.LIST</FETCH><FILTERS>BridgeAgentWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>", xml_escape(company))
}

fn render_agent_changed_masters(company: &str, alter_id: u64) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Changed Masters</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeAgentChangedMaster\">$AlterID &gt; {alter_id}</SYSTEM><COLLECTION NAME=\"Bridge Agent Changed Ledgers\" TYPE=\"Ledger\"><FETCH>NAME,PARENT,ALTERID,MASTERID</FETCH><FILTERS>BridgeAgentChangedMaster</FILTERS></COLLECTION><COLLECTION NAME=\"Bridge Agent Changed Groups\" TYPE=\"Group\"><FETCH>NAME,PARENT,ALTERID,MASTERID</FETCH><FILTERS>BridgeAgentChangedMaster</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn render_agent_company_high_water(company: &str) -> String {
    format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Agent Company High Water</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME=\"Bridge Agent Company High Water\" TYPE=\"Company\"><FETCH>GUID,ALTVCHID,ALTMSTID</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

fn xml_scalar(xml: &str, tag: &str) -> Value {
    let open = format!("<{tag}");
    let Some(start) = xml.find(&open) else {
        return Value::String("not_observed".to_string());
    };
    let Some(value_start) = xml[start..].find('>') else {
        return Value::String("not_observed".to_string());
    };
    let value_start = start + value_start + 1;
    let close = format!("</{tag}>");
    let Some(value_end) = xml[value_start..].find(&close) else {
        return Value::String("not_observed".to_string());
    };
    Value::String(xml[value_start..value_start + value_end].trim().to_string())
}

fn parse_agent_changed_masters(xml: &str) -> Vec<Value> {
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
                        rows.push(json!({"kind": kind.to_ascii_lowercase(), "name": fields.get("NAME"), "parent": fields.get("PARENT"), "alter_id": fields.get("ALTERID").and_then(|value| value.parse::<u64>().ok()), "master_id": fields.get("MASTERID")}));
                    }
                }
                tag.clear();
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    rows
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_agent_rows(xml: &str) -> Vec<Value> {
    // Tally's collection XML varies by release; use a deliberately conservative
    // extractor and never infer a missing field. Unknown collection shapes return
    // an empty result rather than invented records.
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut rows = Vec::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut current_tag = String::new();
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if tag == "VOUCHER" {
                    current = Some(BTreeMap::new());
                }
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some(row) = current.as_mut() {
                    if let Ok(text) = text.decode() {
                        row.insert(current_tag.clone(), text.into_owned());
                    }
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                if String::from_utf8_lossy(event.name().as_ref()).eq_ignore_ascii_case("VOUCHER") {
                    if let Some(row) = current.take() {
                        rows.push(json!({"date": row.get("DATE"), "voucher_number": row.get("VOUCHERNUMBER"), "voucher_type": row.get("VOUCHERTYPENAME"), "party": row.get("PARTYLEDGERNAME"), "narration": row.get("NARRATION"), "guid": row.get("GUID"), "alter_id": row.get("ALTERID").and_then(|v| v.parse::<u64>().ok()), "master_id": row.get("MASTERID"), "amounts": []}));
                    }
                }
                current_tag.clear();
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    rows
}

fn tool_definitions() -> Value {
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
                        "Return paired native receivable/payable totals, ageing, top exposure, and any exact partial reason.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid"],"properties":{"company_guid":{"type":"string","minLength":1},"direction":{"type":"string","enum":["receivable","payable","both"],"default":"both"},"as_of":{"type":"string","pattern":"^[0-9]{4}-?[0-9]{2}-?[0-9]{2}$"},"ageing_basis":{"type":"string","enum":["bill_date","due_date"],"default":"due_date"},"top":{"type":"integer","minimum":1,"default":25}}}),
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
                        "Return AlterID-filtered voucher and master evidence; deletion detection remains unsupported.",
                        json!({"type":"object","additionalProperties":false,"required":["company_guid","alter_id"],"properties":{"company_guid":{"type":"string","minLength":1},"alter_id":{"type":"integer","minimum":0}}}),
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
            "initialize" => Ok(
                json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}}),
            ),
            "notifications/initialized" => {
                if id.is_none() {
                    continue;
                }
                Ok(json!({}))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
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
        );
        assert!(request.contains("<FILTERS>BridgeAgentWindow</FILTERS>"));
        assert!(request.contains("$AlterID > 99"));
        assert_eq!(
            redact_value(
                json!({"party":"Acme Party", "narration":"private"}),
                Redaction::MaskParties
            )["party"],
            "Ac…ty"
        );
        assert!(tool_definitions()
            .as_array()
            .is_some_and(|tools| tools.len() == 13));
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
        });
        let down_response = down.call_tool("tally_status", json!({})).await;
        assert!(down_response["structuredContent"]["result"]["error"]["code"].is_string());
        let receipts = fs::read_to_string(directory.path().join("agent-egress.jsonl"))
            .expect("egress receipts");
        assert_eq!(receipts.lines().count(), 2);
    }
}
