//! Vouchers for the local MCP adapter.
use super::*;
use std::collections::BTreeSet;

impl Server {
    pub(super) async fn vouchers(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
        selected_voucher_operation(self, args).await
    }
}

/// The authoritative selected-voucher operation shared by the MCP and the
/// desktop presentation adapter. It owns source admission, catalogue
/// stability, validation, filtering, response shaping, and redaction; callers
/// only choose their adapter policy and dispatch this operation.
pub(crate) async fn selected_voucher_operation(
    server: &Server,
    args: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let guid = required_string(args, "company_guid")?;
    let from = normalized_date(required_string(args, "from")?)?;
    let to = normalized_date(required_string(args, "to")?)?;
    if from > to {
        return Err("invalid_date_range".to_string().into());
    }
    let (company, identity, accumulated) = server.verified_company(guid).await?;
    selected_voucher_operation_for_verified(
        server,
        args,
        VoucherOperationScope {
            guid: guid.to_string(),
            from,
            to,
            company,
            identity,
            initial_evidence: Some(accumulated),
        },
    )
    .await
}

/// Runs the same operation after the desktop command has already admitted the
/// exact observed company tuple. This avoids degrading that tuple to a GUID or
/// issuing another company-list read before the shared source read.
pub(crate) struct VoucherOperationScope {
    pub(crate) guid: String,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) company: TallyCompany,
    pub(crate) identity: VerifiedCompanyIdentity,
    pub(crate) initial_evidence: Option<Evidence>,
}

pub(crate) async fn selected_voucher_operation_for_verified(
    server: &Server,
    args: &Value,
    scope: VoucherOperationScope,
) -> Result<ToolOutcome, ToolFailure> {
    let VoucherOperationScope {
        guid,
        from,
        to,
        company,
        identity,
        initial_evidence,
    } = scope;
    let mut accumulated = initial_evidence;
    let outcome = async {
        let requested_ledger = optional_string(args, "ledger")?;
        let selected_catalogue = if let Some(requested) = requested_ledger {
            let (ledgers, catalogue_evidence) =
                server.read_ledger_catalogue(&identity, &company.name).await?;
            accumulate_evidence(&mut accumulated, catalogue_evidence);
            let resolved = resolve_ledger_name(
                ledgers.iter().map(String::as_str),
                &requested,
            )?;
            Some((resolved, ledgers))
        } else {
            None
        };
        let request = render_agent_vouchers(&company.name, &from, &to, None)?;
        let (xml, evidence) = server.post_read(&identity, request).await?;
        accumulate_evidence(&mut accumulated, evidence);
        let mut rows =
            validate_then_filter_voucher_rows(parse_agent_rows(&xml, identity.company_guid())?, &from, &to, None)?;
        let mut result_state = "complete";
        let mut corroboration_reason = None;
        if rows.is_empty() {
            let (read_evidence, partial, reason) = server
                .corroborate_empty_voucher_read(&identity, &company.name, &from, &to, None)
                .await?;
            accumulate_evidence(&mut accumulated, read_evidence);
            if partial {
                result_state = "partial";
                if let Some(evidence) = accumulated.as_mut() { evidence.state = "partial"; }
            }
            if corroboration_reason.is_none() {
                corroboration_reason = reason;
                if let Some(evidence) = accumulated.as_mut() { evidence.reason_code = reason.map(str::to_string); }
            }
        }
        // A nonempty, validated source can legitimately have no selector match.
        // Corroborate actual source emptiness before any client-side selector.
        if let Some((ledger, catalogue)) = selected_catalogue {
            let (corroboration, catalogue_evidence) =
                server.read_ledger_catalogue(&identity, &company.name).await?;
            accumulate_evidence(&mut accumulated, catalogue_evidence);
            let initial = catalogue.iter().map(String::as_str).collect::<BTreeSet<_>>();
            let repeated = corroboration.iter().map(String::as_str).collect::<BTreeSet<_>>();
            if initial.len() != catalogue.len()
                || repeated.len() != corroboration.len()
                || initial != repeated
                || rows.iter().flat_map(|row| row["amounts"].as_array().into_iter().flatten())
                    .any(|entry| !initial.contains(entry["ledger"].as_str().unwrap_or_default()))
            {
                return Err("ledger_snapshot_drifted".to_string().into());
            }
            rows = filter_voucher_rows_for_ledger(rows, &ledger);
        }
        if let Some(kind) = optional_string(args, "voucher_type")? {
            rows.retain(|row| {
                row.get("voucher_type").and_then(Value::as_str) == Some(kind.as_str())
            });
        }
        let offset = arg_usize(args, "offset", 0)?;
        let limit =
            arg_positive_usize(args, "limit", server.settings.max_rows)?.min(server.settings.max_rows);
        let total = rows.len();
        let items = rows
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|row| redact_value(mark_voucher_party_names(row), server.settings.redaction))
            .collect::<Vec<_>>();
        let truncated = offset.saturating_add(items.len()) < total;
        Ok(ToolOutcome {
            payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"state": result_state, "reason": corroboration_reason, "items": items, "offset": offset, "total": total, "profile": "agent_vouchers_v1_filters"}}),
            evidence: accumulated.clone().expect("voucher source evidence is present after admitted read"),
            company_guid: Some(guid),
            truncated,
        })
    }
    .await;
    outcome.map_err(|failure: ToolFailure| match accumulated {
        Some(evidence) => failure.with_prior_evidence(evidence),
        None => failure,
    })
}

fn accumulate_evidence(target: &mut Option<Evidence>, next: Evidence) {
    *target = Some(match target.take() {
        Some(current) => combine_evidence(current, next),
        None => next,
    });
}

impl Server {
    /// The nonempty counterpart of `corroborate_empty_voucher_read`: one wider
    /// read, compared against what the narrow read reported for the same range.
    ///
    /// It costs a second read on every window that has vouchers in it. The empty
    /// path already paid that, and the alternative is asserting completeness
    /// from a response that cannot show it.
    pub(super) async fn corroborate_nonempty_voucher_read(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        from: &str,
        to: &str,
        ledger: Option<&str>,
        rows: &[Value],
    ) -> Result<(Evidence, bool, Option<&'static str>), ToolFailure> {
        let (wider_from, wider_to) = widened_window(from, to)?;
        let wider_request = render_agent_vouchers(company, &wider_from, &wider_to, None)?;
        let (wider_xml, evidence) = self.post_read(identity, wider_request).await?;
        let outcome = async {
            let wider_rows = validate_then_filter_voucher_rows(
                parse_agent_rows(&wider_xml, identity.company_guid())?,
                &wider_from,
                &wider_to,
                ledger,
            )?;
            let (partial, reason) =
                corroborate_nonempty_voucher_window(rows, &wider_rows, from, to);
            Ok((evidence.clone(), partial, reason))
        }
        .await;
        outcome.map_err(|failure: ToolFailure| failure.with_prior_evidence(evidence))
    }

    pub(super) async fn corroborate_empty_voucher_read(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        from: &str,
        to: &str,
        ledger: Option<&str>,
    ) -> Result<(Evidence, bool, Option<&'static str>), ToolFailure> {
        let (wider_from, wider_to) = widened_window(from, to)?;
        let wider_request = render_agent_vouchers(company, &wider_from, &wider_to, None)?;
        let (wider_xml, wider_evidence) = self.post_read(identity, wider_request).await?;
        let mut evidence = wider_evidence;
        let outcome = async {
            let wider_rows = validate_then_filter_voucher_rows(
                parse_agent_rows(&wider_xml, identity.company_guid())?,
                &wider_from,
                &wider_to,
                ledger,
            )?;
            let high_water = if wider_rows.is_empty() {
                let (high_water_xml, high_water_evidence) = self
                    .post_read(identity, render_agent_company_high_water(company))
                    .await?;
                evidence = combine_evidence(evidence.clone(), high_water_evidence);
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
            Ok((evidence.clone(), partial, reason))
        }
        .await;
        outcome.map_err(|failure: ToolFailure| failure.with_prior_evidence(evidence))
    }
}
