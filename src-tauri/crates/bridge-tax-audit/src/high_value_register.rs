// SPDX-License-Identifier: Apache-2.0
//! Port of the reference engine's `high_value_register`: one register of high-value receipts and
//! payments, in cash and by bank, at (party, day) and (party, voucher) grain, with the s.269ST
//! limbs the books can show, party-to-party journal transfers, and s.194N's informational
//! exposure from a supplied bank statement.
//!
//! * Cash rows are tested against s.269ST(a)'s person-per-day limit (`>=`, the Act's "or more");
//!   bank rows against the CA-set vouching threshold only (s.269ST is cash-only). Findings are at
//!   (party, day) grain; the voucher grain is a companion count and total.
//! * A voucher whose money leg has no identifiable party (only Sales/Purchase Accounts, Duties &
//!   Taxes or round-off lines) is one `UNIDENTIFIED_PARTY` row, cited as a `row`, never a ledger.
//! * A cash receipt against Loans (Liability) is left out entirely (a s.269SS transaction), and in
//!   cash mode so is a counterparty whose configured type is a bank, a co-operative bank or a
//!   Government company (the form's own parenthetical).
//! * Limb (b) groups cash rows by (party, `Voucher.reference`) across dates, a JUDGEMENT candidate;
//!   limb (c) is one fixed question the books cannot answer.
//! * s.194N reports the statement window's narration-matched cash withdrawals, informational, and
//!   states which threshold applies only when the recipient type is known.
//!
//! The reference reads `[high_value_register].ca_threshold_paise` and `[s194n]` when the rules
//! carry them and its own defaults otherwise. The vendored rules excerpt carries neither table, so
//! this port reads the defaults, whose numbers equal the reference's tables (a test pins them).
//!
//! The row walk generalises `cash_payments_40a3`'s s.269ST walk over a money set, a direction and
//! a grain. A figure id the reference would repeat (two ledgers sharing a tag, a two-line journal
//! on one ledger) is refused with an error, as the reference's `fig` raises, never a panic.

use std::collections::{BTreeMap, BTreeSet};

use crate::book::Book;
use crate::documents::{AisRow, BankStatementDoc};
use crate::error::{AuditError, Result};
use crate::findings::TestResult;
use crate::loans_interest::LoanConfig;
use crate::rules::Rules;

pub const TEST_ID: &str = "high_value_register";
pub const VERSION: &str = "1";

pub const RECIPIENT_CO_OPERATIVE: &str = "co_operative_society";
pub const RECIPIENT_NOT_CO_OPERATIVE: &str = "not_co_operative_society";

/// The reference's fallback defaults, equal to its rules tables' numbers.
pub const DEFAULT_CA_THRESHOLD_PAISE: i64 = 2_00_000_00;
pub const DEFAULT_S194N_THRESHOLD_PAISE: i64 = 1_00_00_000_00;
pub const DEFAULT_S194N_THRESHOLD_CO_OPERATIVE_PAISE: i64 = 3_00_00_000_00;
pub const DEFAULT_S194N_THRESHOLD_NON_FILER_PAISE: i64 = 20_00_000_00;

/// Failing first: the configuration readers, the recipient type and a `run` that are not ported yet.
pub fn s194n_terms(_raw: Option<&toml::Value>) -> Result<BTreeSet<String>> {
    Ok(BTreeSet::new())
}

pub fn counterparty_types(
    _loans: &BTreeMap<String, LoanConfig>,
    _by_ledger: &BTreeMap<String, toml::Value>,
) -> Result<BTreeMap<String, String>> {
    Ok(BTreeMap::new())
}

pub fn s194n_recipient_type(_entity_type: Option<&str>) -> Option<&'static str> {
    None
}

/// The caller's inputs, as `tae/pack.py` passes them.
pub struct Inputs<'c> {
    pub cash: &'c BTreeSet<String>,
    pub bank: &'c BTreeSet<String>,
    /// `None`: `DEFAULT_CA_THRESHOLD_PAISE`, equal to the reference rules table's number.
    pub threshold_paise: Option<i64>,
    pub bank_statement: Option<&'c BankStatementDoc>,
    pub s194n_narration_terms: &'c BTreeSet<String>,
    pub ais_rows: &'c [AisRow],
    pub s194n_recipient_type: Option<&'c str>,
    pub round_off_ledgers: &'c BTreeSet<String>,
    pub counterparty_type_by_ledger: &'c BTreeMap<String, String>,
}

pub fn run(_book: &Book, _rules: &Rules, _i: &Inputs<'_>) -> Result<TestResult> {
    Err(AuditError::Config(format!("{TEST_ID}: not ported yet")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_reference_tables_numbers() {
        // The reference's rules/ay2026-27.toml at 1038dc05: [high_value_register].ca_threshold_paise
        // and [s194n]'s three thresholds; run() prefers those tables, and they equal its defaults.
        assert_eq!(DEFAULT_CA_THRESHOLD_PAISE, 2_00_000_00);
        assert_eq!(DEFAULT_S194N_THRESHOLD_PAISE, 1_00_00_000_00);
        assert_eq!(DEFAULT_S194N_THRESHOLD_CO_OPERATIVE_PAISE, 3_00_00_000_00);
        assert_eq!(DEFAULT_S194N_THRESHOLD_NON_FILER_PAISE, 20_00_000_00);
    }

    #[test]
    fn the_recipient_type_follows_pack_py() {
        for e in ["individual", "huf", "firm", "llp", "company"] {
            assert_eq!(
                s194n_recipient_type(Some(e)),
                Some(RECIPIENT_NOT_CO_OPERATIVE),
                "{e}"
            );
        }
        assert_eq!(
            s194n_recipient_type(Some("cooperative_society")),
            Some(RECIPIENT_CO_OPERATIVE)
        );
        assert_eq!(s194n_recipient_type(Some("trust")), None);
        assert_eq!(s194n_recipient_type(None), None);
    }

    #[test]
    fn config_values_are_typed_or_refused() {
        let loan = |t: &str| LoanConfig {
            lender: "L".to_string(),
            lender_type: t.to_string(),
            interest_ledger: None,
        };
        let loans = BTreeMap::from([
            ("Loan A".to_string(), loan("bank")),
            ("Loan B".to_string(), loan("relative")),
        ]);
        // The roles map overrides a loan's own lender_type and adds ledgers of its own.
        let roles = BTreeMap::from([
            ("Loan A".to_string(), toml::Value::from("individual")),
            (
                "Gov Co".to_string(),
                toml::Value::from("government_company"),
            ),
        ]);
        let want: BTreeMap<String, String> = [
            ("Gov Co", "government_company"),
            ("Loan A", "individual"),
            ("Loan B", "relative"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(counterparty_types(&loans, &roles).unwrap(), want);
        let bad = BTreeMap::from([("Gov Co".to_string(), toml::Value::from(1))]);
        assert!(counterparty_types(&loans, &bad).is_err());

        assert!(s194n_terms(None).unwrap().is_empty());
        let terms = toml::Value::Array(vec!["ATW-".into(), "NWD-".into()]);
        assert_eq!(s194n_terms(Some(&terms)).unwrap().len(), 2);
        for v in [
            toml::Value::from("ATW-"),
            toml::Value::Array(vec![1.into()]),
        ] {
            assert!(s194n_terms(Some(&v)).is_err(), "{v}");
        }
    }
}
