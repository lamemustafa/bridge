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
        "approve(count, preview).await?;\n        Ok(Self {",
        "approve_review(count, preview).await?;\n        Ok(Self(()))",
    ] {
        if source.matches(call_site).count() != 1 {
            problems.push(format!("expected exactly one call site `{call_site}`"));
        }
    }
    for call in ["approve(count, preview)", "approve_review(count, preview)"] {
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
        "if let Ok(scripted) = SCRIPTED_REMOTE_IDS.try_with(Clone::clone) {",
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
        source.replacen(
            "        #[cfg(test)]\n        if let Ok(scripted) = SCRIPTED_REMOTE_IDS",
            "        if let Ok(scripted) = SCRIPTED_REMOTE_IDS",
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
        source.replace(
            "approve(count, preview).await?;",
            "confirm(count, preview).await?;",
        ),
        source.replace(
            "approve_review(count, preview).await?;",
            "confirm_review(count, preview).await?;",
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
            "approve_review(count, preview).await?;\n        Ok(Self(()))",
            "approve(count, preview).await?;\n        Ok(Self(()))",
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

/// The post child's whole entry point: its token, its dialog, and the real
/// stdin and stdout. A test calls `answer_with_token` with its own input,
/// output and dialog, so only this pin covers what the entry point passes.
const POST_ENTRY: &str = "pub fn run_confirmation() -> bool {
    answer_with_token(
        POST_TOKEN_PREFIX,
        show_review,
        std::io::stdin(),
        std::io::stdout(),
    )
}";

/// The review child's whole entry point, as [`POST_ENTRY`].
const REVIEW_ENTRY: &str = "pub fn run_review_confirmation() -> bool {
    answer_with_token(
        REVIEW_TOKEN_PREFIX,
        show_review_acknowledgement,
        std::io::stdin(),
        std::io::stdout(),
    )
}";

/// Each dialog subprocess entry point answers with its own token, after its
/// own dialog, from the real stdin to the real stdout (#635, #687). The
/// parent approves a post only on the post token, so pairing that token with
/// the review dialog would let "I reviewed it" approve a post, and passing a
/// dialog other than the real one would approve with nobody asked. No stub
/// test could see either: a stub is a script, not this code. Each entry
/// point's whole body is pinned, so a swapped body is caught too, and no
/// other `answer_with_token(` call may appear in the file. A text gate cannot
/// see a call that does not spell that out, such as one through a function
/// pointer, a parenthesised path `(answer_with_token)(…)`, or a `use … as`
/// rename; the lib.rs mode arms, pinned above, are still the only way in.
fn dialog_token_problems(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for entry in [POST_ENTRY, REVIEW_ENTRY] {
        if source.matches(entry).count() != 1 {
            problems.push(format!("expected exactly one `{entry}`"));
        }
    }
    if source.matches("answer_with_token(").count() != 3 {
        problems.push("expected the two entry points and the definition only".into());
    }
    problems
}

#[test]
fn each_dialog_answers_with_its_own_token() {
    let source = read("src-tauri/src/tally/approved_import.rs");
    assert_eq!(dialog_token_problems(&source), Vec::<String>::new());
    let post_as_review = POST_ENTRY.replace("POST_TOKEN_PREFIX", "REVIEW_TOKEN_PREFIX");
    let review_as_post = REVIEW_ENTRY.replace("REVIEW_TOKEN_PREFIX", "POST_TOKEN_PREFIX");
    let post_body = &POST_ENTRY[POST_ENTRY.find('{').unwrap()..];
    let review_body = &REVIEW_ENTRY[REVIEW_ENTRY.find('{').unwrap()..];
    let swapped = source
        .replace(POST_ENTRY, "SWAP_POST")
        .replace(REVIEW_ENTRY, "SWAP_REVIEW")
        .replace(
            "SWAP_POST",
            &format!("pub fn run_confirmation() -> bool {review_body}"),
        )
        .replace(
            "SWAP_REVIEW",
            &format!("pub fn run_review_confirmation() -> bool {post_body}"),
        );
    for broken in [
        source.replace(POST_ENTRY, &POST_ENTRY.replace("show_review,", "show_review_acknowledgement,")),
        source.replace(POST_ENTRY, &post_as_review),
        source.replace(REVIEW_ENTRY, &review_as_post),
        source.replace(POST_ENTRY, &POST_ENTRY.replace("show_review,", "|_, _| true,")),
        source.replace(POST_ENTRY, &POST_ENTRY.replace("std::io::stdin()", "&b\"\"[..]")),
        swapped,
        format!(
            "{source}\nfn extra() -> bool {{ answer_with_token(POST_TOKEN_PREFIX, |_, _| true, std::io::stdin(), std::io::stdout()) }}\n"
        ),
    ] {
        assert_ne!(broken, source);
        assert!(!dialog_token_problems(&broken).is_empty());
    }
}

/// Where a click becomes the answer (#687). No test can open a real window,
/// and the parent's stub tests run only on unix. So the four native dialog
/// functions, `confirm` and `confirm_review`, and the two functions that
/// decide from the child's answer (`confirm_with`, `confirm_review_with`)
/// are pinned here verbatim, with the button labels and the functions that
/// word each title and the post button (#746). On Windows the post dialog's
/// title is the only text that says what Yes does. The file's `cfg`
/// attributes are counted as well: a platform or test split anywhere in it,
/// such as a `#[cfg(windows)]` twin of a pinned function, must change this
/// gate. This pins text, not the platform's behaviour.
const REVIEW_ACK_DIALOG: &str = r#"#[cfg(not(windows))]
fn show_review_acknowledgement(count: VoucherCount, preview: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title(review_title(count))
        .set_description(preview)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            "Cancel".into(),
            REVIEW_BUTTON.into(),
        ))
        .show()
        == rfd::MessageDialogResult::Custom(REVIEW_BUTTON.into())
}"#;

const REVIEW_ACK_DIALOG_WINDOWS: &str = r#"#[cfg(windows)]
fn show_review_acknowledgement(count: VoucherCount, preview: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNOCANCEL,
    };
    let text: Vec<u16> = preview.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = review_question(count)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: as for `show_review`: both buffers are NUL-terminated and live
    // for the synchronous dialog, and no parent HWND is borrowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNOCANCEL | MB_DEFBUTTON2 | MB_ICONWARNING | MB_SETFOREGROUND,
        ) == IDYES
    }
}"#;

const POST_DIALOG: &str = r#"#[cfg(not(windows))]
fn show_review(count: VoucherCount, preview: &str) -> bool {
    let (title, button) = post_words(count);
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(preview)
        .set_level(rfd::MessageLevel::Warning)
        // The Cancel label supplies the native Escape action. Posting requires
        // the explicitly matched positive button; Return may leave this dialog open.
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            "Cancel".into(),
            button.clone(),
        ))
        .show()
        == rfd::MessageDialogResult::Custom(button)
}"#;

const POST_DIALOG_WINDOWS: &str = r#"#[cfg(windows)]
fn show_review(count: VoucherCount, preview: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNOCANCEL,
    };
    // rfd without common-controls-v6 discards custom labels. Use the existing
    // Win32 dependency so No is the default and Escape/close remain Cancel.
    let text: Vec<u16> = preview.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = post_question(count).encode_utf16().chain(Some(0)).collect();
    // SAFETY: Both buffers are NUL-terminated and live for the synchronous dialog;
    // no parent HWND is borrowed. No application state is exposed to callbacks.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_YESNOCANCEL | MB_DEFBUTTON2 | MB_ICONWARNING | MB_SETFOREGROUND,
        ) == IDYES
    }
}"#;

/// The post button a batch sees (#746): what `show_review` shows and
/// compares against, so it is pinned with the dialog.
const POST_WORDS: &str = r#"#[cfg(not(windows))]
fn post_words(count: VoucherCount) -> (String, String) {
    match count.batch() {
        None => ("Bridge — approve one voucher".into(), POST_LABEL.into()),
        Some(count) => (
            format!("Bridge — approve {count} vouchers"),
            format!("Post {count} vouchers"),
        ),
    }
}"#;

/// Each dialog's title for one voucher and for a batch (#746): the review's,
/// then the Windows review's and the Windows post's questions.
const REVIEW_TITLE: &str = r#"#[cfg(not(windows))]
fn review_title(count: VoucherCount) -> String {
    match count.batch() {
        None => "Bridge — record that you reviewed one voucher".into(),
        Some(count) => format!("Bridge — record that you reviewed {count} vouchers"),
    }
}"#;

const REVIEW_QUESTION: &str = r#"#[cfg(windows)]
fn review_question(count: VoucherCount) -> String {
    match count.batch() {
        None => "Bridge — record that you reviewed this voucher?".into(),
        Some(count) => format!("Bridge — record that you reviewed these {count} vouchers?"),
    }
}"#;

const POST_QUESTION: &str = r#"#[cfg(windows)]
fn post_question(count: VoucherCount) -> String {
    match count.batch() {
        None => "Bridge — post this voucher?".into(),
        Some(count) => format!("Bridge — post {count} vouchers?"),
    }
}"#;

const CONFIRM_WITH: &str = r#"async fn confirm_with(
    executable: &std::path::Path,
    count: VoucherCount,
    preview: &str,
) -> Result<(), String> {
    if preview.len() > MAX_PREVIEW_BYTES {
        return Err("import_review_too_large".into());
    }
    match nonce_bound_dialog(
        executable,
        "--confirm-journal",
        POST_TOKEN_PREFIX,
        count,
        preview,
    )
    .await
    {
        Ok(answer) if answer.token_matched && answer.exited_cleanly => Ok(()),
        // A person's decline is no token and exit 1: `run_confirmation`
        // returns false. A clean exit without the token is never that; it is
        // an executable that does not answer with this token, such as one
        // ignoring the flag, or a build from before #635 whose dialog ran.
        Ok(answer) if answer.exited_cleanly => Err("import_approval_unavailable".into()),
        Ok(_) => Err("import_approval_declined".into()),
        Err(DialogFailure::Unavailable) => Err("import_approval_unavailable".into()),
        Err(DialogFailure::TimedOut) => Err("import_approval_timed_out".into()),
    }
}"#;

const CONFIRM_REVIEW_WITH: &str = r#"async fn confirm_review_with(
    executable: &std::path::Path,
    count: VoucherCount,
    preview: &str,
) -> Result<(), String> {
    if preview.len() > MAX_PREVIEW_BYTES {
        return Err("ack_review_too_large".into());
    }
    match nonce_bound_dialog(
        executable,
        "--confirm-review",
        REVIEW_TOKEN_PREFIX,
        count,
        preview,
    )
    .await
    {
        Ok(answer) if answer.token_matched => Ok(()),
        // A person's decline is no token and exit 1: `run_review_confirmation`
        // returns false. A clean exit without the token is never that; it is
        // an executable that is not this dialog, such as one ignoring the
        // flag (#689).
        Ok(answer) if answer.exited_cleanly => Err("ack_review_unavailable".into()),
        Ok(_) => Err("ack_review_declined".into()),
        Err(DialogFailure::Unavailable) => Err("ack_review_unavailable".into()),
        Err(DialogFailure::TimedOut) => Err("ack_review_timed_out".into()),
    }
}"#;

const DIALOG_ANSWER_PINS: [(&str, usize); 15] = [
    ("const POST_LABEL: &str = \"Post voucher\";", 1),
    (
        "pub(crate) const REVIEW_BUTTON: &str = \"I reviewed it\";",
        1,
    ),
    (POST_DIALOG, 1),
    (POST_WORDS, 1),
    (POST_QUESTION, 1),
    (REVIEW_TITLE, 1),
    (REVIEW_QUESTION, 1),
    (POST_DIALOG_WINDOWS, 1),
    (REVIEW_ACK_DIALOG, 1),
    (REVIEW_ACK_DIALOG_WINDOWS, 1),
    (
        "async fn confirm(count: VoucherCount, preview: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| \"import_approval_unavailable\")?;
    confirm_with(&executable, count, preview).await
}",
        1,
    ),
    (
        "async fn confirm_review(count: VoucherCount, preview: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| \"ack_review_unavailable\")?;
    confirm_review_with(&executable, count, preview).await
}",
        1,
    ),
    (CONFIRM_WITH, 1),
    (CONFIRM_REVIEW_WITH, 1),
    ("pub(crate) const REVIEW_BUTTON: &str = \"Yes\";", 1),
];

/// Every `cfg` in approved_import.rs, by form. The last entry counts the
/// bare text `cfg`, so a `cfg_attr`, a `cfg!`, or a combined predicate such as
/// `cfg(any(…))` is caught too.
const CFG_CENSUS: [(&str, usize); 6] = [
    ("#[cfg(test)]", 5),
    ("#[cfg(not(test))]", 2),
    ("#[cfg(unix)]", 7),
    ("#[cfg(windows)]", 6),
    ("#[cfg(not(windows))]", 7),
    ("cfg", 28),
];

fn dialog_answer_problems(source: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (pin, expected) in DIALOG_ANSWER_PINS.iter().chain(CFG_CENSUS.iter()) {
        if source.matches(pin).count() != *expected {
            problems.push(format!("expected {expected} of `{pin}`"));
        }
    }
    problems
}

/// A dialog function rewritten to compute its comparison and then return
/// `true` whatever the person chose.
fn discard_answer(dialog: &str) -> String {
    let discarded = if dialog.contains("rfd::MessageDialog::new()") {
        dialog.replacen(
            "    rfd::MessageDialog::new()",
            "    let _ = rfd::MessageDialog::new()",
            1,
        )
    } else {
        dialog.replacen("        MessageBoxW(", "        let _ = MessageBoxW(", 1)
    };
    let discarded = if discarded.ends_with(")\n}") || discarded.ends_with("))\n}") {
        format!("{};\n    true\n}}", discarded.trim_end_matches("\n}"))
    } else {
        discarded.replacen(
            ") == IDYES\n    }\n}",
            ") == IDYES;\n        true\n    }\n}",
            1,
        )
    };
    assert_ne!(discarded, dialog);
    discarded
}

#[test]
fn each_dialog_answers_only_on_its_positive_button() {
    let source = read("src-tauri/src/tally/approved_import.rs");
    assert_eq!(dialog_answer_problems(&source), Vec::<String>::new());
    for broken in [
        source.replacen(
            "== rfd::MessageDialogResult::Custom(button)",
            "!= rfd::MessageDialogResult::Custom(button)",
            1,
        ),
        source.replacen(
            "== rfd::MessageDialogResult::Custom(REVIEW_BUTTON.into())",
            "!= rfd::MessageDialogResult::Custom(REVIEW_BUTTON.into())",
            1,
        ),
        source.replacen(") == IDYES", ") != IDNO", 1),
        source.replacen(
            "MB_YESNOCANCEL | MB_DEFBUTTON2",
            "MB_YESNOCANCEL | MB_DEFBUTTON1",
            1,
        ),
        source.replacen("\"Post voucher\"", "\"Cancel\"", 1),
        source.replacen("\"I reviewed it\"", "\"Cancel\"", 1),
        source.replacen(
            "confirm_with(&executable, count, preview).await\n}",
            "let _ = (executable, count, preview);\n    Ok(())\n}",
            1,
        ),
        source.replacen(
            "confirm_review_with(&executable, count, preview).await\n}",
            "let _ = (executable, count, preview);\n    Ok(())\n}",
            1,
        ),
        source.replacen(
            "Ok(answer) if answer.token_matched && answer.exited_cleanly => Ok(()),",
            "Ok(answer) if answer.exited_cleanly => Ok(()),",
            1,
        ),
        source.replacen(
            "Ok(answer) if answer.token_matched => Ok(()),",
            "Ok(_) => Ok(()),",
            1,
        ),
        // A Windows-only twin that approves, beside the real one made
        // non-Windows: every pinned body is still present.
        source.replacen(
            CONFIRM_WITH,
            &format!(
                "#[cfg(not(windows))]\n{CONFIRM_WITH}\n#[cfg(windows)]\nasync fn confirm_with(_: &std::path::Path, _: VoucherCount, _: &str) -> Result<(), String> {{\n    Ok(())\n}}"
            ),
            1,
        ),
        format!(
            "{source}\n#[cfg(windows)]\nfn dialog_token(prefix: &str, _: &str) -> String {{\n    prefix.to_string()\n}}\n"
        ),
        format!("{source}\n#[cfg_attr(windows, allow(unused))]\nfn extra() {{}}\n"),
        source.replacen(
            CONFIRM_WITH,
            &CONFIRM_WITH.replacen(
                "        Ok(answer) if answer.token_matched && answer.exited_cleanly => Ok(()),",
                "        Ok(answer) if answer.exited_cleanly => Ok(()),\n        Ok(answer) if answer.token_matched && answer.exited_cleanly => Ok(()),",
                1,
            ),
            1,
        ),
        // The review dialog's clean-exit arm (#689) turned into an approval.
        source.replacen(
            "Ok(answer) if answer.exited_cleanly => Err(\"ack_review_unavailable\".into()),",
            "Ok(answer) if answer.exited_cleanly => Ok(()),",
            1,
        ),
        // A Windows-only review twin that acknowledges, beside the real one
        // made non-Windows.
        source.replacen(
            CONFIRM_REVIEW_WITH,
            &format!(
                "#[cfg(not(windows))]\n{CONFIRM_REVIEW_WITH}\n#[cfg(windows)]\nasync fn confirm_review_with(_: &std::path::Path, _: &str) -> Result<(), String> {{\n    Ok(())\n}}"
            ),
            1,
        ),
        source.replacen("\"Yes\";", "\"No\";", 1),
        // A batch's post button reads as the decline, or the dialog shows one
        // button and compares against another (#746).
        source.replacen("format!(\"Post {count} vouchers\")", "\"Cancel\".into()", 1),
        source.replacen("            button.clone(),\n", "            \"Post\".into(),\n", 1),
        // The Windows post dialog asks the review's question, so Yes reads as
        // recording a review while it posts (#746).
        source.replacen(
            "None => \"Bridge — post this voucher?\".into(),",
            "None => \"Bridge — record that you reviewed this voucher?\".into(),",
            1,
        ),
        source.replacen(
            "format!(\"Bridge — post {count} vouchers?\")",
            "format!(\"Bridge — record that you reviewed these {count} vouchers?\")",
            1,
        ),
        // A review dialog's title reads as approving a post (#746).
        source.replacen(
            "format!(\"Bridge — record that you reviewed {count} vouchers\")",
            "format!(\"Bridge — approve {count} vouchers\")",
            1,
        ),
        // The Windows review dialog asks the post's question (#746).
        source.replacen(
            "format!(\"Bridge — record that you reviewed these {count} vouchers?\")",
            "format!(\"Bridge — post {count} vouchers?\")",
            1,
        ),
        // rfd post dialog discards its answer: it still computes the comparison, then returns true.
        source.replacen(POST_DIALOG, &discard_answer(POST_DIALOG), 1),
        // Windows post dialog discards its answer: it still computes the comparison, then returns true.
        source.replacen(POST_DIALOG_WINDOWS, &discard_answer(POST_DIALOG_WINDOWS), 1),
        // rfd review dialog discards its answer: it still computes the comparison, then returns true.
        source.replacen(REVIEW_ACK_DIALOG, &discard_answer(REVIEW_ACK_DIALOG), 1),
        // Windows review dialog discards its answer: it still computes the comparison, then returns true.
        source.replacen(
            REVIEW_ACK_DIALOG_WINDOWS,
            &discard_answer(REVIEW_ACK_DIALOG_WINDOWS),
            1,
        ),
    ] {
        assert_ne!(broken, source);
        assert!(!dialog_answer_problems(&broken).is_empty());
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
