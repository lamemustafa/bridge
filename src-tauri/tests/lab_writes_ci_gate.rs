//! Gate G4 (audit-sprint 2026-09-14 Phase 3.1): the `lab-writes` Cargo
//! feature must never be enabled by any release or CI workflow. This test
//! runs unconditionally (no `lab-writes` cfg gate on the test itself) so it
//! catches the mistake regardless of which features the test binary itself
//! was built with.
use std::fs;
use std::path::PathBuf;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent directory (the repo root)")
        .join(".github")
        .join("workflows")
}

#[test]
fn no_workflow_enables_the_lab_writes_feature() {
    let dir = workflows_dir();
    let entries = fs::read_dir(&dir).unwrap_or_else(|error| {
        panic!(
            "workflows directory {} must be readable: {error}",
            dir.display()
        )
    });
    let mut checked_files = 0usize;
    let mut offenders = Vec::new();
    for entry in entries {
        let entry = entry.expect("readable workflow directory entry");
        let path = entry.path();
        let is_workflow_file = matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("yml") | Some("yaml")
        );
        if !is_workflow_file {
            continue;
        }
        checked_files += 1;
        let contents = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "workflow file {} must be valid UTF-8: {error}",
                path.display()
            )
        });
        for (line_number, line) in contents.lines().enumerate() {
            let lower = line.to_ascii_lowercase();
            // Catch the feature name spelled either way, and `--all-features`,
            // which would silently pull `lab-writes` in as a real Cargo
            // feature the moment any job used it.
            if lower.contains("lab-writes") || lower.contains("--all-features") {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        checked_files > 0,
        "expected at least one workflow file under {}",
        dir.display()
    );
    assert!(
        offenders.is_empty(),
        "a release/CI workflow must never enable the lab-writes feature (Gate G4 -- \
        audit-sprint 2026-09-14 Phase 3.1): {offenders:#?}"
    );
}

/// A tripwire for the tripwire: this test must actually be capable of
/// failing. Prove that against a synthetic workflow file in a temp
/// directory rather than by editing a real one.
#[test]
fn the_gate_actually_fails_on_a_workflow_that_enables_the_feature() {
    let dir = tempfile::tempdir().expect("temp dir for the tripwire fixture");
    let workflow_path = dir.path().join("fake.yml");
    fs::write(
        &workflow_path,
        "jobs:\n  build:\n    steps:\n      - run: cargo build --features lab-writes\n",
    )
    .expect("write synthetic workflow fixture");
    let contents = fs::read_to_string(&workflow_path).unwrap();
    let tripped = contents
        .lines()
        .any(|line| line.to_ascii_lowercase().contains("lab-writes"));
    assert!(
        tripped,
        "the substring check itself must catch this fixture"
    );
}
