// SPDX-License-Identifier: Apache-2.0
//! Local parity against real reads. Never run in CI, and nothing it reads or writes belongs
//! in this repository: client reads and client configs stay on the machine that holds them.
//!
//! ```text
//! cargo run --release --example local_parity -- \
//!     TEST_ID ENGINE_RULES_TOML CLIENT_TOML READ_DIR PYTHON_DUMP_JSON RUST_DUMP_OUT
//! ```
//!
//! `TEST_ID` is any test in `bridge_tax_audit::registry::PORTED`. (`ledger_scrutiny` and
//! `cash_book_integrity` read `[roles]`: the cash groups, the bank groups for
//! `cash_book_integrity`, and its optional `own_account_narration_terms`.) For `applicability_44ab`, an optional seventh argument
//! `TURNOVER_INPUTS_JSON` feeds the GSTR-1/GSTR-3B/AIS comparison turnover as caller data -- the
//! file `parity/python_golden.py --emit-turnover-inputs` wrote -- so both sides compare against the
//! same numbers; without it neither side has a comparison source. For
//! `financial_statements`, an optional seventh argument `REPORT_TOTALS_JSON` feeds Tally's own
//! Profit & Loss report totals as caller data -- the file `parity/python_golden.py
//! --emit-report-totals` wrote from the same read, so both sides tie against the same numbers;
//! without it both run with no report. `CLIENT_TOML`'s `[partners.*].interest_ledger` entries are
//! read and bound like every other configured name. `CLIENT_TOML` is the
//! reference engine's client config; its `[snapshot]` is replaced in memory by `READ_DIR` with
//! `allow_unbracketed_read = true`, the same switch `parity/python_golden.py --read` applies, so
//! both sides read the same bytes. For `cash_payments_40a3`, `CLIENT_TOML`'s own `[roles]
//! .round_off_ledgers` and `[loans.loan_ledgers.*]` (both optional) are read the same way
//! `Engagement::from_toml` reads them for any other engagement. For `depreciation`,
//! `CLIENT_TOML`'s own `[depreciation]` table (`block_by_ledger`, `opening_wdv_paise`,
//! `dep_expense_ledgers`, all REQUIRED, and an optional `put_to_use_by_voucher`) is read the same
//! way. `PYTHON_DUMP_JSON` is that script's output for the same config, read and test id.
//! `ENGINE_RULES_TOML` is the reference engine's full rules file: the vendored excerpt must still
//! be a byte-for-byte verbatim part of it (checked block by block; see `src/rules.rs`) and give
//! the same values.
//!
//! **Identity binding.** `CLIENT_TOML`'s `[ledger_ids]`/`[group_ids]` are resolved through
//! `Engagement::bind` (`src/binding.rs`, `docs/tax-audit/config-identity-binding-v1.md`) exactly
//! as the reference implementation's `tae/binding.py` resolves them for its own tests, letting a
//! renamed ledger or group bound by identity in the TOML read correctly here too. `python_golden.py`
//! binds the same way now: it calls `bind_config` before building its `Engagement`, mirroring
//! `tae/run.py`'s own `load()`, so a renamed ledger's identity entry (or a bare name that still
//! matches) resolves on both sides of the comparison, not just this one. Those two tables are
//! written for the reference implementation's FULL pack, though, and a real client TOML typically
//! binds many labels this port never reads (`gst_outward`, `related_parties`, ...);
//! `narrow_identity_tables` below strips `[ledger_ids]`/`[group_ids]` down to just the labels the
//! ten locations this port's `Engagement` reads actually use, before `Engagement::from_toml` ever
//! sees them, so `BIND-ID-UNUSED` never fires on a label this port simply does not consume.
//! `python_golden.py`'s own `bind_config` call sees the FULL, unnarrowed tables (it binds every
//! location the reference implementation reads, not just the ones this port reads), so its
//! `BIND-ID-UNUSED` check never trips over a label only this side narrowed away.
//!
//! Prints one summary line, and every difference if there are any; exits non-zero on any
//! difference or refusal.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use bridge_tax_audit::canonical::hex;
use bridge_tax_audit::compare::compare;
use bridge_tax_audit::registry::{self, CallerData};
use bridge_tax_audit::rules::{Rules, SOURCE_SHA256, VENDORED};
use bridge_tax_audit::{load_book, Engagement};
use sha2::{Digest, Sha256};

fn fail(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("local_parity: {message}");
    ExitCode::FAILURE
}

/// Every vendored TOML block (separated by a blank line in `rules/ay2026-27.s44ab.toml`) must
/// independently be a byte-for-byte verbatim substring of the live source. The blocks are not
/// contiguous in the source file (two are truncated mid-table to skip a private research
/// citation, and `[s44ab.turnover]`/`[s271da]` sit between them), so checking the whole
/// post-header body as a single substring -- this example's earlier, `cash_44ab`-only check --
/// no longer applies.
fn vendored_blocks_are_verbatim(source: &str) -> bool {
    let body = &VENDORED[VENDORED.find("\n[meta]\n").map_or(0, |i| i + 1)..];
    body.split("\n\n")
        .map(str::trim_end)
        .filter(|block| !block.is_empty())
        .all(|block| source.contains(block))
}

/// Narrows `[ledger_ids]`/`[group_ids]` to the labels the ten locations this port's `Engagement`
/// actually reads (`roles.cash_groups`, `roles.bank_groups`, `roles.round_off_ledgers`,
/// `tds.nature_by_ledger`'s and `tds.payee_aliases`' keys, `tds_payees.s194j_category_by_ledger`'s
/// keys, `loans.loan_ledgers`'s keys, `depreciation.block_by_ledger`'s keys,
/// `depreciation.dep_expense_ledgers` and `partners.*.interest_ledger`) use, so
/// `Engagement::bind`'s `BIND-ID-UNUSED` check never refuses over a label a real client TOML
/// binds only for a role this port does not implement (see this file's doc comment and `docs/tax-audit/config-identity-binding-v1.md` section 4).
/// A label absent from `cfg` entirely is simply not collected; this never adds anything to `cfg`,
/// only removes stale table entries.
fn narrow_identity_tables(cfg: &mut toml::Table) {
    let mut ledger_labels = std::collections::BTreeSet::new();
    let mut group_labels = std::collections::BTreeSet::new();
    let strs = |v: &toml::Value| -> Vec<String> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    if let Some(roles) = cfg.get("roles").and_then(toml::Value::as_table) {
        for key in ["cash_groups", "bank_groups"] {
            if let Some(v) = roles.get(key) {
                group_labels.extend(strs(v));
            }
        }
        if let Some(v) = roles.get("round_off_ledgers") {
            ledger_labels.extend(strs(v));
        }
    }
    for (table, key) in [
        ("tds", "nature_by_ledger"),
        ("tds", "payee_aliases"),
        ("tds_payees", "s194j_category_by_ledger"),
    ] {
        if let Some(t) = cfg
            .get(table)
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get(key))
            .and_then(toml::Value::as_table)
        {
            ledger_labels.extend(t.keys().cloned());
        }
    }
    if let Some(t) = cfg
        .get("loans")
        .and_then(toml::Value::as_table)
        .and_then(|loans| loans.get("loan_ledgers"))
        .and_then(toml::Value::as_table)
    {
        ledger_labels.extend(t.keys().cloned());
    }
    if let Some(dep) = cfg.get("depreciation").and_then(toml::Value::as_table) {
        if let Some(t) = dep.get("block_by_ledger").and_then(toml::Value::as_table) {
            ledger_labels.extend(t.keys().cloned());
        }
        if let Some(v) = dep.get("dep_expense_ledgers") {
            ledger_labels.extend(strs(v));
        }
    }
    if let Some(partners) = cfg.get("partners").and_then(toml::Value::as_table) {
        for (key, partner) in partners {
            if key == "deed" {
                continue;
            }
            if let Some(label) = partner
                .as_table()
                .and_then(|p| p.get("interest_ledger"))
                .and_then(toml::Value::as_str)
            {
                ledger_labels.insert(label.to_string());
            }
        }
    }
    if let Some(toml::Value::Table(t)) = cfg.get_mut("ledger_ids") {
        t.retain(|k, _| ledger_labels.contains(k));
    }
    if let Some(toml::Value::Table(t)) = cfg.get_mut("group_ids") {
        t.retain(|k, _| group_labels.contains(k));
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (test_id, rules_toml, client_toml, read_dir, python_dump, rust_out, report_json) =
        match args.as_slice() {
            [a, b, c, d, e, f] => (a, b, c, d, e, f, None),
            [a, b, c, d, e, f, g] => (a, b, c, d, e, f, Some(g)),
            _ => {
                return fail(
                    "usage: TEST_ID ENGINE_RULES_TOML CLIENT_TOML READ_DIR PYTHON_DUMP_JSON \
                     RUST_DUMP_OUT [REPORT_TOTALS_JSON | TURNOVER_INPUTS_JSON]",
                )
            }
        };
    let Some(test) = registry::find(test_id) else {
        let ids: Vec<&str> = registry::PORTED.iter().map(|t| t.id).collect();
        return fail(format!(
            "unknown TEST_ID {test_id:?}; expected one of {ids:?}"
        ));
    };
    if report_json.is_some()
        && !["financial_statements", "applicability_44ab"].contains(&test_id.as_str())
    {
        return fail(
            "a seventh argument applies to financial_statements or applicability_44ab only",
        );
    }
    let mut caller = CallerData::default();
    if let Some(path) = report_json {
        let parsed: serde_json::Value = match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(v) => v,
            Err(e) => return fail(format!("{path}: {e}")),
        };
        let filled = if test_id == "financial_statements" {
            registry::report_totals_from_json(&parsed).map(|t| caller.report_totals = Some(t))
        } else {
            registry::turnover_inputs_from_json(&parsed).map(|t| caller.turnover_inputs = t)
        };
        if let Err(e) = filled {
            return fail(format!("{path}: {e}"));
        }
    }

    // The vendored rules excerpt against the live source.
    let source = match std::fs::read_to_string(rules_toml) {
        Ok(s) => s,
        Err(e) => return fail(format!("{rules_toml}: {e}")),
    };
    let source_sha = hex(&Sha256::digest(source.as_bytes()));
    let verbatim = vendored_blocks_are_verbatim(&source);
    let (live, vendored) = (Rules::parse(&source), Rules::vendored());
    let rules = match (live, vendored) {
        (Ok(live), Ok(vendored)) if live == vendored && verbatim => vendored,
        (live, vendored) => {
            return fail(format!(
                "vendored rules no longer match the source: verbatim={verbatim} live={live:?} \
                 vendored={vendored:?}"
            ))
        }
    };
    let rules_note = if source_sha == SOURCE_SHA256 {
        "source unchanged since vendoring"
    } else {
        "source changed since vendoring; the excerpt still matches"
    };

    let mut cfg: toml::Table = match std::fs::read_to_string(client_toml)
        .map_err(|e| e.to_string())
        .and_then(|t| toml::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(cfg) => cfg,
        Err(e) => return fail(format!("{client_toml}: {e}")),
    };
    let mut snapshot = toml::Table::new();
    snapshot.insert("format".into(), "tally-read-v1".into());
    snapshot.insert("path".into(), read_dir.as_str().into());
    snapshot.insert("allow_unbracketed_read".into(), true.into());
    cfg.insert("snapshot".into(), snapshot.into());
    narrow_identity_tables(&mut cfg);
    let engagement = match Engagement::from_toml(&cfg.to_string(), Path::new(".")) {
        Ok(e) => e,
        Err(e) => return fail(e),
    };

    let started = Instant::now();
    let book = match load_book(&engagement) {
        Ok(book) => book,
        Err(e) => return fail(format!("the Rust slice refused the read: {e}")),
    };
    match engagement.bind(&book) {
        Ok((_bound, report)) => println!(
            "binding: bound_by_id={} bound_by_name={} drifts={}{}",
            report.bound_by_id,
            report.bound_by_name,
            report.drifts.len(),
            if report.drifts.is_empty() {
                String::new()
            } else {
                format!(
                    " ({})",
                    report
                        .drifts
                        .iter()
                        .map(|d| format!("{} {:?} -> {:?}", d.kind, d.label, d.current_name))
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            }
        ),
        Err(e) => return fail(format!("the Rust slice refused to bind the config: {e}")),
    }
    let rust = (test.run_on)(&engagement, &book, &rules, &caller);
    let rust = match rust {
        Ok(doc) => doc,
        Err(e) => return fail(format!("the Rust slice refused: {e}")),
    };
    let elapsed = started.elapsed();
    let lines: usize = book.vouchers.iter().map(|v| v.lines.len()).sum();
    println!(
        "book: groups={} ledgers={} tb_rows={} vouchers={} lines={} in_population={}",
        book.groups.len(),
        book.ledgers.len(),
        book.tb.len(),
        book.vouchers.len(),
        lines,
        book.population().map_or(0, |p| p.len())
    );
    if let Err(e) = std::fs::write(
        rust_out,
        serde_json::to_string_pretty(&rust).unwrap() + "\n",
    ) {
        return fail(format!("{rust_out}: {e}"));
    }
    let python: serde_json::Value = match std::fs::read_to_string(python_dump)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => return fail(format!("{python_dump}: {e}")),
    };
    let count = |key: &str| rust[key].as_array().map_or(0, Vec::len);
    match compare(&python, &rust, None) {
        Err(e) => fail(format!("PARITY REFUSED: {e}")),
        Ok(differences) => {
            println!("test_id={test_id} identical_json={}", python == rust);
            println!(
                "figures={} findings={} book_violations={} result_violations={} differences={} rust_ms={} rules: {rules_note}",
                count("figures"),
                count("findings"),
                count("book_invariant_violations"),
                count("result_invariant_violations"),
                differences.len(),
                elapsed.as_millis()
            );
            for d in &differences {
                println!("  - {d}");
            }
            if differences.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
