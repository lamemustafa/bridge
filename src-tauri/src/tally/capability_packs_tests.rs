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
