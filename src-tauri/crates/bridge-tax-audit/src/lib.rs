//! Books-based tax-audit tests over a Tally read, ported from a Python reference engine one
//! test at a time, each proven equal to the reference by a canonical parity dump.
//!
//! This slice's end-to-end path: read a tally-read-v1 directory ([`read`], every byte verified
//! against its manifest), build only the book fields a test needs ([`book`]), evaluate the book
//! and result invariants ([`invariants`]), run a test ([`cash_44ab`], [`cash_payments_40a3`],
//! [`depreciation`], [`financial_statements`], [`applicability_44ab`], [`trial_balance`],
//! [`stale_balances_41_1`], [`ledger_scrutiny`], [`cash_book_integrity`]) with rule values from
//! a vendored rules excerpt ([`rules`]), and serialise the result canonically ([`canonical`]) so
//! [`compare`] can diff it against the reference engine's dump under the same rules the
//! reference's own comparer applies. A module with its own invariant (`depreciation`'s DEP-1/DEP-2,
//! `financial_statements`' FS-1/FS-2, `trial_balance`'s TB-1/TB-2, `stale_balances_41_1`'s STL-1,
//! `ledger_scrutiny`'s LSC-1, `cash_book_integrity`'s CBI-1/CBI-2) threads it through
//! [`canonical::canonical_test_result`]'s `module_check` parameter.
//!
//! Nothing here talks to Tally, and nothing here writes. The crate reads files a person (or,
//! later, Bridge) put on disk.
//!
//! **Parity evidence.** `tests/parity.rs`, `tests/parity_40a3.rs` and `tests/parity_depreciation.rs`
//! each compare this crate's dump over a committed synthetic read with the reference engine's dump
//! over the same bytes, and prove the comparison can fail. That is parity on invented data only.
//! The evidence that the slice reads real Tally books is `examples/local_parity.rs`, run on the
//! machine that holds client reads and never committed; each change to this crate should record
//! that run's result.
//!
//! **In CI.** The crate is a member of the `src-tauri` Cargo workspace, so the existing
//! workspace `cargo test`/`clippy`/`fmt` steps, and the dependency-inventory and licence gates,
//! already cover it; no crate-specific CI step is needed.

pub mod applicability_44ab;
pub mod binding;
pub mod book;
pub mod canonical;
pub mod cash_44ab;
pub mod cash_book_integrity;
pub mod cash_payments_40a3;
pub mod compare;
pub mod depreciation;
pub mod error;
pub mod financial_statements;
pub mod findings;
pub mod invariants;
pub mod ledger_ids;
pub mod ledger_scrutiny;
pub mod read;
pub mod registry;
pub mod rules;
pub mod stale_balances_41_1;
mod support;
pub mod trial_balance;
pub mod xml;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bridge_tally_primitives::TallyDate;

pub use error::{AuditError, Result};
use read::{CompanyPin, Read, Window};
use rules::Rules;

/// The engagement keys this slice reads, from a reference-engine client config: `[client]`
/// label and assessment year, `[period]`, `[snapshot]` naming a tally-read-v1 directory, and
/// the `[roles]` cash and bank groups. `round_off_ledgers` and `loan_ledgers_configured` are
/// `cash_payments_40a3`-only and are empty when the client config carries no `[roles]
/// .round_off_ledgers` key or no `[loans]` table at all -- the expected shape for a client with
/// nothing configured there, same convention the reference engine's own loaders use, not an
/// error.
#[derive(Debug, Clone)]
pub struct Engagement {
    pub label: String,
    pub assessment_year: String,
    pub period: Window,
    pub read_dir: PathBuf,
    pub allow_unbracketed_read: bool,
    /// `[client.tally]` `company_guid` + `books_from`: the company every read must come from
    /// (C5-client), or `None` when the config does not pin one.
    pub company_pin: Option<CompanyPin>,
    pub cash_groups: Vec<String>,
    pub bank_groups: Vec<String>,
    pub round_off_ledgers: Vec<String>,
    pub loan_ledgers_configured: Vec<String>,
    /// `cash_book_integrity`-only: the optional `[roles].own_account_narration_terms`, the forms
    /// in which the bank prints the assessee's own other account on transfer lines (client data).
    /// Kept as written and validated only when that test runs
    /// ([`cash_book_integrity::own_account_terms`]), so a malformed value fails that one test and
    /// not every test on the engagement -- the reference, too, reads the key only when it runs
    /// `cash_book_integrity`. `None` when the key is absent.
    pub own_account_narration_terms: Option<toml::Value>,
    /// `depreciation`-only: `None` when the client config carries no `[depreciation]` table at
    /// all (an engagement that never runs that test); `Some` once the table is present, at which
    /// point `block_by_ledger`, `opening_wdv_paise` and `dep_expense_ledgers` are REQUIRED within
    /// it (the reference engine's own `depreciation_config` raises on a missing key rather than
    /// defaulting it -- unlike `round_off_ledgers`/`loan_ledgers_configured` above, "nothing
    /// configured" is not a valid state for a client that has fixed assets at all).
    pub depreciation: Option<DepreciationConfig>,
    /// `financial_statements`-only: `[partners.<key>].interest_ledger`, keyed by partner key
    /// (`deed` is not a partner and is skipped, as the reference's `partners_config` pops it).
    /// Empty when the config has no `[partners]` table (e.g. a proprietorship).
    pub partner_interest_ledgers: BTreeMap<String, String>,
    /// `[client].entity_type` ("individual", "firm", ...); `applicability_44ab` reads it (the
    /// s.44ADA profession flag) and refuses without it. Optional for every other test.
    pub entity_type: Option<String>,
    /// `applicability_44ab`-only: the optional `[presumptive_history]` table, verbatim (client
    /// confirmation of s.44AD history, never inferred from the books); `None` when absent.
    pub presumptive_history: Option<toml::Table>,
    /// The parsed config, kept only so [`Engagement::bind`] can read `[ledger_ids]`/
    /// `[group_ids]` (`binding::bind`) without re-parsing the source text. Not part of this
    /// struct's public contract: a field a caller should read directly (`cash_groups` and the
    /// rest above) is exposed as its own field instead.
    raw_cfg: toml::Table,
}

/// `[depreciation]` from the client config: see [`Engagement::depreciation`].
#[derive(Debug, Clone, Default)]
pub struct DepreciationConfig {
    pub block_by_ledger: BTreeMap<String, String>,
    pub opening_wdv_paise: BTreeMap<String, i64>,
    pub dep_expense_ledgers: BTreeSet<String>,
    /// `[depreciation.put_to_use_by_voucher]`, guid -> date; optional, empty when absent (no
    /// client TOML configures this key today -- the reference engine's own `pack.py` never
    /// passes it either -- but `depreciation.py`'s `run()` signature carries it, so this port
    /// does too).
    pub put_to_use_by_voucher: BTreeMap<String, TallyDate>,
}

/// `[snapshot]` keys of the reference engine's legacy raw-export layout. A config naming a read
/// must not also name these (`CFG-mixed`).
const LEGACY_SNAPSHOT_KEYS: [&str; 10] = [
    "company",
    "groups",
    "ledgers",
    "trial_balance",
    "voucher_types",
    "voucher_dir",
    "voucher_glob",
    "status_side_list",
    "status_side_list_scope",
    "read_at",
];

impl Engagement {
    /// Parse a client config; `[snapshot].path` is relative to `base_dir`.
    pub fn from_toml(text: &str, base_dir: &Path) -> Result<Self> {
        let cfg: toml::Table = toml::from_str(text)
            .map_err(|e| AuditError::Config(format!("engagement TOML: {e}")))?;
        let table = |name: &str| {
            cfg.get(name)
                .and_then(toml::Value::as_table)
                .ok_or_else(|| AuditError::Config(format!("no [{name}] table")))
        };
        let string = |t: &toml::Table, key: &str| {
            t.get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| AuditError::Config(format!("{key} is not a string")))
        };
        let strings = |t: &toml::Table, key: &str| -> Result<Vec<String>> {
            t.get(key)
                .and_then(toml::Value::as_array)
                .ok_or_else(|| AuditError::Config(format!("{key} is not a list")))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| AuditError::Config(format!("{key} holds a non-string")))
                })
                .collect()
        };
        let date = |t: &toml::Table, key: &str| -> Result<TallyDate> {
            let s = string(t, key)?;
            TallyDate::parse(s.replace('-', ""))
                .ok()
                .filter(|_| s.len() == 10)
                .ok_or_else(|| {
                    AuditError::Config(format!("[period].{key} {s:?} is not YYYY-MM-DD"))
                })
        };
        let (client, period, snapshot, roles) = (
            table("client")?,
            table("period")?,
            table("snapshot")?,
            table("roles")?,
        );
        if snapshot.get("format").and_then(toml::Value::as_str) != Some("tally-read-v1") {
            return Err(AuditError::refused(
                "C1-format",
                "[snapshot].format is not \"tally-read-v1\"",
            ));
        }
        let mixed: Vec<&str> = LEGACY_SNAPSHOT_KEYS
            .into_iter()
            .filter(|k| snapshot.contains_key(*k))
            .collect();
        if !mixed.is_empty() {
            return Err(AuditError::refused(
                "CFG-mixed",
                format!("[snapshot] names a read and also legacy keys {mixed:?}"),
            ));
        }
        let path = snapshot
            .get("path")
            .and_then(toml::Value::as_str)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| AuditError::refused("CFG-path", "[snapshot].path is required"))?;
        let company_pin = client
            .get("tally")
            .map(|t| -> Result<CompanyPin> {
                let bad = || {
                    AuditError::refused(
                        "CFG-tally-pin",
                        "[client.tally] needs both company_guid and books_from (YYYY-MM-DD)",
                    )
                };
                let t = t.as_table().ok_or_else(bad)?;
                // Exact shapes, nothing stripped: the reference implementation applies the same
                // two checks, so both engines accept and refuse the same pins.
                let guid = t
                    .get("company_guid")
                    .and_then(toml::Value::as_str)
                    .filter(|g| read::is_uuid(g))
                    .ok_or_else(bad)?;
                let books_from = t
                    .get("books_from")
                    .and_then(toml::Value::as_str)
                    .filter(|s| {
                        let b = s.as_bytes();
                        b.len() == 10
                            && b[4] == b'-'
                            && b[7] == b'-'
                            && b.iter()
                                .enumerate()
                                .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
                    })
                    .and_then(|s| TallyDate::parse(s.replace('-', "")).ok())
                    .ok_or_else(bad)?;
                Ok(CompanyPin {
                    guid: guid.to_ascii_lowercase(),
                    books_from,
                })
            })
            .transpose()?;
        let depreciation = cfg
            .get("depreciation")
            .and_then(toml::Value::as_table)
            .map(|dep| -> Result<DepreciationConfig> {
                let string_map = |key: &str| -> Result<BTreeMap<String, String>> {
                    dep.get(key)
                        .and_then(toml::Value::as_table)
                        .ok_or_else(|| {
                            AuditError::Config(format!("[depreciation].{key} is not a table"))
                        })?
                        .iter()
                        .map(|(k, v)| {
                            v.as_str()
                                .map(|s| (k.clone(), s.to_string()))
                                .ok_or_else(|| {
                                    AuditError::Config(format!(
                                        "[depreciation].{key}.{k} is not a string"
                                    ))
                                })
                        })
                        .collect()
                };
                let int_map = |key: &str| -> Result<BTreeMap<String, i64>> {
                    dep.get(key)
                        .and_then(toml::Value::as_table)
                        .ok_or_else(|| {
                            AuditError::Config(format!("[depreciation].{key} is not a table"))
                        })?
                        .iter()
                        .map(|(k, v)| {
                            v.as_integer().map(|n| (k.clone(), n)).ok_or_else(|| {
                                AuditError::Config(format!(
                                    "[depreciation].{key}.{k} is not an integer"
                                ))
                            })
                        })
                        .collect()
                };
                let dep_expense_ledgers: BTreeSet<String> = dep
                    .get("dep_expense_ledgers")
                    .and_then(toml::Value::as_array)
                    .ok_or_else(|| {
                        AuditError::Config(
                            "[depreciation].dep_expense_ledgers is not a list".to_string(),
                        )
                    })?
                    .iter()
                    .map(|v| {
                        v.as_str().map(str::to_string).ok_or_else(|| {
                            AuditError::Config(
                                "[depreciation].dep_expense_ledgers holds a non-string".to_string(),
                            )
                        })
                    })
                    .collect::<Result<_>>()?;
                let put_to_use_by_voucher = dep
                    .get("put_to_use_by_voucher")
                    .and_then(toml::Value::as_table)
                    .map(|t| -> Result<BTreeMap<String, TallyDate>> {
                        t.iter()
                            .map(|(guid, v)| {
                                let s = v.as_str().ok_or_else(|| {
                                    AuditError::Config(format!(
                                        "[depreciation].put_to_use_by_voucher.{guid} is not a \
string"
                                    ))
                                })?;
                                let d = TallyDate::parse(s.replace('-', "")).map_err(|_| {
                                    AuditError::Config(format!(
                                        "[depreciation].put_to_use_by_voucher.{guid} {s:?} is \
not YYYY-MM-DD"
                                    ))
                                })?;
                                Ok((guid.clone(), d))
                            })
                            .collect()
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(DepreciationConfig {
                    block_by_ledger: string_map("block_by_ledger")?,
                    opening_wdv_paise: int_map("opening_wdv_paise")?,
                    dep_expense_ledgers,
                    put_to_use_by_voucher,
                })
            })
            .transpose()?;
        let mut partner_interest_ledgers = BTreeMap::new();
        if let Some(partners) = cfg.get("partners") {
            let partners = partners
                .as_table()
                .ok_or_else(|| AuditError::Config("[partners] is not a table".to_string()))?;
            for (key, partner) in partners.iter().filter(|(k, _)| k.as_str() != "deed") {
                let partner = partner.as_table().ok_or_else(|| {
                    AuditError::Config(format!("[partners].{key} is not a table"))
                })?;
                match partner.get("interest_ledger") {
                    None => {}
                    Some(v) => {
                        let s = v.as_str().ok_or_else(|| {
                            AuditError::Config(format!(
                                "[partners].{key}.interest_ledger is not a string"
                            ))
                        })?;
                        // The reference keeps only a truthy interest_ledger.
                        if !s.is_empty() {
                            partner_interest_ledgers.insert(key.clone(), s.to_string());
                        }
                    }
                }
            }
        }
        Ok(Self {
            label: string(client, "label")?,
            assessment_year: string(client, "assessment_year")?,
            period: Window {
                from: date(period, "start")?,
                to: date(period, "end")?,
            },
            read_dir: base_dir.join(path),
            allow_unbracketed_read: snapshot
                .get("allow_unbracketed_read")
                .and_then(toml::Value::as_bool)
                .unwrap_or(false),
            company_pin,
            cash_groups: strings(roles, "cash_groups")?,
            bank_groups: strings(roles, "bank_groups")?,
            round_off_ledgers: match roles.get("round_off_ledgers") {
                Some(_) => strings(roles, "round_off_ledgers")?,
                None => Vec::new(),
            },
            own_account_narration_terms: roles.get("own_account_narration_terms").cloned(),
            loan_ledgers_configured: cfg
                .get("loans")
                .and_then(toml::Value::as_table)
                .and_then(|loans| loans.get("loan_ledgers"))
                .and_then(toml::Value::as_table)
                .map(|table| table.keys().cloned().collect())
                .unwrap_or_default(),
            depreciation,
            partner_interest_ledgers,
            entity_type: client
                .get("entity_type")
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        AuditError::Config("[client].entity_type is not a string".to_string())
                    })
                })
                .transpose()?,
            presumptive_history: cfg
                .get("presumptive_history")
                .map(|v| {
                    v.as_table().cloned().ok_or_else(|| {
                        AuditError::Config("[presumptive_history] is not a table".to_string())
                    })
                })
                .transpose()?,
            raw_cfg: cfg,
        })
    }

    /// Bind this engagement's ledger and group names to `book` by Tally identity, or refuse.
    /// See [`binding`] for the rule; every location `cash_44ab`, `cash_payments_40a3` and
    /// `depreciation` read from this struct is bound. Called once, before a test runs, mirroring
    /// the reference implementation's `run.load()` calling `bind_config()` once.
    pub fn bind(&self, book: &book::Book) -> Result<(Self, binding::BindingReport)> {
        binding::bind(self, book)
    }
}

/// The vendored rules, for the one assessment year this slice carries.
pub fn rules_for(engagement: &Engagement) -> Result<Rules> {
    if engagement.assessment_year != "2026-27" {
        return Err(AuditError::Config(format!(
            "no vendored rules for AY {}",
            engagement.assessment_year
        )));
    }
    Rules::vendored()
}

/// Read and verify the engagement's read, and build its book (C1-C10).
pub fn load_book(engagement: &Engagement) -> Result<book::Book> {
    let read = Read::open(&engagement.read_dir)?;
    read.check(
        &engagement.period,
        engagement.allow_unbracketed_read,
        engagement.company_pin.as_ref(),
    )?;
    book::load_book(&read, &engagement.label)
}

/// Run `cash_44ab` on a book and return its canonical parity dump.
pub fn cash_44ab_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let cash = book.ledgers_under_any(&engagement.cash_groups);
    let bank = book.ledgers_under_any(&engagement.bank_groups);
    let result = cash_44ab::run(book, rules, &cash, &bank)?;
    canonical::canonical_test_result(book, &result, None)
}

/// Read, verify, build the book, run `cash_44ab` and return its canonical parity dump.
pub fn cash_44ab_canonical(engagement: &Engagement, rules: &Rules) -> Result<serde_json::Value> {
    cash_44ab_on(engagement, &load_book(engagement)?, rules)
}

/// Run `cash_payments_40a3` on a book and return its canonical parity dump.
pub fn cash_payments_40a3_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let cash = book.ledgers_under_any(&engagement.cash_groups);
    let bank = book.ledgers_under_any(&engagement.bank_groups);
    let loan_ledgers_configured: BTreeSet<String> =
        engagement.loan_ledgers_configured.iter().cloned().collect();
    let round_off_ledgers: BTreeSet<String> =
        engagement.round_off_ledgers.iter().cloned().collect();
    let result = cash_payments_40a3::run(
        book,
        rules,
        &cash,
        &bank,
        &loan_ledgers_configured,
        &round_off_ledgers,
    )?;
    canonical::canonical_test_result(book, &result, None)
}

/// Read, verify, build the book, run `cash_payments_40a3` and return its canonical parity dump.
pub fn cash_payments_40a3_canonical(
    engagement: &Engagement,
    rules: &Rules,
) -> Result<serde_json::Value> {
    cash_payments_40a3_on(engagement, &load_book(engagement)?, rules)
}

/// Run `depreciation` on a book and return its canonical parity dump. Refuses with
/// `AuditError::Config` if the engagement carries no `[depreciation]` table, mirroring the
/// reference engine's own `depreciation_config`/`require` failing loud on a client config that
/// never named its Fixed Assets blocks (see [`Engagement::depreciation`]).
pub fn depreciation_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let dep = engagement.depreciation.as_ref().ok_or_else(|| {
        AuditError::Config(
            "client config missing required key 'depreciation' (no [depreciation] table)"
                .to_string(),
        )
    })?;
    let result = depreciation::run(
        book,
        rules,
        &engagement.period,
        &dep.block_by_ledger,
        &dep.opening_wdv_paise,
        &dep.dep_expense_ledgers,
        &dep.put_to_use_by_voucher,
    )?;
    let module_check = depreciation::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Read, verify, build the book, run `depreciation` and return its canonical parity dump.
pub fn depreciation_canonical(engagement: &Engagement, rules: &Rules) -> Result<serde_json::Value> {
    depreciation_on(engagement, &load_book(engagement)?, rules)
}

/// Run `trial_balance` on a book and return its canonical parity dump, with the module's own
/// TB-1/TB-2 check. Reads no engagement configuration beyond binding.
pub fn trial_balance_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (_engagement, _report) = engagement.bind(book)?;
    let result = trial_balance::run(book, rules)?;
    let module_check = trial_balance::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Run `cash_book_integrity` on a book and return its canonical parity dump, with the module's
/// own CBI-1/CBI-2 check. Cash and bank are the engagement's cash and bank groups; the
/// own-account narration terms are the optional `[roles]` key, as the reference's pack passes
/// them.
pub fn cash_book_integrity_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let cash = book.ledgers_under_any(&engagement.cash_groups);
    let bank = book.ledgers_under_any(&engagement.bank_groups);
    let terms =
        cash_book_integrity::own_account_terms(engagement.own_account_narration_terms.as_ref())?;
    let result = cash_book_integrity::run(book, rules, &cash, &bank, &terms)?;
    let module_check = cash_book_integrity::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Read, verify, build the book, run `cash_book_integrity` and return its canonical parity dump.
pub fn cash_book_integrity_canonical(
    engagement: &Engagement,
    rules: &Rules,
) -> Result<serde_json::Value> {
    cash_book_integrity_on(engagement, &load_book(engagement)?, rules)
}

/// Run `ledger_scrutiny` on a book and return its canonical parity dump, with the module's own
/// LSC-1 check. The cash ledgers are the engagement's cash groups, as the reference's pack passes
/// them; the last-days window ends at the engagement period's end.
pub fn ledger_scrutiny_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let cash = book.ledgers_under_any(&engagement.cash_groups);
    let result = ledger_scrutiny::run(book, rules, &engagement.period, &cash)?;
    let module_check = ledger_scrutiny::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Read, verify, build the book, run `ledger_scrutiny` and return its canonical parity dump.
pub fn ledger_scrutiny_canonical(
    engagement: &Engagement,
    rules: &Rules,
) -> Result<serde_json::Value> {
    ledger_scrutiny_on(engagement, &load_book(engagement)?, rules)
}

/// Run `stale_balances_41_1` on a book and return its canonical parity dump, with the module's
/// own STL-1 check. Reads no engagement configuration beyond binding.
pub fn stale_balances_41_1_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (_engagement, _report) = engagement.bind(book)?;
    let result = stale_balances_41_1::run(book, rules)?;
    let module_check = stale_balances_41_1::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Read, verify, build the book, run `stale_balances_41_1` and return its canonical parity dump.
pub fn stale_balances_41_1_canonical(
    engagement: &Engagement,
    rules: &Rules,
) -> Result<serde_json::Value> {
    stale_balances_41_1_on(engagement, &load_book(engagement)?, rules)
}

/// Read, verify, build the book, run `trial_balance` and return its canonical parity dump.
pub fn trial_balance_canonical(
    engagement: &Engagement,
    rules: &Rules,
) -> Result<serde_json::Value> {
    trial_balance_on(engagement, &load_book(engagement)?, rules)
}

/// Run `financial_statements` on a book and return its canonical parity dump. `report_totals` is
/// caller data (Tally's own Profit & Loss report); `None` gives the reference's "no report" result.
pub fn financial_statements_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
    report_totals: Option<&financial_statements::ReportTotals>,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let interest: BTreeSet<String> = engagement
        .partner_interest_ledgers
        .values()
        .cloned()
        .collect();
    let result = financial_statements::run(book, rules, &interest, report_totals)?;
    let module_check = financial_statements::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Read, verify, build the book, run `financial_statements` and return its canonical parity dump.
pub fn financial_statements_canonical(
    engagement: &Engagement,
    rules: &Rules,
    report_totals: Option<&financial_statements::ReportTotals>,
) -> Result<serde_json::Value> {
    financial_statements_on(engagement, &load_book(engagement)?, rules, report_totals)
}

/// Run `applicability_44ab` on a book and return its canonical parity dump. As the reference's own
/// pack does, books turnover is `financial_statements`' `sales` figure and the cash share is
/// `cash_44ab`'s own two figures and finding limits, both run here on the same bound engagement.
/// `comparisons` carries the GSTR-1/GSTR-3B/AIS turnover (caller data: this crate reads no GST
/// document); only its `gstr1`/`gstr3b`/`ais` fields are read. Refuses without
/// `[client].entity_type`.
pub fn applicability_44ab_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
    comparisons: &applicability_44ab::TurnoverInputs,
) -> Result<serde_json::Value> {
    use findings::Value;
    let entity_type = engagement.entity_type.clone().ok_or_else(|| {
        AuditError::Config("applicability_44ab needs [client].entity_type".to_string())
    })?;
    let (bound, _report) = engagement.bind(book)?;
    let interest: BTreeSet<String> = bound.partner_interest_ledgers.values().cloned().collect();
    let fs = financial_statements::run(book, rules, &interest, None)?;
    let int_of = |r: &findings::TestResult, id: &str| -> Option<i64> {
        r.figures
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| match f.value {
                Value::Int(n) => Some(n),
                _ => None,
            })
    };
    let cash = book.ledgers_under_any(&bound.cash_groups);
    let bank = book.ledgers_under_any(&bound.bank_groups);
    let c44 = cash_44ab::run(book, rules, &cash, &bank)?;
    let inputs = applicability_44ab::TurnoverInputs {
        books_turnover_paise: int_of(&fs, "financial_statements.sales"),
        ..comparisons.clone()
    };
    let cash_share = applicability_44ab::CashShare {
        receipts_bp: int_of(&c44, "cash_44ab.cash_share_receipts"),
        payments_bp: int_of(&c44, "cash_44ab.cash_share_payments"),
        limits: c44
            .findings
            .first()
            .map(|f| f.limits.clone())
            .unwrap_or_default(),
    };
    let result = applicability_44ab::run(
        rules,
        &entity_type,
        &inputs,
        &cash_share,
        bound.presumptive_history.as_ref(),
    )?;
    canonical::canonical_test_result(book, &result, None)
}

/// Read, verify, build the book, run `applicability_44ab` and return its canonical parity dump.
pub fn applicability_44ab_canonical(
    engagement: &Engagement,
    rules: &Rules,
    comparisons: &applicability_44ab::TurnoverInputs,
) -> Result<serde_json::Value> {
    applicability_44ab_on(engagement, &load_book(engagement)?, rules, comparisons)
}
