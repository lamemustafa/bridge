//! Gate G4 (audit-sprint 2026-09-14 Phase 3.1): the `lab-writes` Cargo
//! feature must never be enabled by any release or CI workflow. This test
//! runs unconditionally (no `lab-writes` cfg gate on the test itself) so it
//! catches the mistake regardless of which features the test binary itself
//! was built with.
use std::fs;
use std::path::{Path, PathBuf};

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent directory (the repo root)")
        .join(".github")
        .join("workflows")
}

/// The scan itself, taking the directory as an argument.
///
/// Factored out for one reason: the tripwire below must drive *this* code, not
/// a copy of it. It previously re-implemented the substring check inline
/// against its own fixture, which proved `str::contains` works -- never in
/// doubt -- while leaving the real scan free to lose a case with the tripwire
/// still green. A tripwire that does not call the thing it guards is the
/// failure it exists to catch.
///
/// Returns how many workflow files were examined and every offending line, so
/// a caller can assert it actually looked at something.
fn scan_workflows_for_lab_writes(dir: &Path) -> (usize, Vec<String>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|error| {
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
            if lower.contains("lab-writes")
                || lower.contains("lab_writes")
                || lower.contains("--all-features")
            {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_number + 1,
                    line.trim()
                ));
            }
        }
    }
    (checked_files, offenders)
}

#[test]
fn no_workflow_enables_the_lab_writes_feature() {
    let dir = workflows_dir();
    let (checked_files, offenders) = scan_workflows_for_lab_writes(&dir);
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

/// A tripwire for the tripwire: the scan above must actually be capable of
/// failing, and must catch every spelling it claims to.
///
/// This drives `scan_workflows_for_lab_writes` against synthetic fixtures
/// rather than editing a real workflow. Each spelling gets its own case, so
/// dropping any one condition from the scan fails a named case here instead of
/// quietly narrowing the gate.
#[test]
fn the_scan_catches_every_spelling_it_claims_to() {
    for (label, line) in [
        (
            "hyphenated",
            "      - run: cargo build --features lab-writes",
        ),
        (
            "underscored",
            "      - run: cargo build --features lab_writes",
        ),
        ("all-features", "      - run: cargo build --all-features"),
        (
            "mixed case",
            "      - run: cargo build --features LAB-WRITES",
        ),
    ] {
        let dir = tempfile::tempdir().expect("temp dir for the tripwire fixture");
        fs::write(
            dir.path().join("fake.yml"),
            format!("jobs:\n  build:\n    steps:\n{line}\n"),
        )
        .expect("write synthetic workflow fixture");
        let (checked_files, offenders) = scan_workflows_for_lab_writes(dir.path());
        assert_eq!(checked_files, 1, "{label}: the fixture must have been read");
        assert_eq!(
            offenders.len(),
            1,
            "{label}: the scan must catch this spelling -- if it does not, the gate \
            has been narrowed and the real workflows are no longer covered for it"
        );
    }
}

/// The extension filter is a condition like any other, and it had no fixture.
///
/// Every real workflow in this repository is spelled `.yml`, so a regression
/// that dropped `"yaml"` from the match arm would pass every other test here
/// and keep passing until somebody added a `.yaml` workflow -- at which point
/// the gate would silently stop reading it. Covered by fixture instead of by
/// the accident of what the repository happens to contain today.
#[test]
fn the_scan_reads_both_workflow_extensions() {
    for extension in ["yml", "yaml"] {
        let dir = tempfile::tempdir().expect("temp dir for the tripwire fixture");
        fs::write(
            dir.path().join(format!("fake.{extension}")),
            "jobs:\n  build:\n    steps:\n      - run: cargo build --features lab-writes\n",
        )
        .expect("write synthetic workflow fixture");
        let (checked_files, offenders) = scan_workflows_for_lab_writes(dir.path());
        assert_eq!(checked_files, 1, ".{extension} must count as a workflow");
        assert_eq!(
            offenders.len(),
            1,
            ".{extension} must be scanned -- a dropped extension silently stops the \
            gate reading a whole class of workflow file"
        );
    }
}

/// The other half of the pair: the scan must stay silent on a workflow that
/// does nothing wrong. Without this, a scan that flagged every line would pass
/// the test above while making the gate useless.
#[test]
fn the_scan_is_silent_on_a_workflow_that_enables_nothing() {
    let dir = tempfile::tempdir().expect("temp dir for the tripwire fixture");
    fs::write(
        dir.path().join("clean.yml"),
        "jobs:\n  build:\n    steps:\n      - run: cargo build --locked\n",
    )
    .expect("write synthetic workflow fixture");
    fs::write(dir.path().join("ignored.txt"), "lab-writes\n").expect("write non-workflow file");
    let (checked_files, offenders) = scan_workflows_for_lab_writes(dir.path());
    assert_eq!(checked_files, 1, "only the .yml file counts as a workflow");
    assert!(
        offenders.is_empty(),
        "a clean workflow must not trip the gate: {offenders:#?}"
    );
}
