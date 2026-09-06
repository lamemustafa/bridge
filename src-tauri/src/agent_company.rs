//! Company for the local MCP adapter.
use super::*;
use bridge_tally_core::{CapabilityFeatureId, CapabilityState, EvidenceConfidence};

impl Server {
    pub(super) async fn status(&self) -> Result<(Value, Evidence), String> {
        let endpoint = endpoint_origin(&self.settings.endpoint)?;
        let (probe, wire_evidence) = self
            .runtime
            .probe_with_wire_evidence(self.tally_config())
            .await
            .map_err(|_| "status_probe_unavailable".to_string())?;
        // Product identity comes from the gateway observation, not the optional
        // status page's heuristic banner. Unknown capability stays explicit.
        let observed = probe
            .profile
            .features
            .get(&CapabilityFeatureId::ProductAndMode)
            .is_some_and(|feature| {
                feature.state == CapabilityState::Supported
                    && feature.confidence == EvidenceConfidence::Observed
            });
        let product = if observed {
            probe.profile.product.as_str()
        } else {
            "not_observed"
        };
        let education_mode = match (observed, probe.profile.mode.as_deref()) {
            (true, Some("Education")) => Some(true),
            (true, Some("Licensed")) => Some(false),
            _ => None,
        };
        Ok((
            json!({
                "product": product,
                "release": probe.profile.release,
                "license_tier": probe.profile.license_tier,
                "education_mode": education_mode,
                "endpoint": endpoint,
                "loaded_companies": probe.companies,
                "refusal_reason": Value::Null,
            }),
            evidence_from_runtime_read(wire_evidence),
        ))
    }

    pub(super) async fn companies(&self) -> Result<(Vec<TallyCompany>, Evidence), ToolFailure> {
        let company_list = self
            .runtime
            .fetch_agent_companies(self.tally_config())
            .await
            .map_err(|error| ToolFailure::from_runtime("company_collection_invalid", error))?;
        let evidence = Evidence {
            request_sha256: sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
                &ReadOnlyProfile::CompanyListV2.render(),
            )),
            response_sha256: company_list.response_sha256,
            bytes: company_list.response_bytes.saturating_mul(2),
            state: "complete",
            read_at: None,
            duration_ms: None,
            reason_code: None,
        };
        Ok((company_list.companies, evidence))
    }

    pub(super) async fn verified_company(
        &self,
        guid: &str,
    ) -> Result<(TallyCompany, VerifiedCompanyIdentity, Evidence), ToolFailure> {
        if guid.trim().is_empty() {
            return Err("company_guid_required".to_string().into());
        }
        let (companies, evidence) = self.companies().await?;
        let result: Result<(TallyCompany, VerifiedCompanyIdentity, Evidence), ToolFailure> =
            async {
                let observed_companies = companies.clone();
                let mut matches = Vec::new();
                for company in companies {
                    let observed_guid = company
                        .guid
                        .as_deref()
                        .filter(|guid| !guid.trim().is_empty())
                        .map(parse_native_company_guid)
                        .transpose()?;
                    if company
                        .guid
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case(guid))
                    {
                        if let Some(observed_guid) = observed_guid {
                            matches.push((company, observed_guid));
                        }
                    }
                }
                if matches.len() != 1 {
                    return Err(if matches.is_empty() {
                        "company_identity_not_found".to_string()
                    } else {
                        "company_identity_ambiguous".to_string()
                    }
                    .into());
                }
                let (company, observed_guid) =
                    matches.into_iter().next().expect("one checked above");
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
                    observed_guid.hyphenated().to_string(),
                    company_number,
                    books_from,
                    &observed_companies,
                )
                .map_err(|error| {
                    match error {
                        crate::tally::VerifiedCompanyIdentityError::Missing => {
                            "company_identity_not_found"
                        }
                        crate::tally::VerifiedCompanyIdentityError::DuplicateTuple => {
                            "company_identity_ambiguous"
                        }
                        crate::tally::VerifiedCompanyIdentityError::DisplayScopeAmbiguous => {
                            "company_display_scope_ambiguous"
                        }
                    }
                    .to_string()
                })?;
                Ok((company, identity, evidence.clone()))
            }
            .await;
        result.map_err(|failure| failure.with_prior_evidence(evidence))
    }
}

pub(super) fn parse_native_company_guid(value: &str) -> Result<uuid::Uuid, String> {
    // Native company GUIDs use hyphenated UUID spelling. Preserve case-insensitive
    // matching without adding UUID aliases that the native identity bracket rejects.
    let invalid = || "company_guid_invalid".to_string();
    if value.len() != 36 {
        return Err(invalid());
    }
    let guid = uuid::Uuid::parse_str(value).map_err(|_| invalid())?;
    if !guid.hyphenated().to_string().eq_ignore_ascii_case(value) {
        return Err(invalid());
    }
    Ok(guid)
}

pub(super) fn company_json(company: &TallyCompany, all: &[TallyCompany]) -> Value {
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
    let invalid_guid = guid
        .as_deref()
        .is_some_and(|guid| parse_native_company_guid(guid).is_err());
    json!({"name": company.name, "guid": guid, "company_number": company.company_number, "books_from": company.books_from, "identity_state": if duplicate_guid {"ambiguous_duplicate_guid"} else if missing.is_some() {"incomplete_tuple"} else if invalid_guid {"invalid_guid"} else {"verified_tuple"}, "missing_field": missing})
}

#[cfg(test)]
#[path = "agent_status_tests.rs"]
mod status_tests;

#[cfg(test)]
#[path = "agent_company_identity_tests.rs"]
mod identity_tests;
