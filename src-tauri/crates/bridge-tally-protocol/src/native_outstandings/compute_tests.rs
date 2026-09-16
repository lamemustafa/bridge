use crate::TALLY_SANITIZED_ROOT_MARKER;

use super::*;

#[test]
fn reserved_root_policy_matches_canonical_window_for_marker_carrying_parents() {
    for root in [
        format!("{TALLY_SANITIZED_ROOT_MARKER} Primary"),
        "Primary".to_string(),
    ] {
        let groups = [TallyNamedMaster {
            name: "Sundry Debtors".to_string(),
            parent: crate::PartyLedgerMasterFieldObservation::Returned(root),
            reserved_name: Some("Sundry Debtors".to_string()),
        }];
        let ledgers = [LedgerSnapshotEntry {
            name: "Synthetic Ledger".to_string(),
            parent: Some("Sundry Debtors".to_string()),
            closing_balance: Some(ExactDecimal::zero()),
            opening_balance: ExactDecimal::zero(),
            bill_wise_on: false,
        }];
        compute_residuals(&[], &[], &ledgers, NativeGroupSnapshot::Complete(&groups))
            .expect("shared reserved-root forms must terminate group ancestry");
    }

    let groups = [TallyNamedMaster {
        name: "Sundry Debtors".to_string(),
        parent: crate::PartyLedgerMasterFieldObservation::Returned("Primary".to_string()),
        reserved_name: Some("Sundry Debtors".to_string()),
    }];
    for parent in [
        format!("{TALLY_SANITIZED_ROOT_MARKER} Resave"),
        format!("{TALLY_SANITIZED_ROOT_MARKER}{TALLY_SANITIZED_ROOT_MARKER} Primary"),
    ] {
        let ledgers = [LedgerSnapshotEntry {
            name: "Synthetic Ledger".to_string(),
            parent: Some(parent),
            closing_balance: Some(ExactDecimal::zero()),
            opening_balance: ExactDecimal::zero(),
            bill_wise_on: false,
        }];

        assert!(matches!(
            compute_residuals(&[], &[], &ledgers, NativeGroupSnapshot::Complete(&groups)),
            Err(NativeOutstandingsError::InvalidResponse(
                "ledger_group_parent_unresolved"
            ))
        ));
    }
}
