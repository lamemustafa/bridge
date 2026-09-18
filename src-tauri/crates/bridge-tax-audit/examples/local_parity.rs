// SPDX-License-Identifier: Apache-2.0
//! Local parity against real reads. Never run in CI, and nothing it reads or writes belongs
//! in this repository: client reads and client configs stay on the machine that holds them.
//!
//! ```text
//! cargo run --release --example local_parity -- \
//!     TEST_ID ENGINE_RULES_TOML CLIENT_TOML READ_DIR PYTHON_DUMP_JSON RUST_DUMP_OUT
//! ```
//!
//! `TEST_ID` is `cash_44ab` or `cash_payments_40a3`. `CLIENT_TOML` is the reference engine's
//! client config; its `[snapshot]` is replaced in memory by `READ_DIR` with
//! `allow_unbracketed_read = true`, the same switch `parity/python_golden.py --read` applies, so
//! both sides read the same bytes. For `cash_payments_40a3`, `CLIENT_TOML`'s own `[roles]
//! .round_off_ledgers` and `[loans.loan_ledgers.*]` (both optional) are read the same way
//! `Engagement::from_toml` reads them for any other engagement. `PYTHON_DUMP_JSON` is that
//! script's output for the same config, read and test id. `ENGINE_RULES_TOML` is the reference
//! engine's full rules file: the vendored excerpt must still be a byte-for-byte verbatim part of
//! it (checked block by block; see `src/rules.rs`) and give the same values.
//!
//! Prints one summary line, and every difference if there are any; exits non-zero on any
//! difference or refusal.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use bridge_tax_audit::canonical::hex;
use bridge_tax_audit::compare::compare;
use bridge_tax_audit::rules::{Rules, SOURCE_SHA256, VENDORED};
use bridge_tax_audit::{cash_44ab_on, cash_payments_40a3_on, load_book, Engagement};
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [test_id, rules_toml, client_toml, read_dir, python_dump, rust_out] = args.as_slice()
    else {
        return fail(
            "usage: TEST_ID(cash_44ab|cash_payments_40a3) ENGINE_RULES_TOML CLIENT_TOML \
             READ_DIR PYTHON_DUMP_JSON RUST_DUMP_OUT",
        );
    };
    if test_id != "cash_44ab" && test_id != "cash_payments_40a3" {
        return fail(format!(
            "unknown TEST_ID {test_id:?}; expected cash_44ab or cash_payments_40a3"
        ));
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
    let engagement = match Engagement::from_toml(&cfg.to_string(), Path::new(".")) {
        Ok(e) => e,
        Err(e) => return fail(e),
    };

    let started = Instant::now();
    let book = match load_book(&engagement) {
        Ok(book) => book,
        Err(e) => return fail(format!("the Rust slice refused the read: {e}")),
    };
    let rust = match test_id.as_str() {
        "cash_44ab" => cash_44ab_on(&engagement, &book, &rules),
        _ => cash_payments_40a3_on(&engagement, &book, &rules),
    };
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
