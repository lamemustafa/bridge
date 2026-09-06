//! Company for the local MCP adapter.
use super::*;

impl Server {
    pub(super) async fn status(&self) -> Result<(Value, Evidence), String> {
        let endpoint = endpoint_origin(&self.settings.endpoint)?;
        let (probe, wire_evidence) = self
            .runtime
            .probe_with_wire_evidence(self.tally_config())
            .await
            .map_err(|_| "status_probe_unavailable".to_string())?;
        Ok((
            json!({
                "product": serde_json::to_value(&probe.connection.product).unwrap_or_else(|_| json!("not_observed")),
                "release": probe.profile.release,
                "education_mode": probe.profile.mode,
                "endpoint": endpoint,
                "loaded_companies": probe.companies,
                "refusal_reason": Value::Null,
            }),
            evidence_from_runtime_read(wire_evidence),
        ))
    }

    pub(super) async fn companies(&self) -> Result<(Vec<TallyCompany>, Evidence), String> {
        let company_list = self
            .runtime
            .fetch_agent_companies(self.tally_config())
            .await
            .map_err(|_| "company_collection_invalid".to_string())?;
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
                    }
                    .into());
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
    json!({"name": company.name, "guid": guid, "company_number": company.company_number, "books_from": company.books_from, "identity_state": if duplicate_guid {"ambiguous_duplicate_guid"} else if missing.is_some() {"incomplete_tuple"} else {"verified_tuple"}, "missing_field": missing})
}
