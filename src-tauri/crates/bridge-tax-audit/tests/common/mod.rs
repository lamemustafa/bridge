// SPDX-License-Identifier: Apache-2.0
#![allow(dead_code)] // each test binary uses a different subset

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use bridge_tax_audit::applicability_44ab::{ComparisonTurnover, TurnoverInputs};
use bridge_tax_audit::financial_statements::ReportTotals;
use bridge_tax_audit::{
    applicability_44ab_canonical, cash_44ab_canonical, cash_book_integrity_canonical,
    cash_payments_40a3_canonical, depreciation_canonical, financial_statements_canonical,
    ledger_scrutiny_canonical, rules_for, stale_balances_41_1_canonical, trial_balance_canonical,
    Engagement, Result,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn golden() -> Value {
    let text = std::fs::read_to_string(fixtures().join("golden/synthetic.cash_44ab.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

pub fn golden_40a3() -> Value {
    let text = std::fs::read_to_string(fixtures().join("golden/synthetic.cash_payments_40a3.json"))
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

pub fn golden_depreciation() -> Value {
    let text =
        std::fs::read_to_string(fixtures().join("golden/synthetic.depreciation.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

pub fn golden_financial_statements(with_report: bool) -> Value {
    let name = if with_report {
        "golden/synthetic.financial_statements.json"
    } else {
        "golden/synthetic.financial_statements.noreport.json"
    };
    serde_json::from_str(&std::fs::read_to_string(fixtures().join(name)).unwrap()).unwrap()
}

/// The committed synthetic report totals (`synthetic-report-totals.json`), exactly Rs 1 from the
/// derived net profit and closing stock -- FS-1's inclusive tolerance.
pub fn synthetic_report_totals() -> ReportTotals {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("synthetic-report-totals.json")).unwrap(),
    )
    .unwrap();
    ReportTotals {
        net_profit_paise: v["net_profit_paise"].as_i64().unwrap(),
        closing_stock_paise: v["closing_stock_paise"].as_i64(),
        source: v["source"].as_str().map(str::to_string),
    }
}

pub fn golden_applicability_44ab() -> Value {
    let text = std::fs::read_to_string(fixtures().join("golden/synthetic.applicability_44ab.json"))
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The committed synthetic comparison turnover (`synthetic-turnover-inputs.json`).
pub fn synthetic_turnover_inputs() -> TurnoverInputs {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("synthetic-turnover-inputs.json")).unwrap(),
    )
    .unwrap();
    let source = |key: &str| {
        (!v[key].is_null()).then(|| ComparisonTurnover {
            turnover_paise: v[key]["turnover_paise"].as_i64().unwrap(),
            coverage: v[key]["coverage"].as_str().unwrap().to_string(),
        })
    };
    TurnoverInputs {
        books_turnover_paise: None,
        gstr1: source("gstr1"),
        gstr3b: source("gstr3b"),
        ais: source("ais"),
    }
}

/// The synthetic engagement, pointed at `read_dir` instead of the committed read.
pub fn engagement(read_dir: &Path, allow_unbracketed: bool) -> Engagement {
    let text = std::fs::read_to_string(fixtures().join("synthetic-engagement.toml")).unwrap();
    let mut e = Engagement::from_toml(&text, &fixtures()).unwrap();
    e.read_dir = read_dir.to_path_buf();
    e.allow_unbracketed_read = allow_unbracketed;
    e
}

/// A committed golden by its file stem under `golden/`, e.g. `synthetic.trial_balance`.
pub fn golden_named(stem: &str) -> Value {
    let path = fixtures().join(format!("golden/{stem}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

pub fn run_trial_balance(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    trial_balance_canonical(&e, &rules_for(&e)?)
}

pub fn run_cash_book_integrity(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    cash_book_integrity_canonical(&e, &rules_for(&e)?)
}

pub fn run_ledger_scrutiny(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    ledger_scrutiny_canonical(&e, &rules_for(&e)?)
}

pub fn run_stale_balances_41_1(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    stale_balances_41_1_canonical(&e, &rules_for(&e)?)
}

pub fn run(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    cash_44ab_canonical(&e, &rules_for(&e)?)
}

pub fn run_40a3(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    cash_payments_40a3_canonical(&e, &rules_for(&e)?)
}

pub fn run_depreciation(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    depreciation_canonical(&e, &rules_for(&e)?)
}

pub fn run_financial_statements(
    read_dir: &Path,
    allow_unbracketed: bool,
    report_totals: Option<&ReportTotals>,
) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    financial_statements_canonical(&e, &rules_for(&e)?, report_totals)
}

pub fn run_applicability_44ab(read_dir: &Path, comparisons: &TurnoverInputs) -> Result<Value> {
    let e = engagement(read_dir, false);
    applicability_44ab_canonical(&e, &rules_for(&e)?, comparisons)
}

fn hex(bytes: &[u8]) -> String {
    bridge_tax_audit::canonical::hex(&Sha256::digest(bytes))
}

/// A scratch copy of the committed synthetic read, removed on drop.
pub struct ScratchRead {
    pub dir: PathBuf,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl ScratchRead {
    pub fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bridge-tax-audit-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("parts")).unwrap();
        let src = fixtures().join("synthetic-read");
        std::fs::copy(src.join("manifest.json"), dir.join("manifest.json")).unwrap();
        for entry in std::fs::read_dir(src.join("parts")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
        }
        Self { dir }
    }

    pub fn manifest(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join("manifest.json")).unwrap())
            .unwrap()
    }

    pub fn set_manifest(&self, manifest: &Value) {
        std::fs::write(
            self.dir.join("manifest.json"),
            serde_json::to_string_pretty(manifest).unwrap(),
        )
        .unwrap();
    }

    /// Replace `from` with `to` once in an identity-stored part's bytes. With `rehash`, the
    /// manifest's hashes and lengths are updated so C3 admits the altered bytes.
    pub fn edit_part(&self, part_id: &str, from: &str, to: &str, rehash: bool) {
        let mut manifest = self.manifest();
        let part = manifest["parts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["id"] == part_id)
            .unwrap();
        assert_eq!(part["response"]["storage"], "identity");
        let path = self.dir.join(part["response"]["path"].as_str().unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.matches(from).count(),
            1,
            "{from:?} must occur exactly once"
        );
        let bytes = text.replacen(from, to, 1).into_bytes();
        std::fs::write(&path, &bytes).unwrap();
        if rehash {
            let response = &mut part["response"];
            for (hash, len) in [("sha256", "bytes"), ("stored_sha256", "stored_bytes")] {
                response[hash] = Value::from(hex(&bytes));
                response[len] = Value::from(bytes.len());
            }
            self.set_manifest(&manifest);
        }
    }
}

impl Drop for ScratchRead {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
