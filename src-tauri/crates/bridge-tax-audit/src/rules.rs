//! Rule values as data, read from the vendored excerpt of the reference Python implementation's
//! rules table.
//!
//! Provenance: `rules/ay2026-27.s44ab.toml` holds byte-for-byte verbatim blocks of the
//! reference implementation's own AY 2026-27 rules file -- `[meta]` through the end of `[s44ab]`,
//! then `[s40a3]` in full, then the first three lines of `[s269st]`, then `[s269ss_269t]`'s first
//! three lines and its two lender-type lists with their status lines,
//! then `[depreciation]` in full with its three `[depreciation.blocks.<key>]` sub-tables, then
//! `[due_dates]` as three blocks (header, the three dates, `status`), then `[ledger_scrutiny]` in
//! full, then `[s194c]`, `[s194i]` and `[deductor]` in full and `[s194j]` as three blocks
//! (header, its three value lines, `status`), then `[s43b_h]` in full, then `[s43b]` and
//! `[s36_1_va]` as blocks cut clear of their comments, then `[s194a]` with `status` cut at its value,
//! then `[s194t]`, `[s201_1a]`, `[s206c_7]` and `[tds_rates]` as blocks cut clear of their comments
//! and the four `[entity.<type>]` tables in full --
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
    "d56eeaf270501bf0dbb012ea9f26530f259e24840bf5c324cd24c5b8ea0485be";
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
    /// `[s43b_h]`: (`msme_days_with_agreement`, `msme_days_without_agreement`). `None` when the
    /// rules file has no `[s43b_h]` table; `creditor_ageing_43bh` then refuses, as the
    /// reference's `rules["s43b_h"]` raises.
    pub s43b_h_msme_days: Option<(i64, i64)>,
    /// `[s43b]`: (`authority`, `status`), quoted in `statutory_dues_43b`'s findings. `None` when
    /// the table is absent; that test then uses the reference's own prototype default text.
    pub s43b: Option<(String, String)>,
    /// `[s36_1_va].due_day`. `None` when the table is absent (the reference then defaults to 15);
    /// a table without the key is refused, as the reference's lookup raises.
    pub s36_1_va_due_day: Option<i64>,
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
    /// `loans_interest`: `[s194a]`, `None` when the rules carry no such table (the test then
    /// refuses, as the reference's `rules["s194a"]` raises).
    pub s194a: Option<S194a>,
    /// `loans_interest`: `[s269ss_269t].exempt_lender_types`, the wider s.269SS/s.269T breach
    /// exemption. `None` when the key is absent; the test then uses its own default, as the
    /// reference's `.get(..., DEFAULT)` does.
    pub s269ss_269t_exempt_lender_types: Option<Vec<String>>,
    /// `loans_interest`: `[s269ss_269t].reporting_exempt_lender_types`, the narrower Clause 31
    /// reporting exemption. `None` when absent, defaulted by the test as above.
    pub s269ss_269t_reporting_exempt_lender_types: Option<Vec<String>>,
    /// `partners_40b_194t`: `[s194t]`. `None` without the table; the test then uses its own
    /// prototype default and says so, as the reference does.
    pub s194t: Option<S194t>,
    /// `tds_interest_201`: `[s201_1a]`, `None` without it (the test's own default, flagged).
    pub s201_1a: Option<S2011a>,
    /// `tds_interest_201`: `[s206c_7]`, `None` without it (the test's own default, flagged).
    pub s206c_7: Option<S206c7>,
    /// `tds_interest_201`'s defaults: `[tds_rates]`. `None` without it; building the defaults
    /// then refuses, as the reference's `rules["tds_rates"]` raises.
    pub tds_rates: Option<TdsRates>,
    /// `[entity.<type>]`, keyed by entity type. `None` when the rules carry no `[entity]` table
    /// at all: every lookup then refuses, as the reference's `self["entity"]` raises.
    pub entity: Option<BTreeMap<String, EntityRules>>,
}

/// `[s194t]`: TDS on payments to partners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct S194t {
    pub rate_bp: i64,
    pub limit_paise: i64,
}

/// `[s201_1a]`: interest on TDS not deducted, or deducted and not paid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S2011a {
    pub rate_before_deduction_bp: i64,
    pub rate_after_deduction_bp: i64,
    pub authority: String,
    pub status: String,
}

/// `[s206c_7]`: interest on TCS not collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S206c7 {
    pub rate_bp: i64,
    pub authority: String,
    pub status: String,
}

/// `[tds_rates]`: the lower and higher statutory sub-rate per section, both used where the
/// payee's own type is not known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TdsRates {
    pub s194c_individual_huf_bp: i64,
    pub s194c_other_bp: i64,
    pub s194i_land_building_bp: i64,
    pub s194i_plant_machinery_bp: i64,
    pub s194j_professional_bp: i64,
    pub s194j_technical_bp: i64,
}

/// One `[entity.<type>]` table: the keys a ported test reads, each absent when the table omits
/// it. A value of the wrong type is refused at parse rather than read by truthiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityRules {
    pub s40b_interest_rate_bp: Option<i64>,
    pub s194t: Option<bool>,
}

/// `[s194a]`: the lender types exempt from s.194A and the threshold for payers other than a bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S194a {
    pub exempt_lender_types: Vec<String>,
    pub threshold_other_than_securities_paise: i64,
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
        let str_in = |t: &toml::Table, table_name: &str, key: &str| -> Result<String> {
            t.get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    AuditError::Config(format!("rules: [{table_name}].{key} is not a string"))
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
            s43b_h_msme_days: match table.get("s43b_h").and_then(toml::Value::as_table) {
                Some(t) => Some((
                    int_in(t, "s43b_h", "msme_days_with_agreement")?,
                    int_in(t, "s43b_h", "msme_days_without_agreement")?,
                )),
                None => None,
            },
            s43b: match table.get("s43b").and_then(toml::Value::as_table) {
                Some(t) => {
                    let text = |key: &str| {
                        t.get(key)
                            .and_then(toml::Value::as_str)
                            .map(str::to_string)
                            .ok_or_else(|| {
                                AuditError::Config(format!("rules: [s43b].{key} is not a string"))
                            })
                    };
                    Some((text("authority")?, text("status")?))
                }
                None => None,
            },
            s36_1_va_due_day: match table.get("s36_1_va").and_then(toml::Value::as_table) {
                Some(t) => Some(int_in(t, "s36_1_va", "due_day")?),
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
            s194a: match optional("s194a") {
                Some(t) => Some(S194a {
                    exempt_lender_types: strings_in(t, "s194a", "exempt_lender_types")?,
                    threshold_other_than_securities_paise: int_in(
                        t,
                        "s194a",
                        "threshold_other_than_securities_paise",
                    )?,
                }),
                None => None,
            },
            s269ss_269t_exempt_lender_types: s269ss_269t
                .contains_key("exempt_lender_types")
                .then(|| strings_in(s269ss_269t, "s269ss_269t", "exempt_lender_types"))
                .transpose()?,
            s269ss_269t_reporting_exempt_lender_types: s269ss_269t
                .contains_key("reporting_exempt_lender_types")
                .then(|| strings_in(s269ss_269t, "s269ss_269t", "reporting_exempt_lender_types"))
                .transpose()?,
            s194t: optional("s194t")
                .map(|t| -> Result<S194t> {
                    Ok(S194t {
                        rate_bp: int_in(t, "s194t", "rate_bp")?,
                        limit_paise: int_in(t, "s194t", "limit_paise")?,
                    })
                })
                .transpose()?,
            s201_1a: optional("s201_1a")
                .map(|t| -> Result<S2011a> {
                    Ok(S2011a {
                        rate_before_deduction_bp: int_in(t, "s201_1a", "rate_before_deduction_bp")?,
                        rate_after_deduction_bp: int_in(t, "s201_1a", "rate_after_deduction_bp")?,
                        authority: str_in(t, "s201_1a", "authority")?,
                        status: str_in(t, "s201_1a", "status")?,
                    })
                })
                .transpose()?,
            s206c_7: optional("s206c_7")
                .map(|t| -> Result<S206c7> {
                    Ok(S206c7 {
                        rate_bp: int_in(t, "s206c_7", "rate_bp")?,
                        authority: str_in(t, "s206c_7", "authority")?,
                        status: str_in(t, "s206c_7", "status")?,
                    })
                })
                .transpose()?,
            tds_rates: optional("tds_rates")
                .map(|t| -> Result<TdsRates> {
                    Ok(TdsRates {
                        s194c_individual_huf_bp: int_in(t, "tds_rates", "s194c_individual_huf_bp")?,
                        s194c_other_bp: int_in(t, "tds_rates", "s194c_other_bp")?,
                        s194i_land_building_bp: int_in(t, "tds_rates", "s194i_land_building_bp")?,
                        s194i_plant_machinery_bp: int_in(
                            t,
                            "tds_rates",
                            "s194i_plant_machinery_bp",
                        )?,
                        s194j_professional_bp: int_in(t, "tds_rates", "s194j_professional_bp")?,
                        s194j_technical_bp: int_in(t, "tds_rates", "s194j_technical_bp")?,
                    })
                })
                .transpose()?,
            entity: optional("entity")
                .map(|t| -> Result<BTreeMap<String, EntityRules>> {
                    t.iter()
                        .map(|(key, value)| {
                            let name = format!("entity.{key}");
                            let e = value.as_table().ok_or_else(|| {
                                AuditError::Config(format!("rules: [{name}] is not a table"))
                            })?;
                            let s40b = e
                                .contains_key("s40b_interest_rate_bp")
                                .then(|| int_in(e, &name, "s40b_interest_rate_bp"))
                                .transpose()?;
                            let s194t = e
                                .get("s194t")
                                .map(|v| {
                                    v.as_bool().ok_or_else(|| {
                                        AuditError::Config(format!(
                                            "rules: [{name}].s194t is not a boolean"
                                        ))
                                    })
                                })
                                .transpose()?;
                            Ok((
                                key.clone(),
                                EntityRules {
                                    s40b_interest_rate_bp: s40b,
                                    s194t,
                                },
                            ))
                        })
                        .collect()
                })
                .transpose()?,
        })
    }

    /// The entity type's table, or none when the rules have no table for it. Refuses when the
    /// rules carry no `[entity]` table at all.
    fn entity_rules(&self, entity_type: &str) -> Result<Option<&EntityRules>> {
        self.entity
            .as_ref()
            .map(|by_type| by_type.get(entity_type))
            .ok_or_else(|| AuditError::Config("rules: no [entity] table".to_string()))
    }

    /// The reference's `rules.entity("s40b_interest_rate_bp", 0) or 0`.
    pub fn s40b_interest_rate_bp(&self, entity_type: &str) -> Result<i64> {
        Ok(self
            .entity_rules(entity_type)?
            .and_then(|e| e.s40b_interest_rate_bp)
            .unwrap_or(0))
    }

    /// The reference's `bool(rules.entity("s194t", False))`.
    pub fn s194t_applies(&self, entity_type: &str) -> Result<bool> {
        Ok(self
            .entity_rules(entity_type)?
            .and_then(|e| e.s194t)
            .unwrap_or(false))
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
    fn vendored_rules_carry_the_s43b_h_limits() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.s43b_h_msme_days, Some((45, 15)));
    }

    #[test]
    fn an_s36_1_va_table_without_its_due_day_is_refused() {
        let text = VENDORED.replace("due_day = 15\n", "");
        assert!(text.contains("[s36_1_va]"));
        assert!(Rules::parse(&text).is_err());
    }

    #[test]
    fn vendored_rules_carry_the_statutory_dues_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.s36_1_va_due_day, Some(15));
        let (authority, status) = rules.s43b.unwrap();
        assert!(authority.starts_with("s.43B and its proviso"));
        assert_eq!(status, "confirm");
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

    #[test]
    fn vendored_rules_carry_the_loans_interest_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(
            rules.s194a,
            Some(S194a {
                exempt_lender_types: vec![
                    "bank".to_string(),
                    "cooperative_bank".to_string(),
                    "insurer".to_string()
                ],
                threshold_other_than_securities_paise: 1_000_000,
            })
        );
        assert_eq!(
            rules.s269ss_269t_exempt_lender_types.unwrap(),
            [
                "bank",
                "cooperative_bank",
                "government",
                "government_company",
                "statutory_corporation",
                "post_office_savings_bank",
                "notified_institution"
            ]
        );
        assert_eq!(
            rules.s269ss_269t_reporting_exempt_lender_types.unwrap(),
            [
                "government",
                "government_company",
                "bank",
                "statutory_corporation"
            ]
        );
    }

    /// The engine's own values (`load_rules("2026-27", ...)` at the reference commit).
    #[test]
    fn vendored_rules_carry_the_tds_interest_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(
            rules.s201_1a,
            Some(S2011a {
                rate_before_deduction_bp: 100,
                rate_after_deduction_bp: 150,
                authority: "s.201(1A)".to_string(),
                status: "VP".to_string(),
            })
        );
        assert_eq!(
            rules.s206c_7,
            Some(S206c7 {
                rate_bp: 100,
                authority: "s.206C(7)".to_string(),
                status: "partial".to_string(),
            })
        );
        assert_eq!(
            rules.tds_rates,
            Some(TdsRates {
                s194c_individual_huf_bp: 100,
                s194c_other_bp: 200,
                s194i_land_building_bp: 1000,
                s194i_plant_machinery_bp: 200,
                s194j_professional_bp: 1000,
                s194j_technical_bp: 200,
            })
        );
    }

    /// `rules.entity("s40b_interest_rate_bp", 0) or 0` and `bool(rules.entity("s194t", False))`,
    /// as the engine gives them for each entity type (huf has no `[entity.huf]` table).
    #[test]
    fn vendored_rules_carry_the_partners_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(
            rules.s194t,
            Some(S194t {
                rate_bp: 1000,
                limit_paise: 2_000_000,
            })
        );
        for (entity_type, s40b, s194t) in [
            ("firm", 1200, true),
            ("individual", 0, false),
            ("company", 0, false),
            ("llp", 1200, true),
            ("huf", 0, false),
        ] {
            assert_eq!(
                rules.s40b_interest_rate_bp(entity_type).unwrap(),
                s40b,
                "{entity_type}"
            );
            assert_eq!(
                rules.s194t_applies(entity_type).unwrap(),
                s194t,
                "{entity_type}"
            );
        }
    }

    /// The engine's `rules.entity(...)` indexes `self["entity"]`, which raises when the rules
    /// carry no `[entity]` table at all; an entity table's value of the wrong type is refused at
    /// parse, never read as truthy.
    #[test]
    fn entity_rules_fail_closed() {
        let without: String = VENDORED
            .split("\n\n")
            .filter(|block| !block.starts_with("[entity."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let rules = Rules::parse(&without).unwrap();
        assert!(rules.s40b_interest_rate_bp("firm").is_err());
        assert!(rules.s194t_applies("firm").is_err());
        let wrong = VENDORED.replace(
            "[entity.llp]\nform = \"3CB\"\ns40b_interest_rate_bp = 1200\ns194t = true",
            "[entity.llp]\ns194t = 1",
        );
        assert_ne!(wrong, VENDORED);
        assert!(Rules::parse(&wrong).is_err());
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
