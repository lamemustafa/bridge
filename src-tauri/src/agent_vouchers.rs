//! Vouchers for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn vouchers(&self, args: &Value) -> Result<ToolOutcome, String> {
        let guid = required_string(args, "company_guid")?;
        let from = normalized_date(required_string(args, "from")?)?;
        let to = normalized_date(required_string(args, "to")?)?;
        if from > to {
            return Err("invalid_date_range".to_string());
        }
        let (company, identity, mut identity_evidence) = self.verified_company(guid).await?;
        let requested_ledger = optional_string(args, "ledger")?;
        let resolved_ledger = if let Some(requested) = requested_ledger {
            let (ledgers, catalogue_evidence) =
                self.read_ledger_catalogue(&identity, &company.name).await?;
            identity_evidence = combine_evidence(identity_evidence, catalogue_evidence);
            Some(resolve_ledger_name(
                ledgers.iter().map(String::as_str),
                &requested,
            )?)
        } else {
            None
        };
        let request = render_agent_vouchers(&company.name, &from, &to, None)?;
        let (xml, mut evidence) = self.post_read(&identity, request).await?;
        let mut rows =
            validate_then_filter_voucher_rows(parse_agent_rows(&xml)?, &from, &to, None)?;
        let mut result_state = "complete";
        let mut corroboration_reason = None;
        if rows.is_empty() {
            let (read_evidence, partial, reason) = self
                .corroborate_empty_voucher_read(&identity, &company.name, &from, &to, None)
                .await?;
            evidence = combine_evidence(evidence, read_evidence);
            if partial {
                result_state = "partial";
                evidence.state = "partial";
            }
            if corroboration_reason.is_none() {
                corroboration_reason = reason;
                evidence.reason_code = reason.map(str::to_string);
            }
        }
        // A nonempty, validated source can legitimately have no selector match.
        // Corroborate actual source emptiness before any client-side selector.
        if let Some(ledger) = resolved_ledger {
            rows = filter_voucher_rows_for_ledger(rows, &ledger);
        }
        if let Some(kind) = optional_string(args, "voucher_type")? {
            rows.retain(|row| {
                row.get("voucher_type").and_then(Value::as_str) == Some(kind.as_str())
            });
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
        Ok(ToolOutcome {
            payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"state": result_state, "reason": corroboration_reason, "items": items, "offset": offset, "total": total, "profile": "agent_vouchers_v1_filters"}}),
            evidence: combine_evidence(identity_evidence, evidence),
            company_guid: Some(guid.to_string()),
            truncated,
        })
    }

    pub(super) async fn corroborate_empty_voucher_read(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        from: &str,
        to: &str,
        ledger: Option<&str>,
    ) -> Result<(Evidence, bool, Option<&'static str>), String> {
        let (wider_from, wider_to) = widened_window(from, to)?;
        let wider_request = render_agent_vouchers(company, &wider_from, &wider_to, None)?;
        let (wider_xml, wider_evidence) = self.post_read(identity, wider_request).await?;
        let mut evidence = wider_evidence;
        let wider_rows = validate_then_filter_voucher_rows(
            parse_agent_rows(&wider_xml)?,
            &wider_from,
            &wider_to,
            ledger,
        )?;
        let high_water = if wider_rows.is_empty() {
            let (high_water_xml, high_water_evidence) = self
                .post_read(identity, render_agent_company_high_water(company))
                .await?;
            evidence = combine_evidence(evidence, high_water_evidence);
            Some(
                parse_company_high_water(&high_water_xml, identity.company_guid())?["altvchid"]
                    .as_u64()
                    .ok_or_else(|| "voucher_checkpoint_invalid".to_string())?,
            )
        } else {
            None
        };
        let (partial, reason) =
            corroborate_empty_voucher_window(&wider_rows, from, to, high_water)?;
        Ok((evidence, partial, reason))
    }
}
