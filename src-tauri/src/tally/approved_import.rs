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
}

/// The native approval every real post goes through. Outside this crate's own
/// unit tests it is exactly [`confirm`]: nothing else exists to answer it.
#[cfg(not(test))]
use confirm as approve;

#[cfg(test)]
use test_seam::approve;

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
                while_pending: None,
            }
        }

        pub(crate) fn previews(&self) -> Vec<String> {
            self.previews.lock().unwrap().clone()
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
}

async fn confirm(preview: &str) -> Result<(), String> {
    if preview.len() > MAX_PREVIEW_BYTES {
        return Err("import_review_too_large".into());
    }
    let executable = std::env::current_exe().map_err(|_| "import_approval_unavailable")?;
    let mut child = tokio::process::Command::new(executable)
        .arg("--confirm-journal")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "import_approval_unavailable")?;
    let result = tokio::time::timeout(Duration::from_secs(120), async {
        let mut input = child.stdin.take().ok_or("import_approval_unavailable")?;
        input
            .write_all(preview.as_bytes())
            .await
            .map_err(|_| "import_approval_unavailable")?;
        drop(input);
        child
            .wait()
            .await
            .map_err(|_| "import_approval_unavailable")
    })
    .await
    .map_err(|_| "import_approval_timed_out")??;
    if result.success() {
        Ok(())
    } else {
        Err("import_approval_declined".into())
    }
}

/// Entry point for the same executable's private native-dialog subprocess.
/// Runs before Tokio starts, because macOS dialogs require the main thread.
pub fn run_confirmation() -> bool {
    let mut preview = String::new();
    if std::io::stdin()
        .take(MAX_PREVIEW_BYTES as u64 + 1)
        .read_to_string(&mut preview)
        .is_err()
        || preview.contains('\0')
        || preview.is_empty()
        || preview.len() > MAX_PREVIEW_BYTES
    {
        return false;
    }
    show_review(&preview)
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
