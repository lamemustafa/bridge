//! Independent local approval. The model never supplies an approval boolean.
use super::agent_read_request::AgentReadRequest;
use bridge_tally_core::TallyDate;
use bridge_tally_protocol::outstandings_shared::DateBoundaryProfile;
use std::{io::Read, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;

const MAX_PREVIEW_BYTES: usize = 8_000;
#[cfg(not(windows))]
const POST_LABEL: &str = "Post Journal";

#[derive(Clone)]
pub(crate) struct ApprovedImport {
    xml: String,
    voucher_date: TallyDate,
    verification_request: AgentReadRequest,
}

impl ApprovedImport {
    pub(crate) async fn confirm(
        xml: String,
        preview: &str,
        voucher_date: TallyDate,
        verification_request: AgentReadRequest,
    ) -> Result<Self, String> {
        confirm(preview).await?;
        Ok(Self {
            xml,
            voucher_date,
            verification_request,
        })
    }

    pub(super) fn xml(&self) -> &str {
        &self.xml
    }

    pub(super) fn verification_request(&self) -> AgentReadRequest {
        self.verification_request.clone()
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
    pub(super) fn approved_for_test(xml: String, voucher_date: TallyDate) -> Self {
        Self {
            xml,
            voucher_date,
            verification_request: AgentReadRequest::parse(
                bridge_tally_protocol::xml_read_profiles::ReadOnlyProfile::CompanyListV2.render(),
            )
            .expect("static read profile is admitted"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum ApprovedImportAdmissionError {
    #[error("education_voucher_date_unsupported")]
    EducationVoucherDateUnsupported,
    #[error("import_preexisting_identity")]
    PreexistingIdentity,
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
        .set_title("Bridge — approve one Journal")
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
    let title: Vec<u16> = "Bridge — post this Journal?"
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
mod tests {
    use super::*;
    #[test]
    fn import_name_scope_must_select_one_observed_company() {
        // Local identity-admission test. These are not claimed Tally responses.
        let original = bridge_tally_protocol::TallyCompany {
            name: "Synthetic Book".into(),
            guid: Some("00000000-0000-4000-8000-000000000001".into()),
            company_number: Some("1".into()),
            books_from: Some("20260401".into()),
        };
        let identity = super::super::VerifiedCompanyIdentity::from_observed_companies(
            original.name.clone(),
            original.guid.clone().unwrap(),
            "1".into(),
            "20260401".into(),
            std::slice::from_ref(&original),
        )
        .unwrap();
        assert!(require_unique_company_scope(std::slice::from_ref(&original), &identity).is_ok());
        for name in ["Synthetic Book", " synthetic book "] {
            let mut other = original.clone();
            other.name = name.into();
            other.guid = Some("00000000-0000-4000-8000-000000000002".into());
            assert_eq!(
                require_unique_company_scope(&[original.clone(), other], &identity),
                Err(AmbiguousImportCompany)
            );
        }
    }
}
