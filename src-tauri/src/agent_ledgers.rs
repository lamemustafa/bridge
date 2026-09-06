//! Ledgers for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn ledger_masters(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let (company, identity, company_evidence) = self.verified_company(guid).await?;
        let fields = optional_string(args, "fields")?.unwrap_or_else(|| "basic".to_string());
        let compliance = ledger_master_fields(&fields)?;
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
        let truncated = offset.saturating_add(page.len()) < total;
        let result = json!({"items": page, "offset": offset, "total": total, "fields": fields, "compliance": if compliance {"paired_party_ledger_master_source"} else {"not_requested"}});
        Ok(ToolOutcome {
            payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": result}),
            evidence: combine_evidence(
                company_evidence,
                evidence_from_runtime_read(ledger_evidence),
            ),
            company_guid: Some(guid.to_string()),
            truncated,
        })
    }
}
