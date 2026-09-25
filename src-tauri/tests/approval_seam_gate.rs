//! bridge#583: the scripted answer to the native "approve this Journal" dialog
//! exists only in this crate's own unit tests. This gate holds the source,
//! build configuration and workflows to that; the shipped-binary scan
//! (`scripts/check-no-test-seam.mjs`) proves the result in every artefact.
//!
//! It runs as an integration test, which links the non-test library, so it
//! reads files rather than naming the seam: the seam does not exist here.
//! Each check is a function over text, driven below both on the real files
//! and on a broken copy it must reject, so a check that stops looking fails.
use std::fs;
use std::path::{Path, PathBuf};

const SEAM_ITEMS: [&str; 4] = [
    "test_seam",
    "SCRIPTED_APPROVAL",
    "ScriptedApproval",
    "SEAM_MARKER",
];

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent directory (the repo root)")
        .to_path_buf()
}

fn read(path: impl AsRef<Path>) -> String {
    let path = repo().join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Problems with how `approved_import.rs` gates the seam: the non-test arm
/// must be exactly the unchanged function, the test arm and module bare
/// `#[cfg(test)]`, and nothing in the file may widen a gate.
fn seam_gate_problems(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    let mut problems = Vec::new();
    // The attribute governing an item: the nearest line above it that is not
    // a comment, so a doc comment between the two changes nothing.
    let attribute_before = |needle: &str| {
        let index = lines.iter().position(|line| *line == needle)?;
        lines[..index]
            .iter()
            .rev()
            .find(|line| !line.starts_with("//"))
            .copied()
    };
    for (item, gate) in [
        ("use confirm as approve;", "#[cfg(not(test))]"),
        ("use test_seam::approve;", "#[cfg(test)]"),
        // The review dialog for a doubted post (#239), gated the same way.
        ("use confirm_review as approve_review;", "#[cfg(not(test))]"),
        ("use test_seam::approve_review;", "#[cfg(test)]"),
        ("pub(crate) mod test_seam {", "#[cfg(test)]"),
    ] {
        if lines.iter().filter(|line| **line == item).count() != 1 {
            problems.push(format!("expected exactly one `{item}`"));
        } else if attribute_before(item) != Some(gate) {
            problems.push(format!("`{item}` must sit directly under `{gate}`"));
        }
    }
    // Each dialog is asked from exactly one place, and each answer builds only
    // its own type: a post approval is an `ApprovedImport`, a review a
    // `ReviewAcknowledged`, and nothing converts one into the other.
    for call_site in [
        "approve(preview).await?;\n        Ok(Self {",
        "approve_review(preview).await?;\n        Ok(Self(()))",
    ] {
        if source.matches(call_site).count() != 1 {
            problems.push(format!("expected exactly one call site `{call_site}`"));
        }
    }
    for call in ["approve(preview)", "approve_review(preview)"] {
        if source.matches(call).count() != 1 {
            problems.push(format!("`{call}` must be called exactly once"));
        }
    }
    if source.contains("impl From<") || source.contains("impl Into<") {
        problems.push("no conversion may exist between the approval types".into());
    }
    problems.extend(widened_test_gates(source));
    problems
}

/// Any attribute that names `test` inside a cfg (`any(test, ..)`,
/// `cfg_attr(test, ..)`) other than a bare `#[cfg(test)]` or
/// `#[cfg(not(test))]` could widen or relocate a seam's gate, including one on
/// an enclosing block or wrapped over several lines, so each attribute is read
/// whole, from `#[` to its closing bracket. Unrelated cfgs are not a seam's
/// business.
fn widened_test_gates(source: &str) -> Vec<String> {
    attributes(source)
        .into_iter()
        .filter(|attribute| {
            attribute.contains("cfg")
                && (attribute.contains("(test") || attribute.contains(",test"))
                && !matches!(attribute.as_str(), "#[cfg(test)]" | "#[cfg(not(test))]")
        })
        .map(|attribute| format!("`{attribute}` could widen the seam's gate"))
        .collect()
}

/// The post's test-only REMOTEID seam (`SCRIPTED_REMOTE_ID`) lives in the post
/// path itself, so that file may carry only bare `cfg(test)` gates, and the
/// seam's two items must each sit under one.
fn remote_id_seam_problems(source: &str) -> Vec<String> {
    let mut problems = widened_test_gates(source);
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    for item in [
        "tokio::task_local! {",
        "if let Ok(scripted) = SCRIPTED_REMOTE_ID.try_with(|id| *id) {",
    ] {
        let at = lines.iter().position(|line| *line == item);
        if at
            .and_then(|at| at.checked_sub(1))
            .map(|above| lines[above])
            != Some("#[cfg(test)]")
        {
            problems.push(format!("`{item}` must sit directly under `#[cfg(test)]`"));
        }
    }
    problems
}

#[test]
fn the_remote_id_seam_is_gated_by_bare_cfg_test() {
    let source = read("src-tauri/src/agent_import_post.rs");
    assert_eq!(remote_id_seam_problems(&source), Vec::<String>::new());
    for broken in [
        source.replacen(
            "#[cfg(test)]\ntokio::task_local! {",
            "#[cfg(any(test, feature = \"remote-id-seam\"))]\ntokio::task_local! {",
            1,
        ),
        source.replacen(
            "    #[cfg(test)]\n    if let Ok(scripted)",
            "    if let Ok(scripted)",
            1,
        ),
    ] {
        assert_ne!(
            broken, source,
            "the fabricated breakage must change the source"
        );
        assert!(!remote_id_seam_problems(&broken).is_empty());
    }
}

/// Every outer or inner attribute in `source`, read from `#[` or `#![` to the
/// bracket that closes it, however many lines it spans. Whitespace is removed
/// and string literals are kept only as `""`: a bracket inside a string (say
/// `feature = "x]y"`) neither ends the attribute nor hides what follows it,
/// and a word inside a string is not a cfg predicate.
fn attributes(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current: Option<String> = None;
    let (mut depth, mut in_string, mut escaped) = (0_i32, false, false);
    for line in source.lines() {
        let trimmed = line.trim();
        if current.is_none() && (trimmed.starts_with("#[") || trimmed.starts_with("#![")) {
            current = Some(String::new());
            depth = 0;
        }
        let Some(text) = current.as_mut() else {
            continue;
        };
        for character in trimmed.chars() {
            if in_string {
                match (escaped, character) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, '"') => {
                        in_string = false;
                        text.push('"');
                    }
                    _ => {}
                }
                continue;
            }
            match character {
                '"' => {
                    in_string = true;
                    text.push('"');
                }
                '[' => {
                    depth += 1;
                    text.push('[');
                }
                ']' => {
                    depth -= 1;
                    text.push(']');
                }
                c if c.is_whitespace() => {}
                c => text.push(c),
            }
            if depth == 0 && character == ']' {
                break;
            }
        }
        if depth == 0 && !in_string {
            found.push(current.take().unwrap());
        }
    }
    found
}

/// Files outside `approved_import.rs` and `*_tests.rs` that name the seam.
fn seam_users_outside_tests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("readable source directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if !name.ends_with(".rs") || name == "approved_import.rs" || name.ends_with("_tests.rs")
            {
                continue;
            }
            let text = fs::read_to_string(&path).expect("UTF-8 source");
            if SEAM_ITEMS.iter().any(|item| text.contains(item)) {
                found.push(path);
            }
        }
    }
    found
}

/// Lines that set `cfg(test)` for a build: a `--cfg test` flag or a build
/// script's `rustc-cfg=test`.
fn cfg_test_settings(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let compact: String = line
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            compact.contains("--cfgtest")
                || compact.contains("--cfg=test")
                || compact.contains("\"--cfg\",\"test\"")
                || compact.contains("rustc-cfg=test")
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn the_seam_is_gated_by_bare_cfg_test_and_its_non_test_arm_is_the_real_dialog() {
    let source = read("src-tauri/src/tally/approved_import.rs");
    assert_eq!(seam_gate_problems(&source), Vec::<String>::new());
    // The check itself: each way of widening or rewiring the gate is caught.
    for broken in [
        source.replace(
            "#[cfg(test)]\npub(crate) mod test_seam {",
            "#[cfg(any(test, feature = \"approval-seam\"))]\npub(crate) mod test_seam {",
        ),
        source.replace(
            "#[cfg(not(test))]\nuse confirm as approve;",
            "#[cfg(not(debug_assertions))]\nuse confirm as approve;",
        ),
        source.replace("approve(preview).await?;", "confirm(preview).await?;"),
        source.replace(
            "approve_review(preview).await?;",
            "confirm_review(preview).await?;",
        ),
        source.replace(
            "#[cfg(not(test))]\nuse confirm_review as approve_review;",
            "#[cfg(not(debug_assertions))]\nuse confirm_review as approve_review;",
        ),
        source.replace(
            "#[cfg(test)]\nuse test_seam::approve_review;",
            "use test_seam::approve_review;",
        ),
        // A review answer must never build a post approval.
        source.replace(
            "approve_review(preview).await?;\n        Ok(Self(()))",
            "approve(preview).await?;\n        Ok(Self(()))",
        ),
        format!(
            "{source}\nimpl From<ReviewAcknowledged> for ApprovedImport {{ fn from(_: ReviewAcknowledged) -> Self {{ unreachable!() }} }}\n"
        ),
        source.replace(
            "#[cfg(test)]\nuse test_seam::approve;",
            "use test_seam::approve;",
        ),
        source.replace(
            "#[cfg(test)]\nuse test_seam::approve;",
            "#[cfg_attr(test, allow(unused))]\nuse test_seam::approve;",
        ),
        // A bracket inside a string literal must not end the attribute early
        // and hide the `test` that follows it (on an enclosing item, so the
        // seam's own bare `#[cfg(test)]` still sits directly above it).
        source.replace(
            "#[cfg(test)]\npub(crate) mod test_seam {",
            "#[cfg(any(feature = \"x]y\", test))]\nmod scoped {}\n#[cfg(test)]\npub(crate) mod test_seam {",
        ),
        // A widened gate on an enclosing block, wrapped over lines, with the
        // seam's own bare `#[cfg(test)]` left in place inside it.
        source.replace(
            "#[cfg(test)]\npub(crate) mod test_seam {",
            "#[cfg(any(\n    test,\n    feature = \"approval-seam\"\n))]\nmod scoped {}\n#[cfg(test)]\npub(crate) mod test_seam {",
        ),
    ] {
        assert_ne!(
            broken, source,
            "the fabricated breakage must change the source"
        );
        assert!(!seam_gate_problems(&broken).is_empty());
    }
    // Ordinary edits that leave the gate alone pass: a doc comment between an
    // attribute and its item, and an unrelated feature cfg elsewhere.
    for benign in [
        source.replace(
            "#[cfg(test)]\npub(crate) mod test_seam {",
            "#[cfg(test)]\n/// Scripted approvals.\npub(crate) mod test_seam {",
        ),
        format!("{source}\n#[cfg(feature = \"voucher-scan\")]\nfn unrelated() {{}}\n"),
        // A word inside a string is not a cfg predicate.
        format!("{source}\n#[cfg(feature = \"test-fixtures\")]\nfn unrelated() {{}}\n"),
    ] {
        assert_ne!(benign, source);
        assert_eq!(seam_gate_problems(&benign), Vec::<String>::new());
    }
}

/// The post path accepts only a post approval: its signature names
/// `ApprovedImport`, so passing a review answer is a compile error (#239).
fn post_path_problems(runtime: &str) -> Vec<String> {
    let signature = "request: super::approved_import::ApprovedImport,";
    let name = "async fn post_approved_import<";
    let start = runtime.find(name).map(|start| start + name.len());
    match start.and_then(|start| {
        runtime[start..]
            .find(')')
            .map(|end| &runtime[start..start + end])
    }) {
        Some(parameters) if parameters.contains(signature) => Vec::new(),
        _ => vec![format!("post_approved_import must take `{signature}`")],
    }
}

#[test]
fn the_post_path_accepts_only_a_post_approval() {
    let runtime = read("src-tauri/src/tally/runtime.rs");
    assert_eq!(post_path_problems(&runtime), Vec::<String>::new());
    let broken = runtime.replace(
        "request: super::approved_import::ApprovedImport,",
        "request: super::approved_import::ReviewAcknowledged,",
    );
    assert_ne!(broken, runtime);
    assert!(!post_path_problems(&broken).is_empty());
}

/// Each subprocess mode runs its own dialog: swapping them would show the
/// post dialog for a review, or answer a review with a post approval (#239).
fn dialog_mode_problems(lib: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for arm in [
        "Some(\"--confirm-journal\") => agent::run_confirmation as fn() -> bool,",
        "Some(\"--confirm-review\") => agent::run_review_confirmation,",
    ] {
        if lib.matches(arm).count() != 1 {
            problems.push(format!("expected exactly one `{arm}`"));
        }
    }
    problems
}

#[test]
fn each_dialog_mode_runs_its_own_dialog() {
    let lib = read("src-tauri/src/lib.rs");
    assert_eq!(dialog_mode_problems(&lib), Vec::<String>::new());
    let swapped = lib
        .replace("agent::run_confirmation as fn() -> bool,", "SWAP,")
        .replace(
            "agent::run_review_confirmation,",
            "agent::run_confirmation as fn() -> bool,",
        )
        .replace("SWAP,", "agent::run_review_confirmation,");
    assert_ne!(swapped, lib);
    assert!(!dialog_mode_problems(&swapped).is_empty());
}

/// The block of the queued post whose errors are marked as refused before the
/// intent (#656) must hold no intent and no POST: a refusal from inside it is
/// reported as "nothing was sent". So in `post_approved_import` the block
/// opens once, closes into `PreIntentQueueRefusal` once, holds neither
/// `before_dispatch()` nor `post_probe_xml(`, and both follow it in that order.
fn pre_intent_block_problems(runtime: &str) -> Vec<String> {
    let open = "let (admission_evidence, before_marks) = async {";
    let close = "PreIntentQueueRefusal { source }";
    let Some(function) = runtime.find("async fn post_approved_import<") else {
        return vec!["post_approved_import not found".into()];
    };
    let body = &runtime[function..];
    let body = &body[..body.find("\n    }\n").unwrap_or(body.len())];
    if body.matches(open).count() != 1 || body.matches(close).count() != 1 {
        return vec!["expected exactly one marked block in post_approved_import".into()];
    }
    let start = body.find(open).unwrap();
    let end = body.find(close).unwrap();
    if end < start {
        return vec!["the marked block closes before it opens".into()];
    }
    let mut problems = Vec::new();
    let block = &body[start..end];
    for forbidden in ["before_dispatch()", "post_probe_xml("] {
        if block.contains(forbidden) {
            problems.push(format!("`{forbidden}` inside the pre-intent block"));
        }
    }
    let after = &body[end..];
    match (
        after.find("before_dispatch()"),
        after.find("post_probe_xml("),
    ) {
        (Some(intent), Some(post)) if intent < post => {}
        _ => problems.push("the intent, then the POST, must follow the block".into()),
    }
    problems
}

#[test]
fn nothing_is_sent_inside_the_pre_intent_block() {
    let runtime = read("src-tauri/src/tally/runtime.rs");
    assert_eq!(pre_intent_block_problems(&runtime), Vec::<String>::new());
    // The check itself: the intent moved into the block, or the block's error
    // left unmarked, is caught.
    let intent_inside = runtime.replacen(
        "let (admission_evidence, before_marks) = async {",
        "let (admission_evidence, before_marks) = async {\n                        before_dispatch().ok();",
        1,
    );
    let unmarked = runtime.replacen("PreIntentQueueRefusal { source }", "source", 1);
    for broken in [intent_inside, unmarked] {
        assert_ne!(broken, runtime);
        assert!(!pre_intent_block_problems(&broken).is_empty());
    }
}

#[test]
fn only_test_files_name_the_seam() {
    let source = repo().join("src-tauri").join("src");
    assert_eq!(seam_users_outside_tests(&source), Vec::<PathBuf>::new());
    let fabricated = tempfile::tempdir().unwrap();
    fs::write(
        fabricated.path().join("agent_import_post_tests.rs"),
        "SCRIPTED_APPROVAL",
    )
    .unwrap();
    assert!(seam_users_outside_tests(fabricated.path()).is_empty());
    fs::write(
        fabricated.path().join("agent_import_post.rs"),
        "use test_seam::ScriptedApproval;",
    )
    .unwrap();
    assert_eq!(seam_users_outside_tests(fabricated.path()).len(), 1);
}

#[test]
fn nothing_sets_cfg_test_for_a_build() {
    let mut candidates: Vec<PathBuf> = [
        ".cargo/config.toml",
        ".cargo/config",
        "src-tauri/.cargo/config.toml",
        "src-tauri/.cargo/config",
    ]
    .iter()
    .map(|path| repo().join(path))
    .collect();
    candidates.extend(
        fs::read_dir(repo().join(".github").join("workflows"))
            .unwrap()
            .map(|entry| entry.unwrap().path()),
    );
    candidates.push(repo().join("src-tauri").join("build.rs"));
    for crate_dir in fs::read_dir(repo().join("src-tauri").join("crates")).unwrap() {
        candidates.push(crate_dir.unwrap().path().join("build.rs"));
    }
    candidates.push(repo().join("package.json"));
    let mut examined = 0;
    for path in candidates.iter().filter(|path| path.is_file()) {
        examined += 1;
        let settings = cfg_test_settings(&fs::read_to_string(path).unwrap());
        assert!(
            settings.is_empty(),
            "{} sets cfg(test): {settings:?}",
            path.display()
        );
    }
    assert!(
        examined >= 3,
        "the gate must have looked at the workflows and build scripts"
    );
    for setting in [
        "RUSTFLAGS: --cfg test",
        "rustflags = [\"--cfg\", \"test\"]",
        "println!(\"cargo:rustc-cfg=test\");",
        "RUSTFLAGS=\"--cfg=test\" cargo build",
    ] {
        assert_eq!(cfg_test_settings(setting).len(), 1, "{setting}");
    }
}

#[test]
fn every_shipped_build_runs_the_seam_scan() {
    let marker = "bridge-test-approval-seam-5f1c9e7a";
    assert!(read("src-tauri/src/tally/approved_import.rs").contains(&format!("\"{marker}\"")));
    assert!(read("scripts/check-no-test-seam.mjs").contains(&format!("\"{marker}\"")));

    let tauri: serde_json::Value =
        serde_json::from_str(&read("src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(
        tauri["build"]["beforeBundleCommand"],
        "node scripts/check-no-test-seam.mjs --tauri-bundle-hook"
    );

    let package = read("scripts/package-mcpb.mjs");
    let scan = package
        .find("assertNoTestSeam([sourceBinary]);")
        .expect("package-mcpb scans the binary");
    let stage = package
        .find("await cp(sourceBinary, destination);")
        .expect("package-mcpb stages the binary");
    assert!(
        scan < stage,
        "package-mcpb must scan the binary before staging it"
    );

    let ci = read(".github/workflows/ci.yml");
    for invocation in [
        "node scripts/check-no-test-seam.mjs --test-harness\n",
        "node scripts/check-no-test-seam.mjs --test-harness --release",
        "node scripts/check-no-test-seam.mjs \"src-tauri/target/release/bridge$ext\" \"src-tauri/target/release/bridge_mcp$ext\"",
        "node scripts/check-no-test-seam.mjs src-tauri/target/release/bundle/macos",
    ] {
        assert!(ci.contains(invocation), "ci.yml must run `{}`", invocation.trim());
    }
    assert!(read(".github/workflows/release-mcpb-preview.yml").contains(
        "node scripts/check-no-test-seam.mjs src-tauri/target/release/${{ matrix.binary }}"
    ));
}
