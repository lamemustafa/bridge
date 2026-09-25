//! Independent local approval. The model never supplies an approval boolean.
use super::agent_read_request::AgentReadRequest;
use bridge_tally_core::TallyDate;
use bridge_tally_protocol::{
    outstandings_shared::DateBoundaryProfile, StandardLedgerCatalogBinding,
};
use std::{io::Read, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;

const MAX_PREVIEW_BYTES: usize = 8_000;
#[cfg(not(windows))]
const POST_LABEL: &str = "Post voucher";
/// The review dialog's positive button, as the person sees it. Windows shows
/// a Yes/No/Cancel message box, which has no custom labels.
#[cfg(not(windows))]
pub(crate) const REVIEW_BUTTON: &str = "I reviewed it";
#[cfg(windows)]
pub(crate) const REVIEW_BUTTON: &str = "Yes";
/// What the review dialog's subprocess prints, followed by the nonce it was
/// given, when and only when the person chose the positive button.
const REVIEW_TOKEN_PREFIX: &str = "bridge-review-acknowledged:";
/// The same for the post dialog (#635). Distinct from the review's, so a
/// subprocess in one mode can never answer the other.
const POST_TOKEN_PREFIX: &str = "bridge-post-approved:";

#[derive(Clone)]
pub(crate) struct ApprovedImport {
    xml: String,
    voucher_date: TallyDate,
    verification_request: AgentReadRequest,
    ledger_catalogue_request: AgentReadRequest,
    ledger_binding: StandardLedgerCatalogBinding,
    /// The group collection, for a Payment, Receipt or Contra: its legs'
    /// classification is re-derived from it inside the queue. A Journal has
    /// none, so it adds no group read to the queue.
    group_collection_request: Option<AgentReadRequest>,
    /// The company's Currency masters, re-read inside the queue: a post goes
    /// only into a book with exactly one (bridge#551).
    currency_request: AgentReadRequest,
    /// The all-company change marks, read last before the POST to confirm the
    /// aim and again right after it to see where the voucher went (#574).
    company_marks_request: AgentReadRequest,
}

/// What the queue read for the last admission before the POST.
pub(crate) struct QueuedAdmission<'a> {
    pub(crate) first: &'a str,
    pub(crate) second: &'a str,
    pub(crate) catalogue: &'a str,
    pub(crate) groups: Option<&'a str>,
    pub(crate) currencies: &'a str,
    /// The all-company marks read as the binding reads began (#239).
    pub(crate) company_marks_at_binding: &'a str,
    pub(crate) company_marks: &'a str,
    pub(crate) ledger_binding: &'a StandardLedgerCatalogBinding,
}

impl ApprovedImport {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn confirm(
        xml: String,
        preview: &str,
        voucher_date: TallyDate,
        verification_request: AgentReadRequest,
        ledger_catalogue_request: AgentReadRequest,
        ledger_binding: StandardLedgerCatalogBinding,
        group_collection_request: Option<AgentReadRequest>,
        currency_request: AgentReadRequest,
        company_marks_request: AgentReadRequest,
    ) -> Result<Self, String> {
        approve(preview).await?;
        Ok(Self {
            xml,
            voucher_date,
            verification_request,
            ledger_catalogue_request,
            ledger_binding,
            group_collection_request,
            currency_request,
            company_marks_request,
        })
    }

    pub(super) fn xml(&self) -> &str {
        &self.xml
    }

    pub(super) fn verification_request(&self) -> AgentReadRequest {
        self.verification_request.clone()
    }

    pub(super) fn ledger_catalogue_request(&self) -> AgentReadRequest {
        self.ledger_catalogue_request.clone()
    }

    pub(super) fn ledger_binding(&self) -> &StandardLedgerCatalogBinding {
        &self.ledger_binding
    }

    pub(super) fn group_collection_request(&self) -> Option<AgentReadRequest> {
        self.group_collection_request.clone()
    }

    pub(super) fn currency_request(&self) -> AgentReadRequest {
        self.currency_request.clone()
    }

    pub(super) fn company_marks_request(&self) -> AgentReadRequest {
        self.company_marks_request.clone()
    }

    /// Recheck the operator-approved dates after the endpoint queue admits this
    /// request. The observed product/mode can change while native approval waits.
    pub(super) fn require_boundary_profile(
        &self,
        profile: DateBoundaryProfile,
    ) -> Result<(), ApprovedImportAdmissionError> {
        if profile.accepts_boundary(&self.voucher_date) {
            Ok(())
        } else {
            Err(ApprovedImportAdmissionError::EducationVoucherDateUnsupported)
        }
    }

    #[cfg(test)]
    pub(super) fn approved_for_test(
        xml: String,
        voucher_date: TallyDate,
        ledger_catalogue_request: AgentReadRequest,
        ledger_binding: StandardLedgerCatalogBinding,
        currency_request: AgentReadRequest,
        company_marks_request: AgentReadRequest,
    ) -> Self {
        // Carries the seam marker so the shipped-binary scan also covers this
        // bypass (bridge#583).
        std::hint::black_box(test_seam::SEAM_MARKER);
        Self {
            xml,
            voucher_date,
            verification_request: AgentReadRequest::parse(
                bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::CompanyListV2.render(),
            )
            .expect("static read profile is admitted"),
            ledger_catalogue_request,
            ledger_binding,
            group_collection_request: None,
            currency_request,
            company_marks_request,
        }
    }
}

/// A person's answer to the review dialog for a doubted post (#239): that
/// they checked the voucher in Tally. It changes nothing in Tally and
/// authorises no post: it is a different type from [`ApprovedImport`], built
/// only by [`ReviewAcknowledged::confirm`], and nothing converts one into the
/// other.
#[must_use]
pub(crate) struct ReviewAcknowledged(());

impl ReviewAcknowledged {
    pub(crate) async fn confirm(preview: &str) -> Result<Self, String> {
        approve_review(preview).await?;
        Ok(Self(()))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ApprovedImportAdmissionError {
    #[error("education_voucher_date_unsupported")]
    EducationVoucherDateUnsupported,
    #[error("import_preexisting_identity")]
    PreexistingIdentity,
    #[error("import_masters_changed")]
    LedgerIdentityChanged,
    /// A Payment, Receipt or Contra leg no longer classifies as it did when
    /// approved: a ledger or one of its groups moved (bridge#466 follow-up).
    #[error("import_bank_classification_changed")]
    BankClassificationChanged,
    /// A named ledger now folds equal to another live ledger, which Tally's
    /// import lookup could take for it (bridge#626).
    #[error("ledger_has_folded_twin")]
    LedgerFoldedTwin,
    /// A bank voucher reached the queue without its group read, or a Journal
    /// with one: a wiring fault, refused before any request is sent.
    #[error("import_post_admission_inconsistent")]
    AdmissionInconsistent,
    /// The snapshot sent last before the POST no longer shows exactly one
    /// loaded company with the target's GUID and name, or shows another loaded
    /// company sharing its name (#574).
    #[error("post_company_scope_changed")]
    CompanyScopeChanged,
    /// That snapshot could not be read, so the aim cannot be confirmed.
    #[error("post_company_scope_unconfirmed")]
    CompanyScopeUnconfirmed,
    /// The company defines more than one Currency master. Bridge's amounts are
    /// plain base-currency figures, and which master is the base cannot be
    /// identified yet (bridge#601), so no leg can be shown to be in it
    /// (bridge#551). Carries every master's NAME, for the refusal to name.
    #[error("import_multi_currency_unsupported")]
    MultiCurrencyBook { currencies: Vec<String> },
    /// The company's Currency masters read as none, or the response does not
    /// parse (a master without a NAME does not).
    #[error("import_base_currency_undetermined")]
    BaseCurrencyUndetermined,
    /// The target's master mark (ALTMSTID) moved between the snapshot taken as
    /// the queue's binding reads began and the aim snapshot read last before
    /// the POST: a master changed after the catalogue re-read (bridge#239).
    #[error("post_masters_moved")]
    MastersMoved,
    /// That comparison could not be made: the first snapshot could not be
    /// read, or either did not hold exactly one row for the target.
    #[error("post_masters_unconfirmed")]
    MastersUnconfirmed,
    /// The ledger catalogue the queue re-read could not be parsed, so the
    /// approved binding cannot be rechecked. Raised only by that recheck,
    /// before the intent or the POST (bridge#641). The source is the typed,
    /// data-free cause the refusal carries.
    #[error("post_catalogue_unreadable")]
    CatalogueUnreadable(#[source] bridge_tally_protocol::StandardLedgerCatalogError),
}

/// The native approval every real post goes through. Outside this crate's own
/// unit tests it is exactly [`confirm`]: nothing else exists to answer it.
#[cfg(not(test))]
use confirm as approve;

#[cfg(test)]
use test_seam::approve;

/// The native review dialog every acknowledgement goes through, gated exactly
/// as [`approve`] is.
#[cfg(not(test))]
use confirm_review as approve_review;

#[cfg(test)]
use test_seam::approve_review;

/// A scripted answer to the native approval, for this crate's unit tests only
/// (bridge#583). It is compiled only under bare `cfg(test)`, which Cargo sets
/// for no shipped build and no feature, variable or flag can set at runtime;
/// `tests/approval_seam_gate.rs` holds it to that, and
/// `scripts/check-no-test-seam.mjs` proves its marker is absent from every
/// shipped executable.
#[cfg(test)]
pub(crate) mod test_seam {
    use std::sync::{Arc, Mutex};

    /// Present in any binary this module is compiled into, and in no other.
    pub(crate) const SEAM_MARKER: &str = "bridge-test-approval-seam-5f1c9e7a";

    /// What a test decided, and every preview the post path asked it about.
    #[derive(Clone)]
    pub(crate) struct ScriptedApproval {
        approve: bool,
        previews: Arc<Mutex<Vec<String>>>,
        /// Every preview the review dialog was asked about, kept apart from
        /// the post dialog's so a test can tell which dialog a person saw.
        reviews: Arc<Mutex<Vec<String>>>,
        /// Run while the approval is pending, as something else changing the
        /// book or the journal while an operator reads the dialog would.
        while_pending: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    impl ScriptedApproval {
        pub(crate) fn approving() -> Self {
            Self::new(true)
        }

        pub(crate) fn declining() -> Self {
            Self::new(false)
        }

        /// Approves, after running `while_pending` as the dialog would wait.
        pub(crate) fn approving_after(while_pending: impl Fn() + Send + Sync + 'static) -> Self {
            Self {
                while_pending: Some(Arc::new(while_pending)),
                ..Self::new(true)
            }
        }

        fn new(approve: bool) -> Self {
            Self {
                approve,
                previews: Arc::default(),
                reviews: Arc::default(),
                while_pending: None,
            }
        }

        pub(crate) fn previews(&self) -> Vec<String> {
            self.previews.lock().unwrap().clone()
        }

        pub(crate) fn reviews(&self) -> Vec<String> {
            self.reviews.lock().unwrap().clone()
        }
    }

    tokio::task_local! {
        /// The decision for the one task a test scopes it to. A task-local
        /// does not cross `tokio::spawn`: an approval asked from a spawned
        /// task finds no decision and is declined, which fails safe.
        pub(crate) static SCRIPTED_APPROVAL: ScriptedApproval;
    }

    /// The test-build approval. Unscoped, it declines at once and starts no
    /// process, so no test can reach a real dialog or approve by default.
    pub(super) async fn approve(preview: &str) -> Result<(), String> {
        let decision = SCRIPTED_APPROVAL
            .try_with(|scripted| {
                scripted.previews.lock().unwrap().push(preview.to_string());
                if let Some(while_pending) = &scripted.while_pending {
                    while_pending();
                }
                scripted.approve
            })
            .unwrap_or(false);
        std::hint::black_box(SEAM_MARKER);
        if decision {
            Ok(())
        } else {
            Err("import_approval_declined".into())
        }
    }

    /// The test-build review dialog, scripted by the same decision and
    /// declining the same way when unscoped.
    pub(super) async fn approve_review(preview: &str) -> Result<(), String> {
        let decision = SCRIPTED_APPROVAL
            .try_with(|scripted| {
                scripted.reviews.lock().unwrap().push(preview.to_string());
                if let Some(while_pending) = &scripted.while_pending {
                    while_pending();
                }
                scripted.approve
            })
            .unwrap_or(false);
        std::hint::black_box(SEAM_MARKER);
        if decision {
            Ok(())
        } else {
            Err("ack_review_declined".into())
        }
    }

    /// The real approval keeps its own preview limit; the scripted one does
    /// not repeat it, so the limit is held here, on the real path, where it
    /// refuses before any process is started.
    #[tokio::test]
    async fn the_real_approval_refuses_an_oversized_preview_before_starting_a_process() {
        let oversized = "x".repeat(super::MAX_PREVIEW_BYTES + 1);
        assert_eq!(
            super::confirm(&oversized).await,
            Err("import_review_too_large".to_string())
        );
    }

    /// The same for the review dialog (#239), which has its own limit code.
    #[tokio::test]
    async fn the_real_review_refuses_an_oversized_preview_before_starting_a_process() {
        let oversized = "x".repeat(super::MAX_PREVIEW_BYTES + 1);
        assert_eq!(
            super::confirm_review(&oversized).await,
            Err("ack_review_too_large".to_string())
        );
    }

    /// A script standing in for a dialog subprocess.
    ///
    /// Each call writes a file of its own. Rewriting one path that another
    /// thread may be executing can fail on Linux with "text file busy"
    /// (ETXTBSY), which would surface as `import_approval_unavailable`.
    #[cfg(unix)]
    fn stub(directory: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = directory.join(format!("stub-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Only the token echoing this call's nonce is an answer. An older build
    /// that ignores `--confirm-review` and exits 0, a process that echoes its
    /// input, and a token for another nonce are refused; the right token is
    /// accepted whatever the exit status, which is not an answer.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_review_is_answered_only_by_the_token_for_its_nonce() {
        let directory = tempfile::tempdir().unwrap();
        let answers = [
            ("an older build exits 0", "cat > /dev/null; exit 0"),
            ("an echo of the input", "cat"),
            (
                "a token for another nonce",
                "cat > /dev/null; echo bridge-review-acknowledged:00000000-0000-4000-8000-000000000000",
            ),
            (
                "the post dialog's token for this nonce",
                "read nonce; printf 'bridge-post-approved:%s\\n' \"$nonce\"; cat > /dev/null",
            ),
            (
                "the token, but a failing exit",
                "read nonce; printf 'bridge-review-acknowledged:%s\\n' \"$nonce\"; cat > /dev/null; exit 1",
            ),
        ];
        for (name, body) in answers {
            let result = super::confirm_review_with(&stub(directory.path(), body), "Review").await;
            let expected = if name == "the token, but a failing exit" {
                Ok(())
            } else {
                Err("ack_review_declined".to_string())
            };
            assert_eq!(result, expected, "{name}");
        }
        // The control: the token for this call's nonce is accepted.
        let echoes_token = stub(
            directory.path(),
            "read nonce; printf 'bridge-review-acknowledged:%s\\n' \"$nonce\"; cat > /dev/null",
        );
        assert_eq!(
            super::confirm_review_with(&echoes_token, "Review").await,
            Ok(())
        );
    }

    /// The post dialog is answered only by the token echoing this call's
    /// nonce, and a clean exit (#635). An executable that ignores
    /// `--confirm-journal` and exits 0, one that echoes its input, a token for
    /// another nonce, the review dialog's token, and the right token with a
    /// failing exit are all refused, never approved.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_post_is_approved_only_by_the_token_for_its_nonce() {
        let directory = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("an executable ignoring the flag exits 0", "cat > /dev/null; exit 0"),
            ("an echo of the input", "cat"),
            (
                "a token for another nonce",
                "cat > /dev/null; echo bridge-post-approved:00000000-0000-4000-8000-000000000000",
            ),
            (
                "the review dialog's token for this nonce",
                "read nonce; printf 'bridge-review-acknowledged:%s\\n' \"$nonce\"; cat > /dev/null",
            ),
            (
                "the token, but a failing exit",
                "read nonce; printf 'bridge-post-approved:%s\\n' \"$nonce\"; cat > /dev/null; exit 1",
            ),
            // The answer is matched byte for byte, so any stray output, such
            // as a log line, declines. That is fail-closed on purpose: do not
            // trim or search the output to "fix" it.
            (
                "a log line, then the token",
                "read nonce; echo starting; printf 'bridge-post-approved:%s\\n' \"$nonce\"; cat > /dev/null",
            ),
            (
                "the token, then more output",
                "read nonce; printf 'bridge-post-approved:%s\\nmore\\n' \"$nonce\"; cat > /dev/null",
            ),
            (
                "the token without its newline",
                "read nonce; printf 'bridge-post-approved:%s' \"$nonce\"; cat > /dev/null",
            ),
        ] {
            assert_eq!(
                super::confirm_with(&stub(directory.path(), body), "Post").await,
                Err("import_approval_declined".to_string()),
                "{name}"
            );
        }
        // The control: the token for this call's nonce, then a clean exit.
        let approves = stub(
            directory.path(),
            "read nonce; printf 'bridge-post-approved:%s\\n' \"$nonce\"; cat > /dev/null",
        );
        assert_eq!(super::confirm_with(&approves, "Post").await, Ok(()));
    }

    /// Each call sends a nonce of its own: a stub that answers every call
    /// with the token for the nonce it read sees a different one each time.
    #[cfg(unix)]
    #[tokio::test]
    async fn each_post_dialog_gets_a_fresh_nonce() {
        let directory = tempfile::tempdir().unwrap();
        let seen = directory.path().join("nonces");
        let approves = stub(
            directory.path(),
            &format!(
                "read nonce; echo \"$nonce\" >> '{}'; printf 'bridge-post-approved:%s\\n' \"$nonce\"; cat > /dev/null",
                seen.display()
            ),
        );
        for _ in 0..2 {
            assert_eq!(super::confirm_with(&approves, "Post").await, Ok(()));
        }
        let nonces = std::fs::read_to_string(&seen).unwrap();
        let nonces = nonces.lines().collect::<Vec<_>>();
        assert_eq!(nonces.len(), 2);
        assert!(nonces
            .iter()
            .all(|nonce| uuid::Uuid::parse_str(nonce).is_ok()));
        assert_ne!(nonces[0], nonces[1]);
    }

    /// The dialog subprocesses show a dialog only for input of the shape the
    /// parent sends: a nonce line, then a preview within the limit.
    #[test]
    fn a_dialog_subprocess_admits_only_the_parents_input_shape() {
        let nonce = "9c8d8de4-c06c-447b-8309-60ba702bf663";
        assert_eq!(
            super::dialog_input(&format!("{nonce}\nReview")),
            Some((nonce, "Review"))
        );
        let oversized = "x".repeat(super::MAX_PREVIEW_BYTES + 1);
        for input in [
            "Review".to_string(),
            "not-a-nonce\nReview".to_string(),
            format!("{nonce}\n"),
            format!("{nonce}\nRe\0view"),
            format!("{nonce}\n{oversized}"),
        ] {
            assert_eq!(super::dialog_input(&input), None, "{input:.40}");
        }
    }
}

/// The post dialog. An exit status alone is not an answer (#635): an
/// executable that does not know `--confirm-journal`, such as a build of
/// `bridge_mcp` older than the flag left at `current_exe()`, starts the MCP
/// server instead, reads the preview as input and exits 0 at its end. So the
/// parent sends a fresh nonce and requires the token that echoes it, which
/// only this dialog's positive button prints, and a clean exit as well.
async fn confirm(preview: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| "import_approval_unavailable")?;
    confirm_with(&executable, preview).await
}

async fn confirm_with(executable: &std::path::Path, preview: &str) -> Result<(), String> {
    if preview.len() > MAX_PREVIEW_BYTES {
        return Err("import_review_too_large".into());
    }
    match nonce_bound_dialog(executable, "--confirm-journal", POST_TOKEN_PREFIX, preview).await {
        Ok(answer) if answer.token_matched && answer.exited_cleanly => Ok(()),
        Ok(_) => Err("import_approval_declined".into()),
        Err(DialogFailure::Unavailable) => Err("import_approval_unavailable".into()),
        Err(DialogFailure::TimedOut) => Err("import_approval_timed_out".into()),
    }
}

/// The review dialog for a doubted post (#239): its own subprocess mode, so
/// its title and button never read as approving a post. It is answered by
/// the token alone, as the post dialog is by the token and a clean exit.
async fn confirm_review(preview: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|_| "ack_review_unavailable")?;
    confirm_review_with(&executable, preview).await
}

async fn confirm_review_with(executable: &std::path::Path, preview: &str) -> Result<(), String> {
    if preview.len() > MAX_PREVIEW_BYTES {
        return Err("ack_review_too_large".into());
    }
    match nonce_bound_dialog(executable, "--confirm-review", REVIEW_TOKEN_PREFIX, preview).await {
        Ok(answer) if answer.token_matched => Ok(()),
        Ok(_) => Err("ack_review_declined".into()),
        Err(DialogFailure::Unavailable) => Err("ack_review_unavailable".into()),
        Err(DialogFailure::TimedOut) => Err("ack_review_timed_out".into()),
    }
}

/// What a dialog subprocess answered: whether it printed exactly the token
/// for this call's nonce, and whether it exited cleanly.
struct DialogAnswer {
    token_matched: bool,
    exited_cleanly: bool,
}

enum DialogFailure {
    Unavailable,
    TimedOut,
}

/// Show `preview` in the native dialog `mode` selects, in a subprocess of
/// `executable`, and read its answer. The parent sends a fresh nonce line and
/// then the preview; the child prints `prefix` and that nonce only when the
/// person chose the positive button.
async fn nonce_bound_dialog(
    executable: &std::path::Path,
    mode: &str,
    prefix: &str,
    preview: &str,
) -> Result<DialogAnswer, DialogFailure> {
    let nonce = uuid::Uuid::new_v4().to_string();
    let mut child = tokio::process::Command::new(executable)
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| DialogFailure::Unavailable)?;
    let (answer, status) = tokio::time::timeout(Duration::from_secs(120), async {
        let mut input = child.stdin.take().ok_or(DialogFailure::Unavailable)?;
        input
            .write_all(format!("{nonce}\n{preview}").as_bytes())
            .await
            .map_err(|_| DialogFailure::Unavailable)?;
        drop(input);
        // The answer is one short line, so at most 128 bytes are read. A
        // child that writes more without exiting is waited on until the
        // timeout, then killed on drop: bounded on purpose.
        let mut output = child.stdout.take().ok_or(DialogFailure::Unavailable)?;
        let mut answer = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(
            &mut tokio::io::AsyncReadExt::take(&mut output, 128),
            &mut answer,
        )
        .await
        .map_err(|_| DialogFailure::Unavailable)?;
        let status = child.wait().await.map_err(|_| DialogFailure::Unavailable)?;
        Ok::<_, DialogFailure>((answer, status))
    })
    .await
    .map_err(|_| DialogFailure::TimedOut)??;
    Ok(DialogAnswer {
        token_matched: answer == dialog_token(prefix, &nonce).as_bytes(),
        exited_cleanly: status.success(),
    })
}

fn dialog_token(prefix: &str, nonce: &str) -> String {
    format!("{prefix}{nonce}\n")
}

/// Entry point for the same executable's private native-dialog subprocess.
/// Runs before Tokio starts, because macOS dialogs require the main thread.
/// It prints the token for the nonce it was given only when the person chose
/// to post (#635); the parent trusts nothing else.
pub fn run_confirmation() -> bool {
    answer_with_token(POST_TOKEN_PREFIX, show_review)
}

/// Entry point for the review dialog's subprocess (#239), under the same rules.
pub fn run_review_confirmation() -> bool {
    answer_with_token(REVIEW_TOKEN_PREFIX, show_review_acknowledgement)
}

/// Read the parent's nonce line and preview, show `dialog`, and print the
/// token for that nonce only when it returns true.
fn answer_with_token(prefix: &str, dialog: fn(&str) -> bool) -> bool {
    let mut input = String::new();
    if std::io::stdin()
        .take(MAX_PREVIEW_BYTES as u64 + 64)
        .read_to_string(&mut input)
        .is_err()
    {
        return false;
    }
    let Some((nonce, preview)) = dialog_input(&input) else {
        return false;
    };
    if !dialog(preview) {
        return false;
    }
    use std::io::Write as _;
    let mut stdout = std::io::stdout();
    stdout
        .write_all(dialog_token(prefix, nonce).as_bytes())
        .is_ok()
        && stdout.flush().is_ok()
}

/// The nonce line and the preview, when the input has the shape the parent
/// sends; `None` shows no dialog.
fn dialog_input(input: &str) -> Option<(&str, &str)> {
    let (nonce, preview) = input.split_once('\n')?;
    (uuid::Uuid::parse_str(nonce).is_ok()
        && !preview.contains('\0')
        && !preview.is_empty()
        && preview.len() <= MAX_PREVIEW_BYTES)
        .then_some((nonce, preview))
}

/// The acknowledgement dialog. It posts nothing, so neither its title nor its
/// button may read as approving a post.
#[cfg(not(windows))]
fn show_review_acknowledgement(preview: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title("Bridge — record that you reviewed one voucher")
        .set_description(preview)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            "Cancel".into(),
            REVIEW_BUTTON.into(),
        ))
        .show()
        == rfd::MessageDialogResult::Custom(REVIEW_BUTTON.into())
}

#[cfg(windows)]
fn show_review_acknowledgement(preview: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNOCANCEL,
    };
    let text: Vec<u16> = preview.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Bridge — record that you reviewed this voucher?"
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
}

#[cfg(not(windows))]
fn show_review(preview: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title("Bridge — approve one voucher")
        .set_description(preview)
        .set_level(rfd::MessageLevel::Warning)
        // The Cancel label supplies the native Escape action. Posting requires
        // the explicitly matched positive button; Return may leave this dialog open.
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            "Cancel".into(),
            POST_LABEL.into(),
        ))
        .show()
        == rfd::MessageDialogResult::Custom(POST_LABEL.into())
}

#[cfg(windows)]
fn show_review(preview: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_SETFOREGROUND, MB_YESNOCANCEL,
    };
    // rfd without common-controls-v6 discards custom labels. Use the existing
    // Win32 dependency so No is the default and Escape/close remain Cancel.
    let text: Vec<u16> = preview.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Bridge — post this voucher?"
        .encode_utf16()
        .chain(Some(0))
        .collect();
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
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("import_company_scope_ambiguous")]
pub(crate) struct AmbiguousImportCompany;

pub(super) fn require_unique_company_scope(
    companies: &[bridge_tally_protocol::TallyCompany],
    identity: &super::VerifiedCompanyIdentity,
) -> Result<(), AmbiguousImportCompany> {
    let count = companies
        .iter()
        .filter(|company| {
            company
                .name
                .trim()
                .eq_ignore_ascii_case(identity.display_name().trim())
        })
        .count();
    if count == 1 {
        Ok(())
    } else {
        Err(AmbiguousImportCompany)
    }
}

#[cfg(test)]
#[path = "approved_import_tests.rs"]
mod tests;
