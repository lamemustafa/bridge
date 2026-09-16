//! Counterparty → ledger mapping, with a suspense fallback and no guessing.
//!
//! This decides only which **name** a statement party is posted under. Whether
//! that name is a real master, and which master a near-miss spelling was meant
//! to be, is `validate_masters`' job; nothing here matches against Tally.

use crate::bank::PARSER_SENTINELS;
use crate::refusal::Refusal;
use crate::text::{mapping_key, squash, strip};
use std::collections::BTreeMap;
use std::io::Read;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Treatment {
    /// Payment when money leaves the bank, Receipt when it arrives.
    Auto,
    /// Both legs are bank or cash ledgers: an own-account or ATM transfer.
    Contra,
    /// Emit no voucher: a transfer between two accounts that are BOTH being
    /// imported, already carried by the other statement's Contra.
    Skip,
}

impl Treatment {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "contra" => Some(Self::Contra),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

/// One instruction as supplied, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappingRow {
    /// Where the row came from, for refusal messages (a CSV line, an index).
    pub origin: String,
    pub party: String,
    pub ledger: String,
    /// `None` means "not given" and defaults to `auto`.
    pub treatment: Option<String>,
}

/// A validated mapping, keyed by [`mapping_key`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapping(BTreeMap<String, (String, Treatment)>);

impl Mapping {
    pub fn get(&self, party: &str) -> Option<&(String, Treatment)> {
        self.0.get(&mapping_key(party))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Validate instructions (the record loop of `load_mapping`).
    pub fn from_rows(rows: impl IntoIterator<Item = MappingRow>) -> Result<Self, Refusal> {
        let mut mapping = BTreeMap::new();
        let mut first_origin: BTreeMap<String, String> = BTreeMap::new();
        for row in rows {
            let party = squash(&row.party);
            if party.is_empty() {
                continue;
            }
            let treatment_text = match row.treatment.as_deref() {
                None | Some("") => "auto".to_string(),
                Some(text) => strip(text).to_lowercase(),
            };
            let Some(treatment) = Treatment::parse(&treatment_text) else {
                return Err(Refusal::new(
                    "unknown_treatment",
                    format!(
                        "{}: unknown treatment; use auto, contra or skip",
                        row.origin
                    ),
                ));
            };
            let ledger = squash(&row.ledger);
            if treatment == Treatment::Contra && ledger.is_empty() {
                return Err(Refusal::new(
                    "contra_without_ledger",
                    format!(
                        "{}: marked contra with no ledger. A Contra's other leg must be a bank or cash ledger; defaulting it to suspense would misclassify the transaction.",
                        row.origin
                    ),
                ));
            }
            let key = mapping_key(&party);
            if PARSER_SENTINELS.contains(&key.as_str()) {
                return Err(Refusal::new(
                    "mapping_claims_a_sentinel",
                    format!(
                        "{}: this is a value the parser prints when it could NOT identify a counterparty, not a counterparty name. A row for it would send unrelated transactions to one ledger, or with skip drop them all. Map the individual narrations instead.",
                        row.origin
                    ),
                ));
            }
            if let Some(existing) = mapping.get(&key) {
                if existing != &(ledger.clone(), treatment) {
                    return Err(Refusal::new(
                        "mapping_key_collision",
                        format!(
                            "{} and {} reduce to the same mapping key but give different instructions; every transaction for both would post under whichever came last",
                            row.origin, first_origin[&key]
                        ),
                    ));
                }
            }
            first_origin.entry(key.clone()).or_insert(row.origin);
            mapping.insert(key, (ledger, treatment));
        }
        Ok(Self(mapping))
    }

    /// `party,ledger,treatment` CSV (`load_mapping`). Headers are matched
    /// case-insensitively after trimming, and a header that appears twice under
    /// that rule is refused: one would silently overwrite the other.
    pub fn from_csv(reader: impl Read) -> Result<Self, Refusal> {
        let mut csv = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(reader);
        let mut records = csv.records();
        let malformed = |error: csv::Error| {
            Refusal::new(
                "mapping_unreadable",
                format!("the mapping is not readable UTF-8 CSV ({error})"),
            )
        };
        let headers: Vec<String> = match records.next() {
            Some(record) => record
                .map_err(malformed)?
                .iter()
                .map(|name| strip(name).to_lowercase())
                .collect(),
            None => Vec::new(),
        };
        let mut repeated: Vec<&str> = headers
            .iter()
            .filter(|name| headers.iter().filter(|other| other == name).count() > 1)
            .map(String::as_str)
            .collect();
        repeated.sort_unstable();
        repeated.dedup();
        if !repeated.is_empty() {
            return Err(Refusal::new(
                "mapping_headers_duplicated",
                format!(
                    "column(s) {} appear more than once once case and surrounding space are ignored",
                    repeated.join(", ")
                ),
            ));
        }
        let missing: Vec<&str> = ["party", "ledger", "treatment"]
            .into_iter()
            .filter(|name| !headers.iter().any(|header| header == name))
            .collect();
        if !missing.is_empty() {
            return Err(Refusal::new(
                "mapping_headers_missing",
                format!(
                    "missing column(s) {}; the header row must be party,ledger,treatment",
                    missing.join(", ")
                ),
            ));
        }
        let position = |name: &str| headers.iter().position(|header| header == name);
        let (party, ledger, treatment) = (
            position("party").unwrap_or_default(),
            position("ledger").unwrap_or_default(),
            position("treatment").unwrap_or_default(),
        );
        let mut rows = Vec::new();
        for (index, record) in records.enumerate() {
            let record = record.map_err(malformed)?;
            // Python's DictReader skips a wholly empty line
            if record.iter().all(str::is_empty) && record.len() <= 1 {
                continue;
            }
            rows.push(MappingRow {
                origin: format!("mapping record {}", index + 2),
                party: record.get(party).unwrap_or_default().to_string(),
                ledger: record.get(ledger).unwrap_or_default().to_string(),
                treatment: record.get(treatment).map(str::to_string),
            });
        }
        Self::from_rows(rows)
    }
}
