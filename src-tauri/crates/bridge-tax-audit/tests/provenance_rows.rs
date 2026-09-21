// SPDX-License-Identifier: Apache-2.0
//! Every golden, every edge book and every root-level JSON fixture has a byte row -- "| `file` | bytes | `sha256` | `path` |" --
//! in some Markdown file under `tests/fixtures` (`PROVENANCE.md`, or a batch's
//! `provenance/<batch>.md`), and this test checks that row itself: the file-name cell names the
//! file, the byte count is the file's size and the SHA-256 is its hash. The repository's
//! fixture-provenance gate checks a row's hash only when the row's shape and file-name cell match
//! the file; otherwise it counts the file as named in prose and checks nothing, so a mistyped row
//! would hide a changed golden there. Here it fails.

mod common;

use std::path::Path;

use sha2::{Digest, Sha256};

/// (file-name cell, byte cell, sha cell, path cell) for every four-cell table row.
fn markdown_rows(dir: &Path, out: &mut Vec<(String, String, String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            markdown_rows(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            for line in std::fs::read_to_string(&p).unwrap().lines() {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                if cells.len() == 6 && cells[4].starts_with('`') {
                    let bare = |c: &str| c.trim_matches('`').to_string();
                    out.push((
                        bare(cells[1]),
                        cells[2].to_string(),
                        bare(cells[3]),
                        bare(cells[4]),
                    ));
                }
            }
        }
    }
}

#[test]
fn every_golden_and_edge_book_has_a_correct_byte_row() {
    let root = common::fixtures();
    let mut rows = Vec::new();
    markdown_rows(&root, &mut rows);
    let mut problems = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for dir in ["golden", "edge-books"] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            files.push((format!("{dir}/{name}"), name));
        }
    }
    // The JSON fixtures at the root too (the probe file, the caller data).
    for entry in std::fs::read_dir(&root).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() && p.extension().is_some_and(|x| x == "json") {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            files.push((name.clone(), name));
        }
    }
    {
        for (rel, name) in files {
            let bytes = std::fs::read(root.join(&rel)).unwrap();
            let matching: Vec<_> = rows.iter().filter(|r| r.3 == rel).collect();
            let [(file, count, sha, _)] = matching.as_slice() else {
                problems.push(format!(
                    "{rel}: {} byte rows (want exactly 1)",
                    matching.len()
                ));
                continue;
            };
            if *file != name {
                problems.push(format!("{rel}: file-name cell {file:?}"));
            }
            let parsed = (!count.is_empty()
                && count.bytes().all(|b| b.is_ascii_digit() || b == b','))
            .then(|| count.replace(',', "").parse::<usize>().ok())
            .flatten();
            if parsed != Some(bytes.len()) {
                problems.push(format!(
                    "{rel}: byte cell {count:?}, file has {}",
                    bytes.len()
                ));
            }
            let actual = bridge_tax_audit::canonical::hex(&Sha256::digest(&bytes));
            if *sha != actual {
                problems.push(format!("{rel}: sha256 cell {sha}, file hashes to {actual}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
