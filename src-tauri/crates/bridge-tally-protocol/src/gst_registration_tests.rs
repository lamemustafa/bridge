use super::*;

fn raw(date: Option<&str>, gstin: Option<&str>) -> RawGstRegistrationEntry {
    RawGstRegistrationEntry {
        applicable_from: date.map(str::to_string),
        gstin: gstin.map(str::to_string),
        registration_type: Some("Regular".to_string()),
        repeated_field: false,
    }
}

const A: &str = "27ZZZZZ0000Z1Z5";
const B: &str = "29ZZZZZ0000Z1Z5";

#[test]
fn the_gstin_in_force_is_the_latest_entry_on_or_before_the_date_not_the_first() {
    let history = GstRegistrationHistory::from_raw(vec![
        raw(Some("20250701"), Some(A)),
        raw(Some("20250401"), None),
        raw(Some("20251001"), Some(B)),
    ]);
    let gstin = |as_of| history.in_force(as_of).and_then(|e| e.gstin.as_deref());
    assert_eq!(gstin("20250331"), None, "before any entry");
    assert_eq!(gstin("20250601"), None, "the blank first entry is in force");
    assert_eq!(gstin("20250701"), Some(A), "on the entry's own date");
    assert_eq!(gstin("20250930"), Some(A));
    assert_eq!(
        gstin("20260331"),
        Some(B),
        "a later registration replaces it"
    );
}

#[test]
fn an_empty_placeholder_is_an_observed_empty_history_not_an_unobserved_one() {
    assert_eq!(
        GstRegistrationHistory::from_raw(vec![RawGstRegistrationEntry::default()]),
        GstRegistrationHistory::Entries { entries: vec![] }
    );
    assert_ne!(
        GstRegistrationHistory::from_raw(vec![]),
        GstRegistrationHistory::NotObserved
    );
}

#[test]
fn each_malformed_history_is_unreadable_with_its_own_reason() {
    let unreadable = |raw_entries| match GstRegistrationHistory::from_raw(raw_entries) {
        GstRegistrationHistory::Unreadable { defect } => Some(defect),
        _ => None,
    };
    assert_eq!(
        unreadable(vec![raw(None, Some(A))]),
        Some(GstRegistrationDefect::EntryWithoutDate)
    );
    assert_eq!(
        unreadable(vec![raw(Some("2025-07-01"), Some(A))]),
        Some(GstRegistrationDefect::DateInvalid)
    );
    assert_eq!(
        unreadable(vec![raw(Some("20250231"), Some(A))]),
        Some(GstRegistrationDefect::DateInvalid),
        "eight digits that are not a calendar date"
    );
    assert_eq!(
        unreadable(vec![RawGstRegistrationEntry {
            repeated_field: true,
            ..RawGstRegistrationEntry::default()
        }]),
        Some(GstRegistrationDefect::EntryRepeatsAField),
        "a repeated field is a defect even when nothing else was read"
    );
    assert_eq!(
        unreadable(vec![raw(Some("20250701"), Some("27ZZZZZ0000Z1Z5A"))]),
        Some(GstRegistrationDefect::GstinMalformed),
        "sixteen characters"
    );
    assert_eq!(
        unreadable(vec![raw(Some("20250701"), Some("27zzzzz0000z1z5"))]),
        Some(GstRegistrationDefect::GstinMalformed)
    );
    assert_eq!(
        unreadable(vec![
            raw(Some("20250701"), Some(A)),
            raw(Some("20250701"), Some(B))
        ]),
        Some(GstRegistrationDefect::ConflictingEntriesOnOneDate)
    );
    // The same entry twice says one thing, so it is kept once.
    assert_eq!(
        GstRegistrationHistory::from_raw(vec![
            raw(Some("20250701"), Some(A)),
            raw(Some("20250701"), Some(A))
        ]),
        GstRegistrationHistory::Entries {
            entries: vec![GstRegistrationEntry {
                applicable_from: "20250701".to_string(),
                gstin: Some(A.to_string()),
                registration_type: Some("Regular".to_string()),
            }]
        }
    );
}

#[test]
fn nothing_is_in_force_from_an_unobserved_or_unreadable_history() {
    assert_eq!(
        GstRegistrationHistory::NotObserved.in_force("20260331"),
        None
    );
    assert_eq!(
        GstRegistrationHistory::Unreadable {
            defect: GstRegistrationDefect::DateInvalid
        }
        .in_force("20260331"),
        None
    );
}
