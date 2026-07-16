use bridge_tally_core::{
    CapabilityPackId, CapabilityState, PackSchemaVersion, TransportId,
    BILLS_AND_PAYMENTS_SCHEMA_VERSION, CORE_ACCOUNTING_SCHEMA_VERSION,
};
use std::collections::{BTreeMap, BTreeSet};

/// A canonical field that must be observed before Bridge can call a pack ready.
///
/// These are Bridge contract names rather than raw Tally XML tag names. A query
/// profile owns the mapping from the release-specific Tally representation to
/// this stable contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RequiredField {
    pub object_type: &'static str,
    pub field: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityPackDescriptor {
    pub id: CapabilityPackId,
    pub schema_version: PackSchemaVersion,
    pub required_fields: &'static [RequiredField],
}

const CORE_ACCOUNTING_FIELDS: &[RequiredField] = &[
    RequiredField {
        object_type: "group",
        field: "source_id",
    },
    RequiredField {
        object_type: "group",
        field: "name",
    },
    RequiredField {
        object_type: "group",
        field: "parent_source_id",
    },
    RequiredField {
        object_type: "ledger",
        field: "source_id",
    },
    RequiredField {
        object_type: "ledger",
        field: "name",
    },
    RequiredField {
        object_type: "ledger",
        field: "parent_source_id",
    },
    RequiredField {
        object_type: "ledger",
        field: "opening_balance",
    },
    RequiredField {
        object_type: "voucher_type",
        field: "source_id",
    },
    RequiredField {
        object_type: "voucher_type",
        field: "name",
    },
    RequiredField {
        object_type: "voucher",
        field: "source_id",
    },
    RequiredField {
        object_type: "voucher",
        field: "date_yyyymmdd",
    },
    RequiredField {
        object_type: "voucher",
        field: "voucher_type_source_id",
    },
    RequiredField {
        object_type: "voucher",
        field: "voucher_number",
    },
    RequiredField {
        object_type: "voucher",
        field: "cancelled",
    },
    RequiredField {
        object_type: "voucher",
        field: "optional",
    },
    RequiredField {
        object_type: "ledger_entry",
        field: "source_id",
    },
    RequiredField {
        object_type: "ledger_entry",
        field: "voucher_source_id",
    },
    RequiredField {
        object_type: "ledger_entry",
        field: "ledger_source_id",
    },
    RequiredField {
        object_type: "ledger_entry",
        field: "amount",
    },
    RequiredField {
        object_type: "ledger_entry",
        field: "polarity",
    },
];

const INDIA_TAX_FIELDS: &[RequiredField] = &[
    RequiredField {
        object_type: "tax_registration",
        field: "source_id",
    },
    RequiredField {
        object_type: "tax_registration",
        field: "owner_kind",
    },
    RequiredField {
        object_type: "tax_registration",
        field: "owner_source_id",
    },
    RequiredField {
        object_type: "tax_registration",
        field: "registration_type",
    },
    RequiredField {
        object_type: "tax_registration",
        field: "gstin",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "source_id",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "voucher_source_id",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "place_of_supply",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "assessable_value",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "tax_component",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "tax_rate",
    },
    RequiredField {
        object_type: "voucher_tax",
        field: "tax_amount",
    },
];

const BILLS_AND_PAYMENTS_FIELDS: &[RequiredField] = &[
    RequiredField {
        object_type: "party_outstanding",
        field: "source_identity",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "party_ledger_source_id",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "report_as_of_yyyymmdd",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "direction",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "bill_wise_state",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "allocation_coverage",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "outstanding_coverage",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "fetch_bracket",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "query_profile",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "source_scope_fingerprint",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "source_reported_allocation_count",
    },
    RequiredField {
        object_type: "party_outstanding",
        field: "source_reported_outstanding_count",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "source_id",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "identity_basis",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "origin",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "reference",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "due_date_yyyymmdd",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "due_date_evidence",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "amount",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "observed_polarity",
    },
    RequiredField {
        object_type: "bill_allocation",
        field: "currency_basis",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "source_id",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "identity_basis",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "origin",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "reference",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "due_date_yyyymmdd",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "due_date_evidence",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "pending_amount",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "observed_polarity",
    },
    RequiredField {
        object_type: "bill_outstanding",
        field: "currency_basis",
    },
];

const INVENTORY_FIELDS: &[RequiredField] = &[
    RequiredField {
        object_type: "stock_item",
        field: "source_id",
    },
    RequiredField {
        object_type: "stock_item",
        field: "name",
    },
    RequiredField {
        object_type: "stock_item",
        field: "base_unit",
    },
    RequiredField {
        object_type: "godown",
        field: "source_id",
    },
    RequiredField {
        object_type: "godown",
        field: "name",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "source_id",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "voucher_source_id",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "stock_item_source_id",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "godown_source_id",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "quantity",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "rate",
    },
    RequiredField {
        object_type: "inventory_entry",
        field: "amount",
    },
];

pub const CAPABILITY_PACKS: [CapabilityPackDescriptor; 4] = [
    CapabilityPackDescriptor {
        id: CapabilityPackId::CoreAccounting,
        schema_version: CORE_ACCOUNTING_SCHEMA_VERSION,
        required_fields: CORE_ACCOUNTING_FIELDS,
    },
    CapabilityPackDescriptor {
        id: CapabilityPackId::IndiaTax,
        schema_version: PackSchemaVersion { major: 1, minor: 0 },
        required_fields: INDIA_TAX_FIELDS,
    },
    CapabilityPackDescriptor {
        id: CapabilityPackId::BillsAndPayments,
        schema_version: BILLS_AND_PAYMENTS_SCHEMA_VERSION,
        required_fields: BILLS_AND_PAYMENTS_FIELDS,
    },
    CapabilityPackDescriptor {
        id: CapabilityPackId::Inventory,
        schema_version: PackSchemaVersion { major: 1, minor: 0 },
        required_fields: INVENTORY_FIELDS,
    },
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PackProfileScope {
    pub product: String,
    pub release: String,
    pub mode: String,
    pub transport: TransportId,
    /// Stable identifier for the exact query and normalisation contract.
    pub query_profile: String,
}

impl PackProfileScope {
    pub fn is_exact(&self) -> bool {
        [
            self.product.as_str(),
            self.release.as_str(),
            self.mode.as_str(),
            self.query_profile.as_str(),
        ]
        .into_iter()
        .all(|value| !value.trim().is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPackEvidence {
    pub observed_fields: BTreeSet<(String, String)>,
    /// The exact release/query profile completed successfully against Tally.
    pub query_completed: bool,
    /// Pack-specific invariants (for example exact-decimal parsing and stable
    /// identities) passed for the observed response.
    pub required_invariants_verified: bool,
    /// An explicit negative probe may mark a pack unsupported. Absence of a
    /// field is not an unsupported verdict; it remains unknown.
    pub explicitly_unsupported_reason: Option<String>,
}

impl ObservedPackEvidence {
    pub fn from_fields(
        fields: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        Self {
            observed_fields: fields
                .into_iter()
                .map(|(object_type, field)| (object_type.into(), field.into()))
                .collect(),
            query_completed: false,
            required_invariants_verified: false,
            explicitly_unsupported_reason: None,
        }
    }

    pub fn verified_fields(
        fields: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let mut evidence = Self::from_fields(fields);
        evidence.query_completed = true;
        evidence.required_invariants_verified = true;
        evidence
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSupportAssessment {
    pub state: CapabilityState,
    pub safe_reason_code: &'static str,
    pub missing_required_fields: Vec<RequiredField>,
}

/// Release/query-profile scoped pack evidence. A fresh registry deliberately
/// contains no optimistic support claims.
#[derive(Debug, Clone, Default)]
pub struct CapabilityPackRegistry {
    evidence: BTreeMap<(PackProfileScope, CapabilityPackId), ObservedPackEvidence>,
}

impl CapabilityPackRegistry {
    pub fn descriptor(pack: CapabilityPackId) -> &'static CapabilityPackDescriptor {
        CAPABILITY_PACKS
            .iter()
            .find(|descriptor| descriptor.id == pack)
            .expect("every public capability pack has a descriptor")
    }

    pub fn record_observation(
        &mut self,
        scope: PackProfileScope,
        pack: CapabilityPackId,
        evidence: ObservedPackEvidence,
    ) -> Result<(), &'static str> {
        if !scope.is_exact() {
            return Err("pack_profile_scope_incomplete");
        }
        self.evidence.insert((scope, pack), evidence);
        Ok(())
    }

    pub fn assess(
        &self,
        scope: &PackProfileScope,
        pack: CapabilityPackId,
    ) -> PackSupportAssessment {
        if !scope.is_exact() {
            return PackSupportAssessment {
                state: CapabilityState::Unknown,
                safe_reason_code: "pack_profile_scope_incomplete",
                missing_required_fields: Vec::new(),
            };
        }

        let Some(evidence) = self.evidence.get(&(scope.clone(), pack)) else {
            return PackSupportAssessment {
                state: CapabilityState::Unknown,
                safe_reason_code: "pack_not_observed_for_profile",
                missing_required_fields: Vec::new(),
            };
        };

        if evidence.explicitly_unsupported_reason.is_some() {
            return PackSupportAssessment {
                state: CapabilityState::Unsupported,
                safe_reason_code: "pack_explicitly_unsupported",
                missing_required_fields: Vec::new(),
            };
        }

        let descriptor = Self::descriptor(pack);
        let missing_required_fields = descriptor
            .required_fields
            .iter()
            .copied()
            .filter(|required| {
                !evidence
                    .observed_fields
                    .contains(&(required.object_type.to_string(), required.field.to_string()))
            })
            .collect::<Vec<_>>();

        if missing_required_fields.is_empty()
            && evidence.query_completed
            && evidence.required_invariants_verified
        {
            PackSupportAssessment {
                state: CapabilityState::Supported,
                safe_reason_code: "all_required_pack_fields_observed",
                missing_required_fields,
            }
        } else {
            PackSupportAssessment {
                state: CapabilityState::Unknown,
                safe_reason_code: "pack_readiness_not_fully_observed",
                missing_required_fields,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(release: &str) -> PackProfileScope {
        PackProfileScope {
            product: "tally_prime".to_string(),
            release: release.to_string(),
            mode: "education".to_string(),
            transport: TransportId::XmlHttp,
            query_profile: "core-v1-sha256:synthetic".to_string(),
        }
    }

    #[test]
    fn registry_is_unknown_by_default_for_every_pack() {
        let registry = CapabilityPackRegistry::default();
        for descriptor in CAPABILITY_PACKS {
            let assessment = registry.assess(&scope("7.0"), descriptor.id);
            assert_eq!(assessment.state, CapabilityState::Unknown);
            assert_eq!(assessment.safe_reason_code, "pack_not_observed_for_profile");
            assert!(!descriptor.required_fields.is_empty());
        }
    }

    #[test]
    fn a_pack_is_supported_only_when_every_required_field_was_observed() {
        for descriptor in CAPABILITY_PACKS {
            let mut registry = CapabilityPackRegistry::default();
            let all_fields = descriptor
                .required_fields
                .iter()
                .map(|field| (field.object_type, field.field));
            registry
                .record_observation(
                    scope("7.0"),
                    descriptor.id,
                    ObservedPackEvidence::verified_fields(all_fields),
                )
                .expect("exact scope");

            assert_eq!(
                registry.assess(&scope("7.0"), descriptor.id).state,
                CapabilityState::Supported
            );
        }
    }

    #[test]
    fn partial_evidence_stays_unknown_and_is_release_scoped() {
        let descriptor = CapabilityPackRegistry::descriptor(CapabilityPackId::CoreAccounting);
        let mut registry = CapabilityPackRegistry::default();
        let all_but_last = descriptor.required_fields[..descriptor.required_fields.len() - 1]
            .iter()
            .map(|field| (field.object_type, field.field));
        registry
            .record_observation(
                scope("7.0"),
                descriptor.id,
                ObservedPackEvidence::verified_fields(all_but_last),
            )
            .expect("exact scope");

        let partial = registry.assess(&scope("7.0"), descriptor.id);
        assert_eq!(partial.state, CapabilityState::Unknown);
        assert_eq!(partial.missing_required_fields.len(), 1);
        assert_eq!(
            registry.assess(&scope("7.1"), descriptor.id).state,
            CapabilityState::Unknown
        );
    }

    #[test]
    fn complete_field_names_without_a_successful_query_and_invariants_stay_unknown() {
        let descriptor = CapabilityPackRegistry::descriptor(CapabilityPackId::Inventory);
        let mut registry = CapabilityPackRegistry::default();
        registry
            .record_observation(
                scope("7.0"),
                descriptor.id,
                ObservedPackEvidence::from_fields(
                    descriptor
                        .required_fields
                        .iter()
                        .map(|field| (field.object_type, field.field)),
                ),
            )
            .expect("exact scope");

        let assessment = registry.assess(&scope("7.0"), descriptor.id);
        assert_eq!(assessment.state, CapabilityState::Unknown);
        assert!(assessment.missing_required_fields.is_empty());
        assert_eq!(
            assessment.safe_reason_code,
            "pack_readiness_not_fully_observed"
        );
    }

    #[test]
    fn incomplete_profiles_cannot_store_or_claim_support() {
        let mut incomplete = scope(" ");
        let mut registry = CapabilityPackRegistry::default();
        assert_eq!(
            registry.record_observation(
                incomplete.clone(),
                CapabilityPackId::Inventory,
                ObservedPackEvidence::from_fields([] as [(&str, &str); 0]),
            ),
            Err("pack_profile_scope_incomplete")
        );
        assert_eq!(
            registry
                .assess(&incomplete, CapabilityPackId::Inventory)
                .state,
            CapabilityState::Unknown
        );

        incomplete.release = "7.0".to_string();
        assert!(incomplete.is_exact());
    }
}
