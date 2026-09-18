//! Books-based tax-audit tests over a Tally read, ported from a Python reference engine one
//! test at a time, each proven equal to the reference by a canonical parity dump.
//!
//! This slice's end-to-end path: read a tally-read-v1 directory ([`read`], every byte verified
//! against its manifest), build only the book fields a test needs ([`book`]), evaluate the book
//! and result invariants ([`invariants`]), run a test ([`cash_44ab`], [`cash_payments_40a3`])
//! with rule values from a vendored rules excerpt ([`rules`]), and serialise the result
//! canonically ([`canonical`]) so [`compare`] can diff it against the reference engine's dump
//! under the same rules the reference's own comparer applies.
//!
//! Nothing here talks to Tally, and nothing here writes. The crate reads files a person (or,
//! later, Bridge) put on disk.
//!
//! **Parity evidence.** `tests/parity.rs` compares this crate's dump over a committed synthetic
//! read with the reference engine's dump over the same bytes, and proves the comparison can
//! fail. That is parity on invented data only. The evidence that the slice reads real Tally
//! books is `examples/local_parity.rs`, run on the machine that holds client reads and never
//! committed; each change to this crate should record that run's result.
//!
//! **In CI.** The crate is a member of the `src-tauri` Cargo workspace, so the existing
//! workspace `cargo test`/`clippy`/`fmt` steps, and the dependency-inventory and licence gates,
//! already cover it; no crate-specific CI step is needed.

pub mod book;
pub mod canonical;
pub mod cash_44ab;
pub mod cash_payments_40a3;
pub mod compare;
pub mod error;
pub mod findings;
pub mod invariants;
pub mod read;
pub mod rules;
pub mod xml;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use bridge_tally_primitives::TallyDate;

pub use error::{AuditError, Result};
use read::{Read, Window};
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
    pub cash_groups: Vec<String>,
    pub bank_groups: Vec<String>,
    pub round_off_ledgers: Vec<String>,
    pub loan_ledgers_configured: Vec<String>,
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
            cash_groups: strings(roles, "cash_groups")?,
            bank_groups: strings(roles, "bank_groups")?,
            round_off_ledgers: match roles.get("round_off_ledgers") {
                Some(_) => strings(roles, "round_off_ledgers")?,
                None => Vec::new(),
            },
            loan_ledgers_configured: cfg
                .get("loans")
                .and_then(toml::Value::as_table)
                .and_then(|loans| loans.get("loan_ledgers"))
                .and_then(toml::Value::as_table)
                .map(|table| table.keys().cloned().collect())
                .unwrap_or_default(),
        })
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
    read.check(&engagement.period, engagement.allow_unbracketed_read)?;
    book::load_book(&read, &engagement.label)
}

/// Run `cash_44ab` on a book and return its canonical parity dump.
pub fn cash_44ab_on(
    engagement: &Engagement,
    book: &book::Book,
    rules: &Rules,
) -> Result<serde_json::Value> {
    let cash = book.ledgers_under_any(&engagement.cash_groups);
    let bank = book.ledgers_under_any(&engagement.bank_groups);
    let result = cash_44ab::run(book, rules, &cash, &bank)?;
    canonical::canonical_test_result(book, &result)
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
    canonical::canonical_test_result(book, &result)
}

/// Read, verify, build the book, run `cash_payments_40a3` and return its canonical parity dump.
pub fn cash_payments_40a3_canonical(
    engagement: &Engagement,
    rules: &Rules,
) -> Result<serde_json::Value> {
    cash_payments_40a3_on(engagement, &load_book(engagement)?, rules)
}
