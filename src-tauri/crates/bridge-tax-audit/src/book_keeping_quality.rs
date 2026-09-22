//! Stub for the failing-parity step; replaced by the port.
use std::collections::{BTreeMap, BTreeSet};

use crate::book::Book;
use crate::error::Result;
use crate::findings::TestResult;
use crate::rules::Rules;

pub const TEST_ID: &str = "book_keeping_quality";
pub const VERSION: &str = "1";

#[derive(Debug, Clone, Default)]
pub struct Inputs {
    pub payment_channel_debtors: BTreeSet<String>,
    pub tax_ledgers_by_head: BTreeMap<String, String>,
    pub gst_payment_ledgers: BTreeSet<String>,
    pub reissue_narration_terms: Vec<String>,
    pub writeoff_discount_ledgers: BTreeSet<String>,
}

pub fn run(
    _book: &Book,
    rules: &Rules,
    _cash: &BTreeSet<String>,
    _inputs: &Inputs,
) -> Result<TestResult> {
    Ok(TestResult::new(TEST_ID, VERSION, &rules.version))
}

pub fn check_invariants(_book: &Book, _result: &TestResult) -> Result<Vec<String>> {
    Ok(Vec::new())
}
