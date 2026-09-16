//! Bank-statement PDF → voucher proposals for Bridge's import builder.
//!
//! A port of the parsing half of `scripts/bank_statement_import.py`. The
//! pipeline, every stage of which fails closed:
//!
//! 1. [`pdf::extract_pages`] — word boxes from the PDF, through PDFium.
//! 2. [`parse::parse_pages`] — rows, by column geometry and the wrap heuristic.
//! 3. [`parse::require_account_match`] — the statement prints the expected account.
//! 4. [`money::reconcile`] and [`money::verify_against_statement`] — every row's
//!    running balance follows from the opening balance, lands on the printed
//!    closing balance, and the printed debit and credit totals agree.
//! 5. [`proposals::build`] — Payment / Receipt / Contra proposals in
//!    `build_import_xml`'s input shape, with a suspense fallback.
//!
//! What is deliberately **not** here: REMOTEIDs, XML, output files, and any
//! matching of names against Tally masters. Bridge's writer owns identity and
//! rendering; `validate_masters` owns near-miss ledger names.

pub mod bank;
pub mod bbox;
pub mod date;
pub mod geometry;
pub mod mapping;
pub mod money;
pub mod parse;
pub mod pdf;
pub mod pipeline;
pub mod proposals;
pub mod refusal;
pub mod text;

pub use bank::Bank;
pub use refusal::Refusal;
