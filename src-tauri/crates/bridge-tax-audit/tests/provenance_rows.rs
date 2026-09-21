// SPDX-License-Identifier: Apache-2.0
//! Every golden and every edge book has a byte row -- "| `file` | bytes | `sha256` | `path` |" --
//! in some Markdown file under `tests/fixtures` (`PROVENANCE.md`, or a batch's
//! `provenance/<batch>.md`). The repository's fixture-provenance gate checks the hash of every row
//! it finds, but accepts a fixture merely named in prose; this makes a row compulsory here.

mod common;

use std::path::Path;

fn markdown_rows(dir: &Path, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            markdown_rows(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            for line in std::fs::read_to_string(&p).unwrap().lines() {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                // "", file, bytes, sha, path, ""
                if cells.len() == 6
                    && cells[3].trim_matches('`').len() == 64
                    && cells[3]
                        .trim_matches('`')
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit())
                {
                    out.push(cells[4].trim_matches('`').to_string());
                }
            }
        }
    }
}

#[test]
fn every_golden_and_edge_book_has_a_byte_row() {
    let root = common::fixtures();
    let mut rows = Vec::new();
    markdown_rows(&root, &mut rows);
    let mut missing = Vec::new();
    for dir in ["golden", "edge-books"] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let rel = format!("{dir}/{name}");
            if !rows.contains(&rel) {
                missing.push(rel);
            }
        }
    }
    assert!(missing.is_empty(), "no byte row for: {missing:?}");
}
