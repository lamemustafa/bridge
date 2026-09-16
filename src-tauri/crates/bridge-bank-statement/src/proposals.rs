//! Statement rows → voucher proposals in exactly the shape `build_import_xml`
//! accepts, and nothing more.
//!
//! **No REMOTEID, no XML, no identity.** Bridge's writer owns all three: it
//! mints a batch, derives each voucher's REMOTEID from that batch and the
//! `bridge_txn_id`, renders the envelope, and corrects an imported batch only
//! through `amends_batch_id` under compare-and-swap. The reference script
//! derived its own content-addressed REMOTEID and upserted on re-import; that
//! correction path deliberately does not survive the move.
//!
//! `bridge_txn_id` is a label, not a Tally key. It is derived from what the bank
//! printed on the row, so re-parsing the same statement with a corrected mapping
//! yields the same labels — which is what an amendment of the earlier batch has
//! to name. It never reaches Tally except through Bridge's own derivation.

use crate::bank::{Bank, BALANCE, CREDIT, DATE, DEBIT, NARRATION};
use crate::date::Date;
use crate::mapping::{Mapping, Treatment};
use crate::money::money;
use crate::parse::Row;
use crate::refusal::Refusal;
use crate::text::{ledger_key, mapping_key, squash, strip};
use bridge_tally_primitives::ExactDecimal;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Bridge's per-name and per-text limits (`agent_import.rs`), checked here so a
/// statement that cannot be built is refused while its row number is known.
pub const MAX_LEDGER_CHARS: usize = 1024;
pub const MAX_NARRATION_CHARS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Side {
    Dr,
    Cr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VoucherType {
    Payment,
    Receipt,
    Contra,
}

impl VoucherType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Payment => "Payment",
            Self::Receipt => "Receipt",
            Self::Contra => "Contra",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub ledger: String,
    /// `^[0-9]+\.[0-9]{2}$`, never zero.
    pub amount: String,
    pub side: Side,
}

/// One voucher in `build_import_xml`'s input shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proposal {
    pub bridge_txn_id: String,
    pub date: String,
    pub voucher_type: VoucherType,
    pub narration: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Voucher(VoucherType),
    /// Carried by the other account's Contra; no voucher is proposed.
    Skipped,
}

/// What became of one statement row. Kept for every row inside the date window,
/// including skipped ones, so the operator can see what was left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatementRecord {
    pub row: usize,
    pub date: String,
    pub disposition: Disposition,
    pub amount: String,
    pub party: String,
    /// The ledger the voucher posts the non-bank leg to (empty when skipped).
    pub ledger: String,
    /// The row reached the suspense ledger, by the deliberately loose fold.
    pub suspense: bool,
    pub bridge_txn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    pub proposals: Vec<Proposal>,
    pub records: Vec<StatementRecord>,
}

/// Everything `build` needs besides the rows and the mapping.
#[derive(Debug, Clone)]
pub struct BuildOptions<'a> {
    pub bank_ledger: &'a str,
    pub suspense_ledger: &'a str,
    /// The operator's label for the account, written into narrations for a
    /// human to read.
    pub account_label: &'a str,
    /// The account number `require_account_match` read off the statement.
    pub account_number: &'a str,
    pub date_from: Option<Date>,
    pub date_to: Option<Date>,
}

fn two_places(text: &str) -> String {
    match text.split_once('.') {
        None => format!("{text}.00"),
        Some((whole, fraction)) => format!("{whole}.{fraction:0<2}"),
    }
}

/// An exact decimal as `^-?[0-9]+\.[0-9]{2}$`. Only called on values whose
/// scale is already at most two places.
pub fn format_amount(value: &ExactDecimal) -> String {
    two_places(value.as_str())
}

fn transaction_id(account_number: &str, date: Date, row: &Row) -> String {
    let material = [
        account_number,
        &date.iso(),
        strip(row.get(DEBIT)),
        strip(row.get(CREDIT)),
        strip(row.get(BALANCE)),
        &squash(row.get(NARRATION)),
    ]
    .join("\0");
    let digest = Sha256::digest(material.as_bytes());
    let hex: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("st-{:04}{:02}{:02}-{hex}", date.year, date.month, date.day)
}

fn admissible_text(text: &str, limit: usize) -> bool {
    text.chars().count() <= limit
        && !text
            .chars()
            .any(|c| ('\u{0}'..='\u{1f}').contains(&c) || ('\u{7f}'..='\u{9f}').contains(&c))
        && !text.to_ascii_lowercase().contains("[bridge:")
}

fn require_ledger(name: &str, what: &str, row: Option<usize>) -> Result<(), Refusal> {
    let refusal = |category, message: String| match row {
        Some(row) => Refusal::at_row(category, row, message),
        None => Refusal::new(category, message),
    };
    if strip(name).is_empty() {
        return Err(refusal(
            "blank_ledger",
            format!("the {what} is empty; an empty ledger name reaches every voucher it touches"),
        ));
    }
    if !admissible_text(name, MAX_LEDGER_CHARS) {
        return Err(refusal(
            "ledger_not_admissible",
            format!(
                "the {what} carries control characters, the reserved [BRIDGE: marker, or more than {MAX_LEDGER_CHARS} characters"
            ),
        ));
    }
    Ok(())
}

/// Proposals and a record per statement row, in printed order (`build`).
pub fn build(
    rows: &[Row],
    bank: Bank,
    mapping: &Mapping,
    options: &BuildOptions<'_>,
) -> Result<Build, Refusal> {
    require_ledger(options.bank_ledger, "bank ledger", None)?;
    require_ledger(options.suspense_ledger, "suspense ledger", None)?;
    let mut proposals = Vec::new();
    let mut records = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let number = index + 1;
        let raw_date = strip(row.get(DATE));
        let Some(date) = bank.parse_date(raw_date) else {
            return Err(Refusal::at_row(
                "unparseable_date",
                number,
                format!(
                    "row {number}: the date cell is not a date this layout can produce ({}); the columns have shifted or the extraction is corrupt",
                    bank.date_format()
                ),
            ));
        };
        if options.date_from.is_some_and(|from| date < from)
            || options.date_to.is_some_and(|to| date > to)
        {
            continue;
        }
        let debit = money(row.get(DEBIT), DEBIT, number)?;
        let credit = money(row.get(CREDIT), CREDIT, number)?;
        let (amount, outward) = match (debit, credit) {
            (Some(_), Some(_)) => {
                return Err(Refusal::at_row(
                    "two_sided_row",
                    number,
                    format!("row {number} fills both amount columns"),
                ))
            }
            (Some(debit), None) => (debit, true),
            (None, Some(credit)) => (credit, false),
            (None, None) => {
                return Err(Refusal::at_row(
                    "row_without_amount",
                    number,
                    format!("row {number} has no amount"),
                ))
            }
        };
        if amount.is_zero() {
            // Bridge's builder refuses a zero amount; refuse it here, with the row.
            return Err(Refusal::at_row(
                "zero_amount_row",
                number,
                format!("row {number} carries a zero amount, which no voucher can post"),
            ));
        }
        let amount_text = format_amount(&amount);
        let party = bank.party(row);
        // A sentinel party ("not identified") always reaches suspense. The
        // reference also re-checked that here, as a second lock behind the
        // loader; in Rust a `Mapping` can only be built by `from_rows`, which
        // refuses a sentinel key, so that lock could never fire and is omitted
        // rather than kept as protection nothing exercises.
        let (mapped_ledger, treatment) = mapping
            .get(&party)
            .cloned()
            .unwrap_or((String::new(), Treatment::Auto));
        let ledger = if mapped_ledger.is_empty() {
            options.suspense_ledger.to_string()
        } else {
            mapped_ledger
        };
        let txn_id = transaction_id(options.account_number, date, row);
        if treatment == Treatment::Skip {
            records.push(StatementRecord {
                row: number,
                date: date.iso(),
                disposition: Disposition::Skipped,
                amount: amount_text,
                party,
                ledger: String::new(),
                suspense: false,
                bridge_txn_id: txn_id,
            });
            continue;
        }
        require_ledger(&ledger, "mapped ledger", Some(number))?;
        let voucher_type = match (treatment, outward) {
            (Treatment::Contra, _) => VoucherType::Contra,
            (_, true) => VoucherType::Payment,
            (_, false) => VoucherType::Receipt,
        };
        let (mode, reference) = bank.reference(row);
        // Deliberately the LOOSE fold: over-flagging costs a look. The message
        // names the ledger actually written, never the word "Suspense".
        let unidentified = ledger_key(&ledger) == ledger_key(options.suspense_ledger);
        let shown = if unidentified { &party } else { &ledger };
        let mut narration = squash(&format!(
            "{mode} {reference} {} {shown} | {} | {}",
            if outward { "to" } else { "from" },
            options.account_label,
            date.narration()
        ));
        if unidentified {
            narration.push_str(&format!(" | UNIDENTIFIED - reallocate from {ledger}"));
        }
        if !admissible_text(&narration, MAX_NARRATION_CHARS) {
            return Err(Refusal::at_row(
                "narration_not_admissible",
                number,
                format!(
                    "row {number}'s narration carries control characters, the reserved [BRIDGE: marker, or more than {MAX_NARRATION_CHARS} characters"
                ),
            ));
        }
        if let Some(first) = seen.insert(txn_id.clone(), number) {
            return Err(Refusal::at_row(
                "duplicate_statement_row",
                number,
                format!(
                    "rows {first} and {number} carry the same date, amounts, balance and narration. Check whether the statement really prints the row twice."
                ),
            ));
        }
        let (debit_ledger, credit_ledger) = if outward {
            (ledger.clone(), options.bank_ledger.to_string())
        } else {
            (options.bank_ledger.to_string(), ledger.clone())
        };
        if ledger_key(&debit_ledger) == ledger_key(&credit_ledger) {
            return Err(Refusal::at_row(
                "self_cancelling_voucher",
                number,
                format!(
                    "both legs of row {number}'s voucher resolve to the same ledger. It would balance, import cleanly and move nothing."
                ),
            ));
        }
        proposals.push(Proposal {
            bridge_txn_id: txn_id.clone(),
            date: date.iso(),
            voucher_type,
            narration,
            entries: vec![
                Entry {
                    ledger: debit_ledger,
                    amount: amount_text.clone(),
                    side: Side::Dr,
                },
                Entry {
                    ledger: credit_ledger,
                    amount: amount_text.clone(),
                    side: Side::Cr,
                },
            ],
        });
        records.push(StatementRecord {
            row: number,
            date: date.iso(),
            disposition: Disposition::Voucher(voucher_type),
            amount: amount_text,
            party,
            ledger,
            suspense: unidentified,
            bridge_txn_id: txn_id,
        });
    }
    if records.is_empty() {
        return Err(Refusal::new(
            "empty_selection",
            "no statement row falls inside the date window; the window does not overlap the statement",
        ));
    }
    Ok(Build { proposals, records })
}

/// What [`selfcheck`] counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selfcheck {
    pub vouchers: usize,
    pub bank_out: ExactDecimal,
    pub bank_in: ExactDecimal,
}

/// Re-read the proposals just built (`selfcheck`).
///
/// A transcription check, not a correctness proof: proposals and records come
/// from the same parse. [`crate::money::reconcile`] is the correctness proof.
/// The reference also checked XML sign convention and DATE/EFFECTIVEDATE; those
/// belong to Bridge's writer now and are not re-checked here.
pub fn selfcheck(build: &Build, bank_ledger: &str) -> Result<Selfcheck, Refusal> {
    let bank = ledger_key(bank_ledger);
    let mut bank_out = ExactDecimal::zero();
    let mut bank_in = ExactDecimal::zero();
    let unbalanced = |id: &str| {
        Refusal::new(
            "unbalanced_voucher",
            format!("proposal {id} is not one debit and one credit of equal amount"),
        )
    };
    for proposal in &build.proposals {
        let [first, second] = proposal.entries.as_slice() else {
            return Err(unbalanced(&proposal.bridge_txn_id));
        };
        let amounts_agree = ExactDecimal::parse(first.amount.as_str())
            .ok()
            .zip(ExactDecimal::parse(second.amount.as_str()).ok())
            .is_some_and(|(left, right)| {
                left.numeric_eq(&right) && !left.is_zero() && !left.is_negative()
            });
        if first.side == second.side || !amounts_agree {
            return Err(unbalanced(&proposal.bridge_txn_id));
        }
        if strip(&proposal.narration).is_empty() {
            return Err(Refusal::new(
                "empty_narration",
                format!("proposal {} has an empty narration", proposal.bridge_txn_id),
            ));
        }
        for entry in &proposal.entries {
            if ledger_key(&entry.ledger) == bank {
                let amount = ExactDecimal::parse(entry.amount.as_str())
                    .map_err(|_| unbalanced(&proposal.bridge_txn_id))?;
                let total = match entry.side {
                    Side::Dr => &mut bank_in,
                    Side::Cr => &mut bank_out,
                };
                *total = total
                    .checked_add(&amount)
                    .map_err(|_| unbalanced(&proposal.bridge_txn_id))?;
            }
        }
    }
    let posted = build
        .records
        .iter()
        .filter(|record| record.disposition != Disposition::Skipped)
        .count();
    if posted != build.proposals.len() {
        return Err(Refusal::new(
            "manifest_count_mismatch",
            format!(
                "{posted} rows were dispositioned as vouchers but {} proposals exist",
                build.proposals.len()
            ),
        ));
    }
    Ok(Selfcheck {
        vouchers: build.proposals.len(),
        bank_out,
        bank_in,
    })
}

/// One line of the list an operator writes the mapping from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CounterpartyGroup {
    /// The spelling with the fewest words, then the most value: a cell wrap can
    /// only insert a space the bank did not print, never remove one.
    pub party: String,
    pub also_printed_as: Vec<String>,
    /// The ledger its non-bank leg posts to; empty when skipped.
    pub ledger: String,
    /// `Payment`, `Receipt`, `Contra` or `Skipped`.
    pub disposition: String,
    pub suspense: bool,
    pub rows: usize,
    pub total: String,
}

/// Records grouped the way the mapping is read (`_print_dry_run`): on the
/// mapping key, so two wrap spellings of one payee are one line.
pub fn group_counterparties(
    records: &[StatementRecord],
) -> Result<Vec<CounterpartyGroup>, Refusal> {
    struct Bucket {
        key: (String, String, bool),
        ledger: String,
        total: ExactDecimal,
        rows: usize,
        spellings: Vec<(String, ExactDecimal)>,
    }
    let overflow = |_| {
        Refusal::new(
            "arithmetic_out_of_range",
            "a counterparty total exceeded the exact-decimal bound",
        )
    };
    let mut buckets: Vec<Bucket> = Vec::new();
    for record in records {
        let disposition = match record.disposition {
            Disposition::Voucher(kind) => kind.as_str().to_string(),
            Disposition::Skipped => "Skipped".to_string(),
        };
        let key = (mapping_key(&record.party), disposition, record.suspense);
        let amount = ExactDecimal::parse(record.amount.as_str()).map_err(overflow)?;
        let position = match buckets.iter().position(|bucket| bucket.key == key) {
            Some(position) => position,
            None => {
                buckets.push(Bucket {
                    key,
                    ledger: record.ledger.clone(),
                    total: ExactDecimal::zero(),
                    rows: 0,
                    spellings: Vec::new(),
                });
                buckets.len() - 1
            }
        };
        let bucket = &mut buckets[position];
        bucket.total = bucket.total.checked_add(&amount).map_err(overflow)?;
        bucket.rows += 1;
        match bucket
            .spellings
            .iter_mut()
            .find(|(name, _)| *name == record.party)
        {
            Some((_, total)) => *total = total.checked_add(&amount).map_err(overflow)?,
            None => bucket.spellings.push((record.party.clone(), amount)),
        }
    }
    // Stable sorts, largest first, so ties keep first-appearance order.
    buckets.sort_by(|left, right| right.total.cmp_magnitude(&left.total));
    Ok(buckets
        .into_iter()
        .map(|mut bucket| {
            bucket
                .spellings
                .sort_by(|left, right| right.1.cmp_magnitude(&left.1));
            let shown = bucket
                .spellings
                .iter()
                .enumerate()
                .min_by(|(left_index, left), (right_index, right)| {
                    left.0
                        .split_whitespace()
                        .count()
                        .cmp(&right.0.split_whitespace().count())
                        .then(right.1.cmp_magnitude(&left.1))
                        .then(left_index.cmp(right_index))
                })
                .map(|(_, spelling)| spelling.0.clone())
                .unwrap_or_default();
            CounterpartyGroup {
                also_printed_as: bucket
                    .spellings
                    .iter()
                    .filter(|(name, _)| *name != shown)
                    .map(|(name, _)| name.clone())
                    .collect(),
                party: shown,
                ledger: bucket.ledger,
                disposition: bucket.key.1,
                suspense: bucket.key.2,
                rows: bucket.rows,
                total: format_amount(&bucket.total),
            }
        })
        .collect())
}
