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
pub mod book_keeping_quality;
pub mod canonical;
pub mod cash_44ab;
pub mod cash_book_integrity;
pub mod cash_payments_40a3;
pub mod compare;
pub mod creditor_ageing_43bh;
pub mod depreciation;
pub mod documents;
pub mod error;
pub mod financial_statements;
pub mod findings;
pub mod invariants;
pub mod ledger_ids;
pub mod ledger_scrutiny;
pub mod loans_interest;
pub mod read;
pub mod registry;
pub mod rules;
pub mod stale_balances_41_1;
pub mod statutory_dues_43b;
mod support;
pub mod tds_payees;
pub mod tds_tcs_26as;
mod text_tables;
pub mod trial_balance;
pub mod twentysixas_receipts;
pub mod xml;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bridge_tally_primitives::TallyDate;

pub use error::{AuditError, Result};
use read::{CompanyPin, Read, Window};
use rules::Rules;
use support::PyIntError;

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
    /// `[roles].creditor_groups`, when present: the groups a `{ kind = "groups" }` trade-creditor
    /// source resolves. Filled by [`Engagement::bind`], which binds it like the cash and bank
    /// groups whenever it is present, as the reference binds every configured group; `None` on an
    /// engagement that has not been bound.
    pub creditor_groups: Option<Vec<String>>,
    /// `[roles].trade_creditors_source`, verbatim. Read only when `creditor_ageing_43bh` runs
    /// ([`trade_creditors`]); a `legacy_json` source's ledger names are read and bound by
    /// [`Engagement::bind`] for every test, as the reference's binding does, and the source is
    /// then replaced by `{ kind = "ledgers", ledgers = [...] }`. `None` when absent.
    pub trade_creditors_source: Option<toml::Value>,
    /// `loans_interest`-only: `[loans]`, filled by [`Engagement::bind`] ([`LoansConfig`]); empty on
    /// an engagement that has not been bound.
    pub loans: LoansConfig,
    /// `creditor_ageing_43bh`-only: the optional `[creditor_ageing_43bh]` table. Filled by
    /// [`Engagement::bind`]; see [`CreditorAgeingConfig`] for what is typed when.
    pub creditor_ageing: CreditorAgeingConfig,
    /// `statutory_dues_43b`-only: the optional `[statutory_dues]` table. Filled by
    /// [`Engagement::bind`]; see [`StatutoryDuesConfig`].
    pub statutory_dues: StatutoryDuesConfig,
    /// `tds_tcs_26as`/`twentysixas_receipts`-only: `[tds_tcs_26as]` ([`Tds26asConfig`]). `None`
    /// when the client config carries no such table. A value of the wrong type refuses reading the
    /// engagement, and so every test on it, as the reference's `bind_config` refuses it
    /// (`BIND-ID-MALFORMED`) whatever test runs. A MISSING key refuses only the two 26AS tests, as
    /// the reference's own `tds_tcs_26as_config` does when one of them asks for it: the key reads
    /// as empty here and `tds_tcs_26as_missing` names it.
    pub tds_tcs_26as: Option<Tds26asConfig>,
    /// The first of `[tds_tcs_26as]`'s four keys (in the reference's order) the table lacks.
    tds_tcs_26as_missing: Option<&'static str>,
    /// `tds_payees`-only: `[tds]` and `[tds_payees]` ([`TdsConfig`]). `None` when the client
    /// config carries no `[tds]` table (an engagement that never runs that test); once `[tds]` is
    /// present, `nature_by_ledger` and `payee_aliases` are REQUIRED within it, as the reference's
    /// own `tds_config` requires them. A malformed `[tds]` or `[tds_payees]` refuses reading the
    /// engagement, and so every test on it, where the reference refuses only `tds_payees` (its pack
    /// reads `[tds]` unconditionally, so a missing map refuses its whole pack too). `[tds_payees]`
    /// without `[tds]` is not read.
    pub tds: Option<TdsConfig>,
    /// `book_keeping_quality`-only: its `[roles]` name locations, bound. Filled by
    /// [`Engagement::bind`]; see [`BookKeepingQualityConfig`] for what is typed when.
    pub book_keeping_quality: BookKeepingQualityConfig,
    /// The parsed config, kept only so [`Engagement::bind`] can read `[ledger_ids]`/
    /// `[group_ids]` (`binding::bind`) without re-parsing the source text. Not part of this
    /// struct's public contract: a field a caller should read directly (`cash_groups` and the
    /// rest above) is exposed as its own field instead.
    raw_cfg: toml::Table,
    /// The directory `[snapshot].path` and a legacy trade-creditor source are relative to.
    base_dir: PathBuf,
}

/// `[loans]` from the client config, bound. Empty when the config has no `[loans]` table: the
/// reference's `loan_ledgers_config` then gives `{}`, which `loans_interest` takes as nothing to
/// report.
///
/// **Typed lazily**, as [`CreditorAgeingConfig`] is: [`Engagement::bind`] binds the three name
/// locations and refuses a malformed one (`BIND-ID-MALFORMED`); every other value is kept as
/// written and typed only when `loans_interest` runs.
#[derive(Debug, Clone, Default)]
pub struct LoansConfig {
    /// `[loans]` is present but is not a table: `loans_interest` refuses when it runs, as the
    /// reference's `cfg.get("loans", {}).get(...)` fails there.
    pub not_a_table: bool,
    /// `[loans.loan_ledgers]`, keyed by each loan ledger's bound name: its entry as written, with
    /// `interest_ledger` (when present) replaced by the bound name.
    pub loan_ledgers: BTreeMap<String, toml::Value>,
    /// `[loans].shared_interest_ledgers`, bound; empty when absent.
    pub shared_interest_ledgers: Vec<String>,
}

/// `[creditor_ageing_43bh]` from the client config, every key optional: the reference's
/// `creditor_ageing_config` defaults (0, {}, frozenset()) when a key or the whole table is
/// absent.
///
/// **Typed lazily** (the batch convention for a test's own table): [`Engagement::bind`] checks
/// only what binding reads, the SHAPE of the name locations (`supplier_classification` a table,
/// `mse_interest_ledgers` a list of names, refused `BIND-ID-MALFORMED` as the reference's binding
/// refuses them), and keeps every VALUE as written. The values are typed only when
/// `creditor_ageing_43bh` runs, so a malformed value there fails that test and no other, as in the
/// reference.
#[derive(Debug, Clone, Default)]
pub struct CreditorAgeingConfig {
    /// `[creditor_ageing_43bh]` is present but is not a table: the test refuses when it runs.
    pub not_a_table: bool,
    /// `acceptance_lag_days` as written; typed as the reference's `int()` types it.
    pub acceptance_lag_days: Option<toml::Value>,
    /// Bound ledger -> classification as written; checked against the list only for a creditor
    /// the test looks up, as the reference's `_classify` checks it.
    pub supplier_classification: BTreeMap<String, toml::Value>,
    pub mse_interest_ledgers: Vec<String>,
}

/// `[statutory_dues]` from the client config: `nature_by_ledger` (bound ledger -> nature as
/// written) and `salary_expense_ledgers`, both empty when absent, as the reference's
/// `statutory_dues_config` defaults them. Typed lazily, like [`CreditorAgeingConfig`].
#[derive(Debug, Clone, Default)]
pub struct StatutoryDuesConfig {
    /// `[statutory_dues]` is present but is not a table: the test refuses when it runs.
    pub not_a_table: bool,
    pub nature_by_ledger: BTreeMap<String, toml::Value>,
    pub salary_expense_ledgers: Vec<String>,
}

/// Python's `int(value)` for a TOML value, as the reference's `creditor_ageing_config` applies it:
/// an integer as itself, a boolean as 0 or 1, a finite float truncated toward zero, and a string
/// as `int()` reads one (`support::py_int_str`). Anything else is refused, as `int()` raises. A
/// value `int()` accepts but i64 cannot hold is refused with its own message: the reference would
/// carry the big integer on, and Bridge cannot.
pub(crate) fn py_int(v: &toml::Value, what: &str) -> Result<i64> {
    py_int_value(v).map_err(|e| {
        AuditError::Config(match e {
            PyIntError::Invalid => format!("{what}: int() cannot take {v}"),
            PyIntError::OutOfRange => {
                format!("{what}: int() gives {v}, outside the range Bridge holds (64-bit)")
            }
        })
    })
}

fn py_int_value(v: &toml::Value) -> std::result::Result<i64, PyIntError> {
    match v {
        toml::Value::Integer(n) => Ok(*n),
        toml::Value::Boolean(b) => Ok(i64::from(*b)),
        toml::Value::Float(f) if f.is_finite() => {
            let t = f.trunc();
            // i64 bounds as f64: -2^63 is exact, 2^63 is the first value past the top.
            if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&t) {
                #[allow(clippy::cast_possible_truncation)]
                Ok(t as i64)
            } else {
                Err(PyIntError::OutOfRange)
            }
        }
        toml::Value::String(s) => crate::support::py_int_str(s),
        _ => Err(PyIntError::Invalid),
    }
}

/// `book_keeping_quality`'s `[roles]` name locations: `payment_channel_debtors`,
/// `gst_payment_ledgers`, `writeoff_discount_ledgers` (lists of ledger names) and `tax_ledgers`
/// (a table of GST head -> ledger names), each `None` when the key is absent.
///
/// **Typed lazily**, the batch convention for a test's own inputs. [`Engagement::bind`] checks
/// and binds what the reference's binding checks and binds for every test: a present list must be
/// a list of names, and a `tax_ledgers` table's every value too, or binding refuses
/// `BIND-ID-MALFORMED`. Everything else waits until `book_keeping_quality` runs
/// ([`BookKeepingQualityConfig::inputs`]): a missing key, a `tax_ledgers` that is not a table, and
/// `reissue_narration_terms` (not a name, never bound) refuse that test and no other. The reference
/// reads all five keys before its pack runs any test, so there one missing key refuses the pack.
#[derive(Debug, Clone, Default)]
pub struct BookKeepingQualityConfig {
    pub payment_channel_debtors: Option<Vec<String>>,
    pub gst_payment_ledgers: Option<Vec<String>>,
    pub writeoff_discount_ledgers: Option<Vec<String>>,
    /// `[roles].tax_ledgers`, when present.
    pub tax_ledgers: Option<TaxLedgers>,
}

/// `[roles].tax_ledgers` as binding found it.
#[derive(Debug, Clone)]
pub enum TaxLedgers {
    /// Each GST head with its bound ledgers, in the parsed table's order.
    Heads(Vec<(String, Vec<String>)>),
    /// Present but not a table: `book_keeping_quality` refuses when it runs.
    NotATable,
}

impl BookKeepingQualityConfig {
    /// The test's typed inputs, refusing what the reference's `book_keeping_quality_config` and
    /// `tax_ledgers_by_head` refuse. `raw_roles` is the unbound `[roles]` table, for
    /// `reissue_narration_terms`, which `list()` reads as the reference does: a list's items
    /// (each must be text, as `.upper()` needs), a string's characters, a table's keys.
    ///
    /// Meaningful on a bound engagement only ([`Engagement::bind`] fills the name locations); on
    /// one that has not been bound every name location reads as missing.
    ///
    /// One divergence, a refusal: a ledger listed under two different GST heads. The reference
    /// keeps the head that comes last in the file; the parsed TOML here does not keep the file's
    /// order, so which head is last cannot be known.
    pub fn inputs(&self, raw_roles: Option<&toml::Table>) -> Result<book_keeping_quality::Inputs> {
        let missing = |key: &str| {
            AuditError::Config(format!("client config missing required key 'roles.{key}'"))
        };
        let set = |v: &Option<Vec<String>>, key: &str| -> Result<BTreeSet<String>> {
            Ok(v.as_ref()
                .ok_or_else(|| missing(key))?
                .iter()
                .cloned()
                .collect())
        };
        let heads = match &self.tax_ledgers {
            None => return Err(missing("tax_ledgers")),
            Some(TaxLedgers::NotATable) => {
                return Err(AuditError::Config(
                    "book_keeping_quality: [roles].tax_ledgers is not a table".to_string(),
                ))
            }
            Some(TaxLedgers::Heads(heads)) => heads,
        };
        let mut tax_ledgers_by_head: BTreeMap<String, String> = BTreeMap::new();
        for (head, ledgers) in heads {
            for ledger in ledgers {
                if let Some(other) = tax_ledgers_by_head.insert(ledger.clone(), head.clone()) {
                    if other != *head {
                        return Err(AuditError::Config(format!(
                            "book_keeping_quality: ledger {ledger:?} is listed under GST heads {other:?} and {head:?}; the reference keeps the one later in the file, which is not known here"
                        )));
                    }
                }
            }
        }
        let payment_channel_debtors =
            set(&self.payment_channel_debtors, "payment_channel_debtors")?;
        let gst_payment_ledgers = set(&self.gst_payment_ledgers, "gst_payment_ledgers")?;
        let writeoff_discount_ledgers =
            set(&self.writeoff_discount_ledgers, "writeoff_discount_ledgers")?;
        let terms = raw_roles
            .and_then(|r| r.get("reissue_narration_terms"))
            .ok_or_else(|| missing("reissue_narration_terms"))?;
        let not_text = || {
            AuditError::Config(
                "book_keeping_quality: [roles].reissue_narration_terms holds a value that is not text"
                    .to_string(),
            )
        };
        let reissue_narration_terms = match terms {
            toml::Value::Array(items) => items
                .iter()
                .map(|t| t.as_str().map(str::to_string).ok_or_else(not_text))
                .collect::<Result<Vec<_>>>()?,
            toml::Value::String(s) => s.chars().map(String::from).collect(),
            toml::Value::Table(t) => t.keys().cloned().collect(),
            _ => return Err(not_text()),
        };
        Ok(book_keeping_quality::Inputs {
            payment_channel_debtors,
            tax_ledgers_by_head,
            gst_payment_ledgers,
            reissue_narration_terms,
            writeoff_discount_ledgers,
        })
    }
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

/// `[tds_tcs_26as]` from the client config: see [`Engagement::tds_tcs_26as`] and the reference
/// engine's `tds_tcs_26as_config`. Every value must be a string, as the reference's `bind_config`
/// requires of each ledger it binds.
#[derive(Debug, Clone, Default)]
pub struct Tds26asConfig {
    pub tds_ledgers: BTreeSet<String>,
    pub tcs_ledgers: BTreeSet<String>,
    /// Form 26AS deductor/collector TAN -> the books party ledger (client configuration; a TAN,
    /// never a name, identifies the deductor).
    pub deductor_aliases: BTreeMap<String, String>,
    pub advance_tax_ledgers: BTreeSet<String>,
}

/// `[tds]` and `[tds_payees]` from the client config: see [`Engagement::tds`] and the reference
/// engine's `tds_config`.
#[derive(Debug, Clone, Default)]
pub struct TdsConfig {
    /// `[tds].nature_by_ledger`: expense ledger -> "194C" | "194I" | "194J". Kept as written: an
    /// empty value maps nothing and an unknown one maps the ledger without reporting it, as in the
    /// reference. A non-string value is refused, where the reference would take a truthy one as
    /// an unknown nature and a falsy one (`false`, `0`) as no mapping (a divergence, stated in
    /// `tds_payees`).
    pub nature_by_ledger: BTreeMap<String, String>,
    /// `[tds].payee_aliases`: payee ledger -> payee entity label (not a ledger).
    pub payee_aliases: BTreeMap<String, String>,
    /// `[tds].previous_year_turnover_paise`, when supplied. An integer; any other type is refused
    /// (the reference would compare a float too; a divergence, stated in `tds_payees`).
    pub previous_year_turnover_paise: Option<i64>,
    /// `[tds_payees].s194j_category_by_ledger`, optional, empty when absent. A value that is not a
    /// string is kept as `None`: the reference finds it in no category, so the ledger is unmapped.
    pub s194j_category_by_ledger: BTreeMap<String, Option<String>>,
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
                    guid: crate::support::py_lower(guid),
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
        let tds_tcs_26as_missing = cfg
            .get("tds_tcs_26as")
            .and_then(toml::Value::as_table)
            .and_then(|t| {
                [
                    "tds_ledgers",
                    "tcs_ledgers",
                    "deductor_aliases",
                    "advance_tax_ledgers",
                ]
                .into_iter()
                .find(|key| !t.contains_key(*key))
            });
        let tds_tcs_26as = cfg
            .get("tds_tcs_26as")
            .map(|t| -> Result<Tds26asConfig> {
                let t = t.as_table().ok_or_else(|| {
                    AuditError::Config("[tds_tcs_26as] is not a table".to_string())
                })?;
                let set = |key: &str| -> Result<BTreeSet<String>> {
                    let Some(v) = t.get(key) else {
                        return Ok(BTreeSet::new());
                    };
                    v.as_array()
                        .ok_or_else(|| {
                            AuditError::Config(format!("[tds_tcs_26as].{key} is not a list"))
                        })?
                        .iter()
                        .map(|v| {
                            v.as_str().map(str::to_string).ok_or_else(|| {
                                AuditError::Config(format!(
                                    "[tds_tcs_26as].{key} holds a non-string"
                                ))
                            })
                        })
                        .collect()
                };
                let no_aliases = toml::Table::new();
                let deductor_aliases = t
                    .get("deductor_aliases")
                    .map_or(Some(&no_aliases), toml::Value::as_table)
                    .ok_or_else(|| {
                        AuditError::Config(
                            "[tds_tcs_26as].deductor_aliases is not a table".to_string(),
                        )
                    })?
                    .iter()
                    .map(|(tan, v)| {
                        v.as_str()
                            .map(|s| (tan.clone(), s.to_string()))
                            .ok_or_else(|| {
                                AuditError::Config(format!(
                                    "[tds_tcs_26as].deductor_aliases.{tan} is not a string"
                                ))
                            })
                    })
                    .collect::<Result<_>>()?;
                Ok(Tds26asConfig {
                    tds_ledgers: set("tds_ledgers")?,
                    tcs_ledgers: set("tcs_ledgers")?,
                    deductor_aliases,
                    advance_tax_ledgers: set("advance_tax_ledgers")?,
                })
            })
            .transpose()?;
        let tds = cfg
            .get("tds")
            .map(|tds| -> Result<TdsConfig> {
                let tds = tds
                    .as_table()
                    .ok_or_else(|| AuditError::Config("[tds] is not a table".to_string()))?;
                let string_map = |key: &str| -> Result<BTreeMap<String, String>> {
                    tds.get(key)
                        .and_then(toml::Value::as_table)
                        .ok_or_else(|| AuditError::Config(format!("[tds].{key} is not a table")))?
                        .iter()
                        .map(|(k, v)| {
                            v.as_str()
                                .map(|s| (k.clone(), s.to_string()))
                                .ok_or_else(|| {
                                    AuditError::Config(format!("[tds].{key}.{k} is not a string"))
                                })
                        })
                        .collect()
                };
                let previous_year_turnover_paise = tds
                    .get("previous_year_turnover_paise")
                    .map(|v| {
                        v.as_integer().ok_or_else(|| {
                            AuditError::Config(
                                "[tds].previous_year_turnover_paise is not an integer".to_string(),
                            )
                        })
                    })
                    .transpose()?;
                let tds_payees = cfg
                    .get("tds_payees")
                    .map(|t| {
                        t.as_table().ok_or_else(|| {
                            AuditError::Config("[tds_payees] is not a table".to_string())
                        })
                    })
                    .transpose()?;
                let s194j_category_by_ledger = match tds_payees
                    .and_then(|t| t.get("s194j_category_by_ledger"))
                {
                    None => BTreeMap::new(),
                    Some(v) => v
                        .as_table()
                        .ok_or_else(|| {
                            AuditError::Config(
                                "[tds_payees].s194j_category_by_ledger is not a table".to_string(),
                            )
                        })?
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
                        .collect(),
                };
                Ok(TdsConfig {
                    nature_by_ledger: string_map("nature_by_ledger")?,
                    payee_aliases: string_map("payee_aliases")?,
                    previous_year_turnover_paise,
                    s194j_category_by_ledger,
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
            tds_tcs_26as,
            tds_tcs_26as_missing,
            tds,
            book_keeping_quality: BookKeepingQualityConfig::default(),
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
            creditor_groups: None,
            trade_creditors_source: roles.get("trade_creditors_source").cloned(),
            loans: LoansConfig::default(),
            creditor_ageing: CreditorAgeingConfig::default(),
            statutory_dues: StatutoryDuesConfig::default(),
            base_dir: base_dir.to_path_buf(),
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

/// The ledger names a `legacy_json` trade-creditor source reads (`derived.fs.trade_creditors[]
/// .ledger` in the JSON file it names, relative to `base_dir`), or `None` when the source is not
/// that kind. The reference's `legacy_trade_creditor_names`.
pub fn legacy_trade_creditor_names(
    source: Option<&toml::Value>,
    base_dir: &Path,
) -> Result<Option<Vec<String>>> {
    let Some(src) = source.and_then(toml::Value::as_table) else {
        return Ok(None);
    };
    if src.get("kind").and_then(toml::Value::as_str) != Some("legacy_json") {
        return Ok(None);
    }
    let bad = |what: String| AuditError::Config(format!("legacy trade-creditor source: {what}"));
    let path = src
        .get("path")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| bad("no path".to_string()))?;
    let text = std::fs::read_to_string(base_dir.join(path))
        .map_err(|e| bad(format!("cannot read {path}: {e}")))?;
    let data: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| bad(format!("{path}: {e}")))?;
    data["derived"]["fs"]["trade_creditors"]
        .as_array()
        .ok_or_else(|| bad(format!("{path}: no derived.fs.trade_creditors list")))?
        .iter()
        .map(|row| {
            row["ledger"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| bad(format!("{path}: a row has no ledger name")))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// The trade creditors `creditor_ageing_43bh` ages: the reference's `trade_creditors`, on a bound
/// engagement. `groups` resolves `[roles].creditor_groups`; `ledgers` is the list itself (what
/// binding turns a `legacy_json` source into).
pub fn trade_creditors(engagement: &Engagement, book: &book::Book) -> Result<BTreeSet<String>> {
    let src = engagement.trade_creditors_source.as_ref().ok_or_else(|| {
        AuditError::Config(
            "client config missing required key 'roles.trade_creditors_source'".to_string(),
        )
    })?;
    let kind = src.get("kind").and_then(toml::Value::as_str).unwrap_or("");
    match kind {
        "groups" => {
            let groups = engagement.creditor_groups.as_ref().ok_or_else(|| {
                AuditError::Config(
                    "client config missing required key 'roles.creditor_groups'".to_string(),
                )
            })?;
            Ok(book.ledgers_under_any(groups))
        }
        "ledgers" => src
            .get("ledgers")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| {
                AuditError::Config("roles.trade_creditors_source.ledgers is not a list".to_string())
            })?
            .iter()
            .map(|v| {
                v.as_str().map(str::to_string).ok_or_else(|| {
                    AuditError::Config(
                        "roles.trade_creditors_source.ledgers holds a non-string".to_string(),
                    )
                })
            })
            .collect(),
        "legacy_json" => Ok(
            legacy_trade_creditor_names(Some(src), &engagement.base_dir)?
                .unwrap_or_default()
                .into_iter()
                .collect(),
        ),
        other => Err(AuditError::Config(format!(
            "roles.trade_creditors_source: unknown kind {other:?}"
        ))),
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

/// Run `loans_interest` on a book and return its canonical parity dump, with the module's own
/// LOAN-1/2/3 invariants. The previous-year turnover is `[tds].previous_year_turnover_paise`, as
/// the reference's pack reads it; absent without a `[tds]` table.
pub fn loans_interest_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let entity_type = engagement.entity_type.clone().ok_or_else(|| {
        AuditError::Config("loans_interest needs [client].entity_type".to_string())
    })?;
    let (bound, _report) = engagement.bind(book)?;
    if bound.loans.not_a_table {
        return Err(AuditError::Config("[loans] is not a table".to_string()));
    }
    let loans = loans_interest::loan_config(&bound.loans.loan_ledgers)?;
    let cash = book.ledgers_under_any(&bound.cash_groups);
    let bank = book.ledgers_under_any(&bound.bank_groups);
    let shared: BTreeSet<String> = bound
        .loans
        .shared_interest_ledgers
        .iter()
        .cloned()
        .collect();
    let turnover = bound
        .tds
        .as_ref()
        .and_then(|t| t.previous_year_turnover_paise);
    let result = loans_interest::run(
        book,
        rules,
        &entity_type,
        &loans,
        turnover,
        &cash,
        &bank,
        &shared,
    )?;
    let module_check = loans_interest::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Read, verify, build the book, run `depreciation` and return its canonical parity dump.
pub fn depreciation_canonical(engagement: &Engagement, rules: &Rules) -> Result<serde_json::Value> {
    depreciation_on(engagement, &load_book(engagement)?, rules)
}

/// Run `creditor_ageing_43bh` on a book and return its canonical parity dump, with the module's
/// own AGE-1 check. Runs as the reference's pack runs it: no next-year payment data.
pub fn creditor_ageing_43bh_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let creditors = trade_creditors(&engagement, book)?;
    let cfg = &engagement.creditor_ageing;
    if cfg.not_a_table {
        return Err(AuditError::Config(
            "[creditor_ageing_43bh] is not a table".to_string(),
        ));
    }
    let params = creditor_ageing_43bh::Params {
        acceptance_lag_days: match &cfg.acceptance_lag_days {
            None => 0,
            Some(v) => py_int(v, "[creditor_ageing_43bh].acceptance_lag_days")?,
        },
        // A value that is not a string can never be one of the classifications: kept as its TOML
        // text, it is refused only if a creditor in scope looks it up, as the reference refuses.
        supplier_classification: cfg
            .supplier_classification
            .iter()
            .map(|(k, v)| {
                let text = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                (k.clone(), text)
            })
            .collect(),
        post_year_payments: BTreeMap::new(),
        mse_interest_ledgers: cfg.mse_interest_ledgers.iter().cloned().collect(),
    };
    let result = creditor_ageing_43bh::run(book, rules, &engagement.period, &creditors, &params)?;
    let module_check = creditor_ageing_43bh::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Run `statutory_dues_43b` on a book and return its canonical parity dump, with the module's
/// own S43B-1 check.
pub fn statutory_dues_43b_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let cfg = &engagement.statutory_dues;
    if cfg.not_a_table {
        return Err(AuditError::Config(
            "[statutory_dues] is not a table".to_string(),
        ));
    }
    let salary: BTreeSet<String> = cfg.salary_expense_ledgers.iter().cloned().collect();
    // Natures are typed here, when the test runs. A nature that is not a string is refused: the
    // reference would stringify it into figure ids, which no real configuration relies on.
    let nature_by_ledger = cfg
        .nature_by_ledger
        .iter()
        .map(|(ledger, nature)| {
            nature
                .as_str()
                .map(|n| (ledger.clone(), n.to_string()))
                .ok_or_else(|| {
                    AuditError::Config(format!(
                        "[statutory_dues].nature_by_ledger.{ledger} is not a string"
                    ))
                })
        })
        .collect::<Result<BTreeMap<String, String>>>()?;
    let result =
        statutory_dues_43b::run(book, rules, &engagement.period, &nature_by_ledger, &salary)?;
    let module_check = statutory_dues_43b::check_invariants(book, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
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

/// Run `book_keeping_quality` on a book and return its canonical parity dump, its module check
/// (BKQ-1) included. Refuses when a required `[roles]` input is missing or mistyped
/// ([`BookKeepingQualityConfig::inputs`]).
pub fn book_keeping_quality_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let (engagement, _report) = engagement.bind(book)?;
    let raw_roles = engagement
        .raw_cfg
        .get("roles")
        .and_then(toml::Value::as_table);
    let inputs = engagement.book_keeping_quality.inputs(raw_roles)?;
    let cash = book.ledgers_under_any(&engagement.cash_groups);
    let result = book_keeping_quality::run(book, rules, &cash, &inputs)?;
    let module_check = book_keeping_quality::check_invariants(book, &result)?;
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

/// The `[tds_tcs_26as]` table both 26AS tests read, or the refusal the reference's own
/// `tds_tcs_26as_config` raises without it or without one of its keys.
fn tds_26as_config<'a>(bound: &'a Engagement, test: &str) -> Result<&'a Tds26asConfig> {
    if let Some(key) = bound.tds_tcs_26as_missing {
        return Err(AuditError::Config(format!(
            "{test} needs [tds_tcs_26as].{key}: the client config does not set it"
        )));
    }
    bound
        .tds_tcs_26as
        .as_ref()
        .ok_or_else(|| AuditError::Config(format!("{test} needs a [tds_tcs_26as] table")))
}

/// Run `twentysixas_receipts` on a book with the caller's Form 26AS rows (`documents`, from the
/// reference's own adapters; this crate reads no document) and return its canonical parity dump,
/// module invariants included. Refuses without `[tds_tcs_26as]`.
pub fn twentysixas_receipts_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
    documents: &documents::TracesDocuments,
) -> Result<serde_json::Value> {
    let (bound, _report) = engagement.bind(book)?;
    let cfg = tds_26as_config(&bound, twentysixas_receipts::TEST_ID)?;
    let result =
        twentysixas_receipts::run(book, rules, &documents.form26as, &cfg.deductor_aliases)?;
    let module_check = twentysixas_receipts::check_invariants(book, &documents.form26as, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Run `tds_tcs_26as` on a book with the caller's Form 26AS, AIS and TIS rows and return its
/// canonical parity dump, module invariants included. Refuses without `[tds_tcs_26as]`.
pub fn tds_tcs_26as_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
    documents: &documents::TracesDocuments,
) -> Result<serde_json::Value> {
    let (bound, _report) = engagement.bind(book)?;
    let cfg = tds_26as_config(&bound, tds_tcs_26as::TEST_ID)?;
    let result = tds_tcs_26as::run(
        book,
        rules,
        &bound.period,
        &documents.form26as,
        &documents.ais,
        &documents.tis,
        cfg,
    )?;
    let module_check = tds_tcs_26as::check_invariants(book, &documents.form26as, &result)?;
    canonical::canonical_test_result(book, &result, Some(module_check))
}

/// Run `tds_payees` on a book and return its canonical parity dump. Refuses with
/// `AuditError::Config` without `[client].entity_type` (the deductor status needs it) or without a
/// `[tds]` table, as the reference's own `tds_config` requires one. The reference module has no
/// `check_invariants`, so the dump's module invariants are empty on both sides.
pub fn tds_payees_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let entity_type = engagement
        .entity_type
        .clone()
        .ok_or_else(|| AuditError::Config("tds_payees needs [client].entity_type".to_string()))?;
    let (bound, _report) = engagement.bind(book)?;
    let tds = bound
        .tds
        .as_ref()
        .ok_or_else(|| AuditError::Config("tds_payees needs a [tds] table".to_string()))?;
    let result = tds_payees::run(book, rules, &entity_type, tds)?;
    canonical::canonical_test_result(book, &result, None)
}

/// Read, verify, build the book, run `tds_payees` and return its canonical parity dump.
pub fn tds_payees_canonical(engagement: &Engagement, rules: &Rules) -> Result<serde_json::Value> {
    tds_payees_on(engagement, &load_book(engagement)?, rules)
}

/// Read, verify, build the book, run `applicability_44ab` and return its canonical parity dump.
pub fn applicability_44ab_canonical(
    engagement: &Engagement,
    rules: &Rules,
    comparisons: &applicability_44ab::TurnoverInputs,
) -> Result<serde_json::Value> {
    applicability_44ab_on(engagement, &load_book(engagement)?, rules, comparisons)
}

#[cfg(test)]
mod py_int_tests {
    use super::{py_int, py_int_value, PyIntError};
    use toml::Value;

    /// Each expectation is Python 3.13's own `int()` on the same value.
    #[test]
    fn py_int_takes_what_pythons_int_takes() {
        let ok: [(Value, i64); 11] = [
            (Value::Integer(-4), -4),
            (Value::Boolean(true), 1),
            (Value::Boolean(false), 0),
            (Value::Float(3.9), 3),
            (Value::Float(-3.9), -3),
            (Value::from(" 1_000 "), 1000),
            (Value::from("+5"), 5),
            (Value::from(" -0 "), 0),
            (Value::from("007"), 7),
            (Value::from("0_1"), 1),
            (Value::from("\u{a0}5\u{2003}"), 5),
        ];
        for (v, want) in ok {
            assert_eq!(py_int(&v, "t").unwrap(), want, "{v}");
        }
        for bad in ["1__0", "_1", "1_", "1.0", "", " ", "+_1", "--1"] {
            assert!(py_int(&Value::from(bad), "t").is_err(), "{bad:?}");
        }
        assert!(py_int(&Value::Float(f64::NAN), "t").is_err());
        assert!(py_int(&Value::Float(f64::INFINITY), "t").is_err());
        assert!(py_int(&Value::Array(Vec::new()), "t").is_err());
    }

    /// A string as Python 3.13.13's own `int()` reads it (measured; `sys.get_int_max_str_digits()`
    /// is 4300): its whitespace is the six ASCII C-whitespace characters plus non-ASCII whitespace,
    /// not `str.isspace()`; any Unicode decimal digit counts as that digit; the 4,300-digit limit
    /// counts leading zeros and not underscores; and the full i64 range, MIN included, is held.
    #[test]
    fn a_string_reads_as_pythons_int_reads_it() {
        let d4300 = format!("{}1", "0".repeat(4299));
        let d4300_underscored = format!("{}_1", vec!["0"; 4299].join("_"));
        let ok: [(&str, i64); 15] = [
            ("-9223372036854775808", i64::MIN),
            ("9223372036854775807", i64::MAX),
            ("\u{663}\u{664}", 34),
            ("\u{ff11}\u{ff12}", 12),
            ("-\u{663}", -3),
            ("1_\u{663}", 13),
            ("\u{967}\u{968}\u{969}", 123),
            // Mathematical digits: five 0..9 blocks in one decimal range.
            ("\u{1d7d9}\u{1d7e3}\u{1d7ff}", 119),
            ("\t\n7\r\u{b}\u{c}", 7),
            ("\u{2028}8\u{2029}", 8),
            ("\u{85}5\u{3000}", 5),
            (" 1_000 ", 1000),
            ("\u{a0}5\u{2003}", 5),
            (&d4300, 1),
            (&d4300_underscored, 1),
        ];
        for (s, want) in ok {
            assert_eq!(py_int(&Value::from(s), "t").unwrap(), want, "{s:?}");
        }
        let d4301 = format!("{}1", "0".repeat(4300));
        let d4301_big = "1".repeat(4301);
        let invalid: [&str; 11] = [
            "\u{1c}5\u{1f}",
            "- 1",
            "1 0",
            "0x10",
            "\u{b2}",
            "\u{2212}5",
            "\u{661}.\u{665}",
            "\u{200b}9",
            "5\u{0}",
            &d4301,
            &d4301_big,
        ];
        for s in invalid {
            assert!(py_int(&Value::from(s), "t").is_err(), "{s:?}");
        }
    }

    /// Python raises on an invalid literal, on one over the digit limit and on an infinite float;
    /// it returns an integer (which i64 cannot hold) past either end of the range. The two are
    /// different outcomes and stay distinguishable.
    #[test]
    fn past_the_i64_range_is_not_the_same_as_not_an_int() {
        let big_4300 = format!("1{}", "0".repeat(4299));
        for s in ["9223372036854775808", "-9223372036854775809", &big_4300] {
            assert_eq!(
                py_int_value(&Value::from(s)),
                Err(PyIntError::OutOfRange),
                "{s:?}"
            );
        }
        assert_eq!(
            py_int_value(&Value::Float(1e19)),
            Err(PyIntError::OutOfRange)
        );
        assert_eq!(
            py_int_value(&Value::Float(-9.3e18)),
            Err(PyIntError::OutOfRange)
        );
        assert_eq!(
            py_int_value(&Value::Float(-9_223_372_036_854_775_808.0)),
            Ok(i64::MIN)
        );
        let d4301_big = "1".repeat(4301);
        for s in ["1__0", "\u{1c}5", &d4301_big] {
            assert_eq!(
                py_int_value(&Value::from(s)),
                Err(PyIntError::Invalid),
                "{s:?}"
            );
        }
        assert_eq!(
            py_int_value(&Value::Float(f64::INFINITY)),
            Err(PyIntError::Invalid)
        );
    }
}
