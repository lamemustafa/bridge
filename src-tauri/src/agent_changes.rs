//! Changes for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn changed_since(&self, args: &Value) -> Result<ToolOutcome, ToolFailure> {
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
                    let mut master_snapshot = 0;
                    let mut snapshot_evidence = evidence;
                    for kind in MasterKind::ALL {
                        let (xml, evidence) = self
                            .post_read(
                                &identity,
                                render_agent_master_domain_high_water(&company.name, kind),
                            )
                            .await?;
                        master_snapshot =
                            master_snapshot.max(parse_master_domain_high_water(&xml)?);
                        snapshot_evidence = combine_evidence(snapshot_evidence, evidence);
                    }
                    (
                        json!({"altvchid": voucher_snapshot, "altmstid": master_snapshot}),
                        snapshot_evidence,
                    )
                }
                _ => return Err("change_snapshot_incomplete".to_string().into()),
            };
        let voucher_snapshot = snapshot["altvchid"]
            .as_u64()
            .ok_or_else(|| "voucher_checkpoint_invalid".to_string())?;
        let master_snapshot = snapshot["altmstid"]
            .as_u64()
            .ok_or_else(|| "master_checkpoint_invalid".to_string())?;
        if voucher_alter_id > voucher_snapshot || master_alter_id > master_snapshot {
            return Err("change_checkpoint_exceeds_snapshot".to_string().into());
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
        let mut all_masters = Vec::new();
        let mut combined_evidence = combine_evidence(evidence, snapshot_evidence);
        for kind in MasterKind::ALL {
            let (xml, evidence) = self
                .post_read(
                    &identity,
                    render_agent_changed_masters(
                        &company.name,
                        master_alter_id,
                        master_snapshot,
                        kind,
                    ),
                )
                .await?;
            all_masters.extend(parse_agent_changed_masters(&xml)?);
            combined_evidence = combine_evidence(combined_evidence, evidence);
        }
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
        Ok(ToolOutcome {
            payload: json!({"company": company_json(&company, std::slice::from_ref(&company)), "result": {"vouchers": rows, "masters": masters, "voucher_alter_id": voucher_alter_id, "master_alter_id": master_alter_id, "voucher_snapshot_alter_id": voucher_snapshot, "master_snapshot_alter_id": master_snapshot, "next_voucher_alter_id": next_voucher_alter_id, "next_master_alter_id": next_master_alter_id, "checkpoint_advanceable": checkpoint_advanceable, "checkpoint_reason": (!checkpoint_advanceable).then_some("scan_not_correlated_to_company_high_water"), "deletion_detection": "unsupported_alterid_does_not_observe_deletions", "current_company_high_water": snapshot}}),
            evidence: combine_evidence(identity_evidence, combined_evidence),
            company_guid: Some(guid.to_string()),
            truncated: voucher_truncated || master_truncated,
        })
    }
}
