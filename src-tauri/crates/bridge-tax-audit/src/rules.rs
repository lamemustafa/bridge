//! Rule values as data, read from the vendored excerpt of the reference Python implementation's
//! rules table.
//!
//! Provenance: `rules/ay2026-27.s44ab.toml` holds five byte-for-byte verbatim blocks of the
//! reference implementation's own AY 2026-27 rules file -- `[meta]` through the end of `[s44ab]`,
//! then `[s40a3]` in full, then the first three lines each of `[s269st]` and `[s269ss_269t]`,
//! then `[depreciation]` in full with its three `[depreciation.blocks.<key>]` sub-tables, then
//! `[due_dates]` as three blocks (header, the three dates, `status`), then `[ledger_scrutiny]` in
//! full --
//! under a header explaining why each block stops where it does (see the file itself). The
//! source file had sha256 [`SOURCE_SHA256`] when it was read at reference commit
//! [`SOURCE_COMMIT`]. The local parity example re-checks, against a local copy of the reference
//! implementation, that every block is still a verbatim substring of the live source and that
//! both files give the same values.
//!
//! [`VENDORED_SHA256`] is the vendored file's own hash; a unit test fails if the file changes
//! without that constant (and so without a reviewer seeing the provenance above) changing too.

use std::collections::BTreeMap;

use crate::error::{AuditError, Result};

pub const VENDORED: &str = include_str!("../rules/ay2026-27.s44ab.toml");
pub const VENDORED_SHA256: &str =
    "5e4af91263e8287f06a98c2c06536e303e4d3290fcc97b4d4bccc56ee1ee9614";
pub const SOURCE_PATH: &str = "the reference Python implementation's AY 2026-27 rules file";
pub const SOURCE_SHA256: &str = "8a6ec80cd5d19da34392e93024dc9a43a98982b09fb457c662f627174acedf2d";
pub const SOURCE_COMMIT: &str = "dd376ed014d922a1e2b12052763af36a565402ec";

/// The rule values the ported tests read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// `[meta].version`, echoed into every result as `rules_version`.
    pub version: String,
    pub turnover_threshold_paise: i64,
    pub turnover_threshold_low_cash_paise: i64,
    pub cash_share_limit_bp: i64,
    /// `[s40a3].limit_per_person_per_day_paise`.
    pub s40a3_limit_per_person_per_day_paise: i64,
    /// `[s40a3].goods_carriage_limit_paise`.
    pub s40a3_goods_carriage_limit_paise: i64,
    /// `[s40a3].excluded_group_roles`, in the order the source lists them.
    pub s40a3_excluded_group_roles: Vec<String>,
    /// `[s269st].limit_per_person_per_day_paise`.
    pub s269st_limit_per_person_per_day_paise: i64,
    /// `[s269ss_269t].limit_paise`.
    pub s269ss_269t_limit_paise: i64,
    /// `[depreciation].half_rate_days_threshold`.
    pub depreciation_half_rate_days_threshold: i64,
    /// `[depreciation].cash_addition_limit_paise`.
    pub depreciation_cash_addition_limit_paise: i64,
    /// `[depreciation.blocks.<key>].rate_bp`, keyed by block key.
    pub depreciation_block_rate_bp: BTreeMap<String, i64>,
    /// `[due_dates].audit_report`, ISO date.
    pub due_date_audit_report: String,
    /// `[due_dates].return_audit_case`, ISO date.
    pub due_date_return_audit_case: String,
    /// `[due_dates].return_non_audit_firm`, ISO date.
    pub due_date_return_non_audit_firm: String,
    /// `[due_dates].status` ("partial" until checked against the Finance Act text).
    pub due_dates_status: String,
    /// `[ledger_scrutiny].large_entry_paise`, a CA analytical materiality convention. `None`
    /// when the rules file has no `[ledger_scrutiny]` table: the reference's `ledger_scrutiny`
    /// then falls back to its own default, and no other test needs the table.
    pub ledger_scrutiny_large_entry_paise: Option<i64>,
    /// `tds_payees`: `[s194c]`, `None` when the rules carry no such table (the test then refuses,
    /// as the reference's `rules["s194c"]` raises).
    pub s194c: Option<S194c>,
    /// `tds_payees`: `[s194i].per_month_per_payee_paise`; `None` without `[s194i]`.
    pub s194i_per_month_per_payee_paise: Option<i64>,
    /// `tds_payees`: `[deductor].individual_huf_prev_year_turnover_paise`; `None` without
    /// `[deductor]`.
    pub deductor_individual_huf_prev_year_turnover_paise: Option<i64>,
    /// `tds_payees`: `[s194j].aggregate_paise`. `None` without `[s194j]`, which the test does not
    /// refuse: it falls back to its own default, as the reference does.
    pub s194j_aggregate_paise: Option<i64>,
}

/// `[s194c]`: the single-sum and aggregate limits of s.194C(5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S194c {
    pub single_sum_paise: i64,
    pub aggregate_paise: i64,
}

impl Rules {
    /// Parse a rules table (the vendored excerpt or the full source file).
    pub fn parse(text: &str) -> Result<Self> {
        let table: toml::Table =
            toml::from_str(text).map_err(|e| AuditError::Config(format!("rules TOML: {e}")))?;
        let section = |name: &str| {
            table
                .get(name)
                .and_then(toml::Value::as_table)
                .ok_or_else(|| AuditError::Config(format!("rules: no [{name}] table")))
        };
        let (meta, s44ab, s40a3, s269st, s269ss_269t, depreciation, due_dates) = (
            section("meta")?,
            section("s44ab")?,
            section("s40a3")?,
            section("s269st")?,
            section("s269ss_269t")?,
            section("depreciation")?,
            section("due_dates")?,
        );
        let date_in = |key: &str| -> Result<String> {
            match due_dates.get(key) {
                Some(toml::Value::Datetime(d)) if d.date.is_some() && d.time.is_none() => {
                    Ok(d.to_string())
                }
                _ => Err(AuditError::Config(format!(
                    "rules: [due_dates].{key} is not a date"
                ))),
            }
        };
        let optional = |name: &str| table.get(name).and_then(toml::Value::as_table);
        let int_in = |t: &toml::Table, table_name: &str, key: &str| {
            t.get(key).and_then(toml::Value::as_integer).ok_or_else(|| {
                AuditError::Config(format!("rules: [{table_name}].{key} is not an integer"))
            })
        };
        let strings_in = |t: &toml::Table, table_name: &str, key: &str| -> Result<Vec<String>> {
            t.get(key)
                .and_then(toml::Value::as_array)
                .ok_or_else(|| {
                    AuditError::Config(format!("rules: [{table_name}].{key} is not a list"))
                })?
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        AuditError::Config(format!(
                            "rules: [{table_name}].{key} holds a non-string"
                        ))
                    })
                })
                .collect()
        };
        Ok(Self {
            version: meta
                .get("version")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| AuditError::Config("rules: [meta].version".to_string()))?
                .to_string(),
            turnover_threshold_paise: int_in(s44ab, "s44ab", "turnover_threshold_paise")?,
            turnover_threshold_low_cash_paise: int_in(
                s44ab,
                "s44ab",
                "turnover_threshold_low_cash_paise",
            )?,
            cash_share_limit_bp: int_in(s44ab, "s44ab", "cash_share_limit_bp")?,
            s40a3_limit_per_person_per_day_paise: int_in(
                s40a3,
                "s40a3",
                "limit_per_person_per_day_paise",
            )?,
            s40a3_goods_carriage_limit_paise: int_in(s40a3, "s40a3", "goods_carriage_limit_paise")?,
            s40a3_excluded_group_roles: strings_in(s40a3, "s40a3", "excluded_group_roles")?,
            s269st_limit_per_person_per_day_paise: int_in(
                s269st,
                "s269st",
                "limit_per_person_per_day_paise",
            )?,
            s269ss_269t_limit_paise: int_in(s269ss_269t, "s269ss_269t", "limit_paise")?,
            depreciation_half_rate_days_threshold: int_in(
                depreciation,
                "depreciation",
                "half_rate_days_threshold",
            )?,
            depreciation_cash_addition_limit_paise: int_in(
                depreciation,
                "depreciation",
                "cash_addition_limit_paise",
            )?,
            depreciation_block_rate_bp: {
                let blocks = depreciation
                    .get("blocks")
                    .and_then(toml::Value::as_table)
                    .ok_or_else(|| {
                        AuditError::Config(
                            "rules: [depreciation.blocks] is not a table".to_string(),
                        )
                    })?;
                blocks
                    .iter()
                    .map(|(key, value)| {
                        let block = value.as_table().ok_or_else(|| {
                            AuditError::Config(format!(
                                "rules: [depreciation.blocks.{key}] is not a table"
                            ))
                        })?;
                        let rate_bp =
                            int_in(block, &format!("depreciation.blocks.{key}"), "rate_bp")?;
                        Ok((key.clone(), rate_bp))
                    })
                    .collect::<Result<BTreeMap<String, i64>>>()?
            },
            due_date_audit_report: date_in("audit_report")?,
            due_date_return_audit_case: date_in("return_audit_case")?,
            due_date_return_non_audit_firm: date_in("return_non_audit_firm")?,
            due_dates_status: due_dates
                .get("status")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| AuditError::Config("rules: [due_dates].status".to_string()))?
                .to_string(),
            ledger_scrutiny_large_entry_paise: match table
                .get("ledger_scrutiny")
                .and_then(toml::Value::as_table)
            {
                Some(ls) => Some(int_in(ls, "ledger_scrutiny", "large_entry_paise")?),
                None => None,
            },
            s194c: match optional("s194c") {
                Some(t) => Some(S194c {
                    single_sum_paise: int_in(t, "s194c", "single_sum_paise")?,
                    aggregate_paise: int_in(t, "s194c", "aggregate_paise")?,
                }),
                None => None,
            },
            s194i_per_month_per_payee_paise: optional("s194i")
                .map(|t| int_in(t, "s194i", "per_month_per_payee_paise"))
                .transpose()?,
            deductor_individual_huf_prev_year_turnover_paise: optional("deductor")
                .map(|t| int_in(t, "deductor", "individual_huf_prev_year_turnover_paise"))
                .transpose()?,
            s194j_aggregate_paise: optional("s194j")
                .map(|t| int_in(t, "s194j", "aggregate_paise"))
                .transpose()?,
        })
    }

    /// The vendored AY 2026-27 values.
    pub fn vendored() -> Result<Self> {
        Self::parse(VENDORED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn vendored_rules_match_their_recorded_hash() {
        let actual = crate::canonical::hex(&Sha256::digest(VENDORED.as_bytes()));
        assert_eq!(
            actual, VENDORED_SHA256,
            "rules/ay2026-27.s44ab.toml changed; re-vendor it from {SOURCE_PATH} and update \
             VENDORED_SHA256, SOURCE_SHA256 and SOURCE_COMMIT together"
        );
    }

    #[test]
    fn vendored_rules_carry_the_s44ab_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.version, "2026-09-17.1");
        assert_eq!(rules.turnover_threshold_paise, 1_000_000_000);
        assert_eq!(rules.turnover_threshold_low_cash_paise, 10_000_000_000);
        assert_eq!(rules.cash_share_limit_bp, 500);
    }

    #[test]
    fn vendored_rules_carry_the_cash_payments_40a3_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.s40a3_limit_per_person_per_day_paise, 1_000_000);
        assert_eq!(rules.s40a3_goods_carriage_limit_paise, 3_500_000);
        assert_eq!(
            rules.s40a3_excluded_group_roles,
            vec![
                "capital",
                "loans_liability",
                "loans_advances_asset",
                "fixed_assets",
                "duties_taxes",
            ]
        );
        assert_eq!(rules.s269st_limit_per_person_per_day_paise, 20_000_000);
        assert_eq!(rules.s269ss_269t_limit_paise, 2_000_000);
    }

    #[test]
    fn vendored_rules_carry_the_depreciation_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.depreciation_half_rate_days_threshold, 180);
        assert_eq!(rules.depreciation_cash_addition_limit_paise, 1_000_000);
        assert_eq!(
            rules.depreciation_block_rate_bp,
            [
                ("furniture_10".to_string(), 1000),
                ("plant_machinery_15".to_string(), 1500),
                ("computers_40".to_string(), 4000),
            ]
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>()
        );
    }

    #[test]
    fn vendored_rules_carry_the_due_dates() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.due_date_audit_report, "2026-09-30");
        assert_eq!(rules.due_date_return_audit_case, "2026-10-31");
        assert_eq!(rules.due_date_return_non_audit_firm, "2026-08-31");
        assert_eq!(rules.due_dates_status, "partial");
    }

    #[test]
    fn vendored_rules_carry_the_tds_payees_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(
            rules.s194c,
            Some(S194c {
                single_sum_paise: 3_000_000,
                aggregate_paise: 10_000_000,
            })
        );
        assert_eq!(rules.s194i_per_month_per_payee_paise, Some(5_000_000));
        assert_eq!(
            rules.deductor_individual_huf_prev_year_turnover_paise,
            Some(1_000_000_000)
        );
        assert_eq!(rules.s194j_aggregate_paise, Some(5_000_000));
    }

    /// The vendored excerpt is public: the source's own comment beside `return_non_audit_firm`
    /// is private, so the excerpt stops at the value, and no private-note citation may appear.
    #[test]
    fn the_vendored_excerpt_is_public_safe() {
        assert!(VENDORED
            .lines()
            .any(|l| l == "return_non_audit_firm = 2026-08-31"));
        for word in ["note)", "research/", ".md", "gap register"] {
            assert!(!VENDORED.contains(word), "{word:?} in the vendored rules");
        }
    }
}
