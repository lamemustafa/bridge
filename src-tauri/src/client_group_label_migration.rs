// SPDX-License-Identifier: Apache-2.0

//! Pure, read-only planning for moving legacy client-group labels to durable
//! composite company keys. The command layer owns loading its inputs; this
//! module owns only deterministic classification and conflict detection.

use crate::client_groups::ClientGroupLabels;
use crate::db::tally_mirror::ClientGroupLabelMigrationProfile;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The disposition of one existing v1 client-group label in a proposed
/// migration. This is deliberately a plan only: none of its variants changes
/// the v1 label file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum ClientGroupLabelMigrationDisposition {
    Resolved {
        composite_key: String,
    },
    Ambiguous {
        composite_keys: Vec<String>,
    },
    IncompleteHistory {
        identified_composite_keys: Vec<String>,
        incomplete_profile_count: usize,
    },
    Conflict {
        conflicts: Vec<ClientGroupLabelMigrationConflict>,
    },
    Unmatched,
}

/// An existing label which would otherwise compete for one composite target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientGroupLabelMigrationConflictEntry {
    pub source_key: String,
    pub label: String,
}

/// All labels which would lose data if a proposed destination were applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientGroupLabelMigrationConflict {
    pub destination_key: String,
    pub competing_entries: Vec<ClientGroupLabelMigrationConflictEntry>,
}

/// One preserved v1 label and the composite candidates derived from durable
/// observed-company history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientGroupLabelMigrationEntry {
    pub source_key: String,
    pub label: String,
    pub disposition: ClientGroupLabelMigrationDisposition,
}

/// Read-only migration proposal for every existing v1 client-group label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientGroupLabelMigrationPlan {
    pub entries: Vec<ClientGroupLabelMigrationEntry>,
}

/// The complete identity state of one durable profile for this migration.
/// `documented` and `observed` rows with a durable key identify a book;
/// every other confidence/key combination is retained as incomplete history.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClientGroupLabelMigrationProfileState {
    Documented(Option<String>),
    Observed(Option<String>),
    Inferred(Option<String>),
    Unknown(Option<String>),
    InvalidConfidence(Option<String>),
}

/// Classifies existing raw-GUID labels against backend-issued, durable
/// correlation keys. The planner receives its inputs from the caller and does
/// no I/O, so classification cannot write, discard, or attach a label.
pub fn classify_client_group_label_migration(
    existing_labels: &ClientGroupLabels,
    persisted_profiles: &[ClientGroupLabelMigrationProfile],
) -> ClientGroupLabelMigrationPlan {
    let mut entries = existing_labels
        .iter()
        .map(|(source_key, label)| {
            let matching_profiles = persisted_profiles
                .iter()
                .filter(|profile| profile.guid.eq_ignore_ascii_case(source_key))
                .collect::<Vec<_>>();
            let mut composite_keys = BTreeSet::new();
            let mut incomplete_profile_count = 0;
            for state in matching_profiles
                .into_iter()
                .map(client_group_label_migration_profile_state)
            {
                match state {
                    ClientGroupLabelMigrationProfileState::Documented(Some(composite_key))
                    | ClientGroupLabelMigrationProfileState::Observed(Some(composite_key)) => {
                        composite_keys.insert(composite_key);
                    }
                    ClientGroupLabelMigrationProfileState::Documented(None)
                    | ClientGroupLabelMigrationProfileState::Observed(None)
                    | ClientGroupLabelMigrationProfileState::Inferred(Some(_))
                    | ClientGroupLabelMigrationProfileState::Inferred(None)
                    | ClientGroupLabelMigrationProfileState::Unknown(Some(_))
                    | ClientGroupLabelMigrationProfileState::Unknown(None)
                    | ClientGroupLabelMigrationProfileState::InvalidConfidence(Some(_))
                    | ClientGroupLabelMigrationProfileState::InvalidConfidence(None) => {
                        incomplete_profile_count += 1;
                    }
                }
            }
            let composite_keys = composite_keys.into_iter().collect::<Vec<_>>();
            let disposition = if incomplete_profile_count > 0 {
                ClientGroupLabelMigrationDisposition::IncompleteHistory {
                    identified_composite_keys: composite_keys,
                    incomplete_profile_count,
                }
            } else {
                match composite_keys.as_slice() {
                    [composite_key] => ClientGroupLabelMigrationDisposition::Resolved {
                        composite_key: composite_key.clone(),
                    },
                    [] => ClientGroupLabelMigrationDisposition::Unmatched,
                    _ => ClientGroupLabelMigrationDisposition::Ambiguous { composite_keys },
                }
            };
            ClientGroupLabelMigrationEntry {
                source_key: source_key.clone(),
                label: label.clone(),
                disposition,
            }
        })
        .collect::<Vec<_>>();
    apply_destination_conflicts(&mut entries, existing_labels);
    ClientGroupLabelMigrationPlan { entries }
}

fn client_group_label_migration_profile_state(
    profile: &ClientGroupLabelMigrationProfile,
) -> ClientGroupLabelMigrationProfileState {
    let correlation_key = profile.correlation_key.clone();
    match profile.identity_confidence.as_str() {
        "documented" => ClientGroupLabelMigrationProfileState::Documented(correlation_key),
        "observed" => ClientGroupLabelMigrationProfileState::Observed(correlation_key),
        "inferred" => ClientGroupLabelMigrationProfileState::Inferred(correlation_key),
        "unknown" => ClientGroupLabelMigrationProfileState::Unknown(correlation_key),
        _ => ClientGroupLabelMigrationProfileState::InvalidConfidence(correlation_key),
    }
}

fn apply_destination_conflicts(
    entries: &mut [ClientGroupLabelMigrationEntry],
    existing_labels: &ClientGroupLabels,
) {
    let mut claims = BTreeMap::<String, DestinationClaims>::new();
    for entry in entries.iter() {
        let ClientGroupLabelMigrationDisposition::Resolved { composite_key } = &entry.disposition
        else {
            continue;
        };
        let canonical_destination = composite_key.to_ascii_lowercase();
        let claim = claims
            .entry(canonical_destination)
            .or_insert_with(|| DestinationClaims::new(composite_key));
        claim.insert(&entry.source_key, &entry.label);
        for (source_key, label) in existing_labels
            .iter()
            .filter(|(source_key, _)| source_key.eq_ignore_ascii_case(composite_key))
        {
            claim.insert(source_key, label);
        }
    }

    let mut conflicts_by_source = BTreeMap::<String, Vec<ClientGroupLabelMigrationConflict>>::new();
    for claim in claims.into_values().filter(|claim| claim.entries.len() > 1) {
        let conflict = ClientGroupLabelMigrationConflict {
            destination_key: claim.destination_key,
            competing_entries: claim
                .entries
                .into_iter()
                .map(
                    |(source_key, label)| ClientGroupLabelMigrationConflictEntry {
                        source_key,
                        label,
                    },
                )
                .collect(),
        };
        for competing_entry in &conflict.competing_entries {
            conflicts_by_source
                .entry(competing_entry.source_key.clone())
                .or_default()
                .push(conflict.clone());
        }
    }

    for entry in entries {
        if let Some(conflicts) = conflicts_by_source.remove(&entry.source_key) {
            entry.disposition = ClientGroupLabelMigrationDisposition::Conflict { conflicts };
        }
    }
}

struct DestinationClaims {
    destination_key: String,
    entries: BTreeMap<String, String>,
}

impl DestinationClaims {
    fn new(destination_key: &str) -> Self {
        Self {
            destination_key: destination_key.to_string(),
            entries: BTreeMap::new(),
        }
    }

    fn insert(&mut self, source_key: &str, label: &str) {
        self.entries
            .insert(source_key.to_string(), label.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_client_group_label_migration, ClientGroupLabelMigrationConflict,
        ClientGroupLabelMigrationConflictEntry, ClientGroupLabelMigrationDisposition,
    };
    use crate::db::tally_mirror::ClientGroupLabelMigrationProfile;
    use std::collections::BTreeMap;

    fn persisted_profile(
        guid: &str,
        correlation_key: Option<&str>,
        identity_confidence: &str,
    ) -> ClientGroupLabelMigrationProfile {
        ClientGroupLabelMigrationProfile {
            guid: guid.to_string(),
            correlation_key: correlation_key.map(str::to_string),
            identity_confidence: identity_confidence.to_string(),
        }
    }

    #[test]
    fn closed_split_book_with_suppressed_history_requires_review() {
        let labels = BTreeMap::from([(
            "synthetic-shared-guid".to_string(),
            "Legacy practice".to_string(),
        )]);
        let plan = classify_client_group_label_migration(
            &labels,
            &[
                persisted_profile(
                    "synthetic-shared-guid",
                    Some("reappeared-book-key"),
                    "observed",
                ),
                persisted_profile("synthetic-shared-guid", None, "unknown"),
            ],
        );

        assert_eq!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::IncompleteHistory {
                identified_composite_keys: vec!["reappeared-book-key".to_string()],
                incomplete_profile_count: 1,
            }
        );
    }

    #[test]
    fn upgraded_install_with_only_suppressed_history_requires_review() {
        let labels = BTreeMap::from([(
            "synthetic-shared-guid".to_string(),
            "Legacy practice".to_string(),
        )]);
        let plan = classify_client_group_label_migration(
            &labels,
            &[
                persisted_profile("synthetic-shared-guid", None, "unknown"),
                persisted_profile("synthetic-shared-guid", None, "unknown"),
            ],
        );

        assert_eq!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::IncompleteHistory {
                identified_composite_keys: vec![],
                incomplete_profile_count: 2,
            }
        );
        assert_ne!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::Unmatched
        );
    }

    #[test]
    fn observed_profile_without_a_key_prevents_resolution() {
        let labels = BTreeMap::from([(
            "synthetic-shared-guid".to_string(),
            "Legacy practice".to_string(),
        )]);
        let plan = classify_client_group_label_migration(
            &labels,
            &[
                persisted_profile("synthetic-shared-guid", Some("identified-book"), "observed"),
                persisted_profile("synthetic-shared-guid", None, "observed"),
            ],
        );

        assert_eq!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::IncompleteHistory {
                identified_composite_keys: vec!["identified-book".to_string()],
                incomplete_profile_count: 1,
            }
        );
        assert_ne!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::Resolved {
                composite_key: "identified-book".to_string(),
            }
        );
    }

    #[test]
    fn documented_profile_with_a_key_can_resolve_a_label() {
        let labels = BTreeMap::from([(
            "synthetic-documented-guid".to_string(),
            "Documented practice".to_string(),
        )]);
        let plan = classify_client_group_label_migration(
            &labels,
            &[persisted_profile(
                "synthetic-documented-guid",
                Some("documented-book"),
                "documented",
            )],
        );

        assert_eq!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::Resolved {
                composite_key: "documented-book".to_string(),
            }
        );
    }

    #[test]
    fn raw_guid_with_multiple_composite_tuples_is_ambiguous() {
        let labels = BTreeMap::from([("synthetic-guid".to_string(), "Several books".to_string())]);
        let plan = classify_client_group_label_migration(
            &labels,
            &[
                persisted_profile("SYNTHETIC-GUID", Some("candidate-a"), "observed"),
                persisted_profile("synthetic-guid", Some("candidate-b"), "observed"),
                persisted_profile("synthetic-guid", Some("candidate-c"), "observed"),
            ],
        );

        assert!(matches!(
            &plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::Ambiguous { composite_keys }
                if composite_keys == &["candidate-a", "candidate-b", "candidate-c"]
        ));
    }

    #[test]
    fn raw_guid_without_a_durable_tuple_is_preserved_and_unmatched() {
        let labels = BTreeMap::from([(
            "legacy-only-guid".to_string(),
            "Legacy practice".to_string(),
        )]);
        let plan = classify_client_group_label_migration(&labels, &[]);

        assert_eq!(plan.entries[0].source_key, "legacy-only-guid");
        assert_eq!(plan.entries[0].label, "Legacy practice");
        assert_eq!(
            plan.entries[0].disposition,
            ClientGroupLabelMigrationDisposition::Unmatched
        );
    }

    #[test]
    fn destination_conflicts_preserve_existing_and_case_variant_labels() {
        let labels = BTreeMap::from([
            (
                "composite-correlation-key".to_string(),
                "Newer label".to_string(),
            ),
            (
                "SYNTHETIC-RAW-GUID".to_string(),
                "Legacy label uppercase".to_string(),
            ),
            ("synthetic-raw-guid".to_string(), "Legacy label".to_string()),
        ]);
        let plan = classify_client_group_label_migration(
            &labels,
            &[persisted_profile(
                "synthetic-raw-guid",
                Some("composite-correlation-key"),
                "observed",
            )],
        );
        let expected_conflict = ClientGroupLabelMigrationConflict {
            destination_key: "composite-correlation-key".to_string(),
            competing_entries: vec![
                ClientGroupLabelMigrationConflictEntry {
                    source_key: "SYNTHETIC-RAW-GUID".to_string(),
                    label: "Legacy label uppercase".to_string(),
                },
                ClientGroupLabelMigrationConflictEntry {
                    source_key: "composite-correlation-key".to_string(),
                    label: "Newer label".to_string(),
                },
                ClientGroupLabelMigrationConflictEntry {
                    source_key: "synthetic-raw-guid".to_string(),
                    label: "Legacy label".to_string(),
                },
            ],
        };

        assert_eq!(plan.entries.len(), 3);
        assert!(plan.entries.iter().all(|entry| {
            entry.disposition
                == ClientGroupLabelMigrationDisposition::Conflict {
                    conflicts: vec![expected_conflict.clone()],
                }
        }));
    }
}
