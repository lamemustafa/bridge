//! Prepared response commitments and confirmed stdio write completion.
//! Completion confirms write_all and flush, never consumption by an MCP client.
use super::*;

#[derive(Serialize)]
struct EgressReceipt<'a> {
    record_type: &'static str,
    receipt_id: String,
    ts: String,
    tool: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_name_sha256: Option<String>,
    args_sha256: String,
    company_guid: Option<String>,
    rows_prepared: usize,
    fields_prepared: Vec<String>,
    bytes_prepared: usize,
    enforced_bytes: usize,
    response_sha256: String,
    truncated: bool,
    redaction_preset: &'a str,
}

// Only a successfully persisted preparation can produce a completion token.
pub(super) struct PreparedReceipt {
    receipt_id: String,
    response_sha256: String,
    bytes: usize,
}

impl Server {
    pub(super) fn append_framed_egress(
        &self,
        context: EgressContext,
        response: &Value,
        serialized_response: &str,
    ) -> Result<PreparedReceipt, String> {
        let structured = response
            .get("result")
            .and_then(|result| result.get("structuredContent"));
        let rows_prepared = structured.and_then(response_row_count).unwrap_or_default();
        let truncated = structured
            .and_then(|value| value.get("truncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let fields_prepared = structured
            .map(agent_receipt_fields::released_fields)
            .unwrap_or_default();
        let path = self.settings.data_dir.join("agent-egress.jsonl");
        let (tool, tool_name_sha256) = receipt_tool_identity(&context.tool);
        let receipt = EgressReceipt {
            record_type: "response_prepared",
            receipt_id: uuid::Uuid::new_v4().to_string(),
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            tool,
            tool_name_sha256,
            args_sha256: context.args_sha256,
            company_guid: context
                .company_guid
                .as_deref()
                .and_then(egress::canonical_company_guid),
            rows_prepared,
            fields_prepared,
            bytes_prepared: serialized_response.len(),
            enforced_bytes: self.settings.max_bytes,
            response_sha256: sha256_hex(serialized_response.as_bytes()),
            truncated,
            redaction_preset: self.settings.redaction.label(),
        };
        let line = serde_json::to_string(&receipt)
            .map_err(|_| "egress_record_write_failed".to_string())?;
        append_egress_line(&path, &line)?;
        Ok(PreparedReceipt {
            receipt_id: receipt.receipt_id,
            response_sha256: receipt.response_sha256,
            bytes: receipt.bytes_prepared,
        })
    }

    pub(super) fn append_notification_refusal_egress(
        &self,
        tool: &str,
        args: &Value,
    ) -> Result<(), String> {
        let refusal = "tools_call_notification_forbidden";
        let (tool, tool_name_sha256) = receipt_tool_identity(tool);
        let receipt = EgressReceipt {
            record_type: "notification_refused",
            receipt_id: uuid::Uuid::new_v4().to_string(),
            ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            tool,
            tool_name_sha256,
            args_sha256: sha256_json(args),
            company_guid: args
                .get("company_guid")
                .and_then(Value::as_str)
                .and_then(egress::canonical_company_guid),
            rows_prepared: 0,
            fields_prepared: Vec::new(),
            bytes_prepared: 0,
            enforced_bytes: self.settings.max_bytes,
            response_sha256: sha256_hex(refusal.as_bytes()),
            truncated: false,
            redaction_preset: self.settings.redaction.label(),
        };
        let line = serde_json::to_string(&receipt)
            .map_err(|_| "egress_record_write_failed".to_string())?;
        append_egress_line(&self.settings.data_dir.join("agent-egress.jsonl"), &line)
    }

    pub(super) fn append_stdio_write_completed(
        &self,
        prepared: PreparedReceipt,
    ) -> Result<(), String> {
        let record = json!({
            "record_type": "stdio_write_completed",
            "receipt_id": prepared.receipt_id,
            "ts": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "response_sha256": prepared.response_sha256,
            "bytes_written": prepared.bytes,
        });
        append_egress_line(
            &self.settings.data_dir.join("agent-egress.jsonl"),
            &record.to_string(),
        )
    }
}

#[cfg(test)]
#[path = "agent_delivery_tests.rs"]
mod tests;
