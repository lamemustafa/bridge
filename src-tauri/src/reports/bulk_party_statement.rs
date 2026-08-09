//! Writes one statement file per party from an already-complete local result.
//!
//! This module deliberately has no Tally transport dependency. The caller
//! supplies the rows obtained during the completed outstandings read.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::party_statement::{build_party_statement, PartyStatement};
use crate::tally::{OpenBillRow, UnallocatedParty};

#[derive(Debug, Clone, Serialize)]
pub struct BulkPartyStatementResult {
    pub destination: String,
    pub manifest_path: String,
    pub written: Vec<WrittenStatement>,
    pub failures: Vec<StatementFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WrittenStatement {
    pub party: String,
    pub file_name: String,
    pub amount: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatementFailure {
    pub party: String,
    pub error: String,
}

#[derive(Serialize)]
struct StatementManifest<'a> {
    company: &'a str,
    as_of_yyyymmdd: &'a str,
    format: &'a str,
    written: &'a [WrittenStatement],
    failures: &'a [StatementFailure],
}

/// Produces a separate file for every party represented in the complete rows.
///
/// Per-party failures are retained and reported after every remaining party is
/// attempted. That makes a partially completed batch explicit, while still
/// giving the operator usable files for parties whose statements rendered.
pub fn write_bulk_party_statements(
    destination: &Path,
    company: &str,
    as_of_yyyymmdd: &str,
    format: &str,
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
    render: impl Fn(&PartyStatement) -> Result<Vec<u8>, String>,
) -> Result<BulkPartyStatementResult, String> {
    if !destination.is_dir() {
        return Err("Bridge could not use that statement destination folder.".to_string());
    }

    let parties = statement_parties(open_bills, unallocated_by_party);
    let mut written = Vec::with_capacity(parties.len());
    let mut failures = Vec::new();
    for party in parties {
        let statement = match build_party_statement(
            company,
            as_of_yyyymmdd,
            &party,
            open_bills,
            unallocated_by_party,
        ) {
            Ok(statement) => statement,
            Err(error) => {
                failures.push(StatementFailure {
                    party,
                    error: error.to_string(),
                });
                continue;
            }
        };
        let amount = statement.grand_total.as_str().to_string();
        let bytes = match render(&statement) {
            Ok(bytes) => bytes,
            Err(error) => {
                failures.push(StatementFailure { party, error });
                continue;
            }
        };
        let stem = format!(
            "statement-{}-{as_of_yyyymmdd}",
            safe_party_slug(&statement.party)
        );
        match write_unique_file(destination, &stem, format, &bytes) {
            Ok(path) => written.push(WrittenStatement {
                party,
                file_name: file_name(&path)?,
                amount,
            }),
            Err(error) => failures.push(StatementFailure { party, error }),
        }
    }

    let manifest = StatementManifest {
        company,
        as_of_yyyymmdd,
        format,
        written: &written,
        failures: &failures,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("Bridge could not build the statement manifest: {error}"))?;
    let manifest_path = write_unique_file(
        destination,
        &format!("statement-manifest-{as_of_yyyymmdd}"),
        "json",
        &manifest_bytes,
    )?;

    Ok(BulkPartyStatementResult {
        destination: destination.to_string_lossy().into_owned(),
        manifest_path: manifest_path.to_string_lossy().into_owned(),
        written,
        failures,
    })
}

fn statement_parties(
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
) -> BTreeSet<String> {
    open_bills
        .iter()
        .filter(|row| !row.amount.is_zero())
        .map(|row| row.party.clone())
        .chain(
            unallocated_by_party
                .iter()
                .filter(|entry| !entry.amount.is_zero())
                .map(|entry| entry.party.clone()),
        )
        .collect()
}

/// Counts the non-zero, exact-name parties that a bulk statement would cover.
/// This is shared by the operator preview and the writer so their scopes
/// cannot diverge.
pub fn bulk_party_statement_party_count(
    open_bills: &[OpenBillRow],
    unallocated_by_party: &[UnallocatedParty],
) -> usize {
    statement_parties(open_bills, unallocated_by_party).len()
}

/// Converts arbitrary ledger text to a portable ASCII filename component.
/// Separators, controls, Windows-reserved punctuation, leading dots, trailing
/// spaces, and non-ASCII characters all become collapsed hyphens.
fn safe_party_slug(party: &str) -> String {
    let mut slug = String::with_capacity(party.len());
    let mut previous_was_dash = false;
    for ch in party.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_was_dash = false;
        } else if !previous_was_dash {
            slug.push('-');
            previous_was_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "party".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Creates a previously unused filename. `create_new` closes the race between
/// candidate selection and writing, so neither a same-run slug collision nor
/// a pre-existing file can be silently overwritten.
fn write_unique_file(
    destination: &Path,
    stem: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    if Path::new(stem).components().count() != 1 {
        return Err("Bridge could not build a safe statement filename.".to_string());
    }
    for sequence in 1..=10_000_u32 {
        let suffix = if sequence == 1 {
            String::new()
        } else {
            format!("-{sequence}")
        };
        let path = destination.join(format!("{stem}{suffix}.{extension}"));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Bridge could not create {}: {error}",
                    path.display()
                ))
            }
        };
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(format!(
                "Bridge could not finish writing {}: {error}",
                path.display()
            ));
        }
        return Ok(path);
    }
    Err("Bridge could not find an unused statement filename after 10,000 attempts.".to_string())
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| "Bridge could not represent the statement filename.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_tally_core::ExactDecimal;

    fn bill(party: &str, amount: &str) -> OpenBillRow {
        OpenBillRow {
            party: party.to_string(),
            reference: "SYNTHETIC-1".to_string(),
            bill_date: "20260101".to_string(),
            due_date: "20260201".to_string(),
            amount: ExactDecimal::parse(amount).expect("synthetic decimal"),
            age_days: 40,
            kind: "receivable",
        }
    }

    #[test]
    fn traversal_like_party_name_cannot_escape_the_selected_directory() {
        let destination = tempfile::tempdir().expect("temporary destination");
        let result = write_bulk_party_statements(
            destination.path(),
            "Synthetic Books Pvt Ltd",
            "20260808",
            "xlsx",
            &[bill("../../etc/passwd", "15.00")],
            &[],
            |_| Ok(b"synthetic workbook".to_vec()),
        )
        .expect("statement batch succeeds");

        assert_eq!(result.written.len(), 1);
        let file = destination.path().join(&result.written[0].file_name);
        assert!(file.starts_with(destination.path()));
        assert!(file.is_file());
        assert_eq!(
            result.written[0].file_name,
            "statement-etc-passwd-20260808.xlsx"
        );
    }

    #[test]
    fn renderer_failure_is_recorded_in_result_and_manifest_while_other_parties_write() {
        let destination = tempfile::tempdir().expect("temporary destination");
        let result = write_bulk_party_statements(
            destination.path(),
            "Synthetic Books Pvt Ltd",
            "20260808",
            "pdf",
            &[bill("Good Party", "10.00"), bill("Broken Party", "20.00")],
            &[],
            |statement| {
                if statement.party == "Broken Party" {
                    Err("synthetic renderer failure".to_string())
                } else {
                    Ok(b"synthetic PDF".to_vec())
                }
            },
        )
        .expect("partial batch result is returned");

        assert_eq!(result.written.len(), 1);
        assert_eq!(result.written[0].party, "Good Party");
        assert_eq!(result.written[0].amount, "10");
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].party, "Broken Party");
        let manifest = fs::read_to_string(&result.manifest_path).expect("manifest is written");
        assert!(manifest.contains("20260808"));
        assert!(manifest.contains("Synthetic Books Pvt Ltd"));
        assert!(manifest.contains("\"amount\": \"10\""));
        assert!(manifest.contains("Broken Party"));
        assert!(manifest.contains("synthetic renderer failure"));
    }

    #[test]
    fn colliding_safe_names_are_written_to_distinct_files() {
        let destination = tempfile::tempdir().expect("temporary destination");
        let result = write_bulk_party_statements(
            destination.path(),
            "Synthetic Books Pvt Ltd",
            "20260808",
            "xlsx",
            &[bill("A/B", "10.00"), bill("A:B", "20.00")],
            &[],
            |_| Ok(b"synthetic workbook".to_vec()),
        )
        .expect("statements write");

        assert_eq!(result.written.len(), 2);
        assert_ne!(result.written[0].file_name, result.written[1].file_name);
        assert!(result
            .written
            .iter()
            .all(|entry| destination.path().join(&entry.file_name).is_file()));
    }

    #[test]
    fn party_count_deduplicates_nonzero_bill_and_unallocated_parties() {
        let unallocated = vec![
            UnallocatedParty {
                party: "Bill and On Account".to_string(),
                amount: ExactDecimal::parse("25.00").expect("synthetic decimal"),
            },
            UnallocatedParty {
                party: "On Account Only".to_string(),
                amount: ExactDecimal::parse("10.00").expect("synthetic decimal"),
            },
            UnallocatedParty {
                party: "Zero Balance".to_string(),
                amount: ExactDecimal::zero(),
            },
        ];

        assert_eq!(
            bulk_party_statement_party_count(
                &[
                    bill("Bill and On Account", "15.00"),
                    bill("Bill Only", "5.00"),
                    bill("Zero Balance", "0"),
                ],
                &unallocated,
            ),
            3
        );
    }
}
