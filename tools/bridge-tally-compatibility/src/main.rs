use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

use bridge_tally_compatibility::{
    enforce_support_gate, format_gate_success, now_unix_ms, parse_artifact, render_claim_matrix,
    safe_error_code, verify_claim_matrix_markdown, CompatibilitySurfaceManifest,
    LiveCompatibilityReceipt, ReviewedEvidenceAttestation, SupportClaimsManifest,
    TrustedEvidenceKeys, MAX_ARTIFACT_BYTES,
};

fn main() -> ExitCode {
    match run() {
        Ok(output) => match emit_output(output) {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => {
                eprintln!("bridge_tally_compatibility_failed:{code}");
                ExitCode::FAILURE
            }
        },
        Err(code) => {
            eprintln!("bridge_tally_compatibility_failed:{code}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
struct CommandOutput {
    message: String,
    output_path: Option<PathBuf>,
}

impl CommandOutput {
    fn stdout(message: String) -> Self {
        Self {
            message,
            output_path: None,
        }
    }

    fn output_file(message: String, output_path: Option<PathBuf>) -> Self {
        Self {
            message,
            output_path,
        }
    }
}

fn emit_output(output: CommandOutput) -> Result<(), &'static str> {
    if let Some(path) = output.output_path {
        write_output_atomically(&path, &output.message)
    } else {
        println!("{}", output.message);
        Ok(())
    }
}

fn run() -> Result<CommandOutput, &'static str> {
    run_from_args(std::env::args().skip(1))
}

fn run_from_args(mut args: impl Iterator<Item = String>) -> Result<CommandOutput, &'static str> {
    match args.next().as_deref() {
        Some("validate-receipt") => {
            let path = one_path(&mut args)?;
            let bytes = read_bounded(&path)?;
            LiveCompatibilityReceipt::from_json(&bytes).map_err(|error| safe_error_code(&error))?;
            Ok(CommandOutput::stdout("compatibility_receipt_valid".to_string()))
        }
        Some("gate") => {
            let support = next_path(&mut args, "missing_support_manifest")?;
            let surface = next_path(&mut args, "missing_surface_manifest")?;
            let trust = next_path(&mut args, "missing_trust_manifest")?;
            let evidence = next_path(&mut args, "missing_evidence_directory")?;
            let root = next_path(&mut args, "missing_repository_root")?;
            if args.next().is_some() {
                return Err("unexpected_argument");
            }
            gate_command(&support, &surface, &trust, &evidence, &root).map(CommandOutput::stdout)
        }
        Some("seal-surface") => {
            let path = next_path(&mut args, "missing_path")?;
            let output_path = optional_output_path(&mut args)?;
            let draft = parse_artifact::<CompatibilitySurfaceManifest>(&read_bounded(&path)?)
                .map_err(|error| safe_error_code(&error))?;
            let sealed = draft.seal().map_err(|error| safe_error_code(&error))?;
            let bytes = sealed
                .to_pretty_json()
                .map_err(|error| safe_error_code(&error))?;
            let message = String::from_utf8(bytes).map_err(|_| "serialization_failed")?;
            Ok(CommandOutput::output_file(message, output_path))
        }
        Some("rehash-surface") => {
            let surface = next_path(&mut args, "missing_surface_manifest")?;
            let repository_root = next_path(&mut args, "missing_repository_root")?;
            let output_path = optional_output_path(&mut args)?;
            rehash_surface_command(&surface, &repository_root)
                .map(|message| CommandOutput::output_file(message, output_path))
        }
        Some("repoint-matrix") => {
            let matrix = next_path(&mut args, "missing_support_manifest")?;
            let surface = next_path(&mut args, "missing_surface_manifest")?;
            let output_path = optional_output_path(&mut args)?;
            repoint_matrix_command(&matrix, &surface)
                .map(|message| CommandOutput::output_file(message, output_path))
        }
        Some("check-matrix-markdown") => {
            let manifest_path = next_path(&mut args, "missing_support_manifest")?;
            let markdown_path = next_path(&mut args, "missing_matrix_markdown")?;
            if args.next().is_some() {
                return Err("unexpected_argument");
            }
            let manifest = SupportClaimsManifest::from_json(&read_bounded(&manifest_path)?)
                .map_err(|error| safe_error_code(&error))?;
            let markdown = fs::read(&markdown_path).map_err(|_| "matrix_markdown_unavailable")?;
            verify_claim_matrix_markdown(&manifest, &markdown)
                .map_err(|error| safe_error_code(&error))?;
            Ok(CommandOutput::stdout(
                "compatibility_matrix_markdown_current".to_string(),
            ))
        }
        Some("render-matrix") => {
            let path = one_path(&mut args)?;
            let manifest = SupportClaimsManifest::from_json(&read_bounded(&path)?)
                .map_err(|error| safe_error_code(&error))?;
            render_claim_matrix(&manifest)
                .map(CommandOutput::stdout)
                .map_err(|error| safe_error_code(&error))
        }
        _ => Err("usage_validate_receipt_rehash_surface_seal_surface_repoint_matrix_render_or_check_matrix_markdown_or_gate"),
    }
}

fn optional_output_path(
    args: &mut impl Iterator<Item = String>,
) -> Result<Option<PathBuf>, &'static str> {
    match args.next() {
        None => Ok(None),
        Some(flag) if flag == "--output" => {
            let output_path = next_path(args, "missing_output_path")?;
            if args.next().is_some() {
                return Err("unexpected_argument");
            }
            Ok(Some(output_path))
        }
        Some(_) => Err("unexpected_argument"),
    }
}

fn write_output_atomically(output_path: &Path, contents: &str) -> Result<(), &'static str> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = create_temporary_output_file(output_path, parent)?;
    temporary
        .write_all(contents.as_bytes())
        .map_err(|_| "output_write_failed")?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| "output_sync_failed")?;
    let temporary_path = temporary.into_temp_path();
    replace_output_file(temporary_path.as_ref(), output_path)
}

#[cfg(unix)]
fn create_temporary_output_file(
    output_path: &Path,
    parent: &Path,
) -> Result<tempfile::NamedTempFile, &'static str> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = match fs::metadata(output_path) {
        Ok(metadata) => metadata.permissions(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::Permissions::from_mode(0o666)
        }
        Err(_) => return Err("output_metadata_unavailable"),
    };
    tempfile::Builder::new()
        .permissions(permissions)
        .tempfile_in(parent)
        .map_err(|_| "output_directory_unavailable")
}

#[cfg(not(unix))]
fn create_temporary_output_file(
    _output_path: &Path,
    parent: &Path,
) -> Result<tempfile::NamedTempFile, &'static str> {
    tempfile::NamedTempFile::new_in(parent).map_err(|_| "output_directory_unavailable")
}

#[cfg(not(windows))]
fn replace_output_file(temporary_path: &Path, output_path: &Path) -> Result<(), &'static str> {
    fs::rename(temporary_path, output_path).map_err(|_| "output_replace_failed")
}

#[cfg(windows)]
fn replace_output_file(temporary_path: &Path, output_path: &Path) -> Result<(), &'static str> {
    if !output_path.exists() {
        return fs::rename(temporary_path, output_path).map_err(|_| "output_replace_failed");
    }

    let output_wide = windows_path(output_path)?;
    let temporary_wide = windows_path(temporary_path)?;
    // The temporary file is created in the output directory, as ReplaceFileW requires.
    let replaced = unsafe {
        ReplaceFileW(
            output_wide.as_ptr(),
            temporary_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced == 0 {
        return Err("output_replace_failed");
    }
    Ok(())
}

#[cfg(windows)]
fn windows_path(path: &Path) -> Result<Vec<u16>, &'static str> {
    use std::os::windows::ffi::OsStrExt;

    Ok(path.as_os_str().encode_wide().chain([0]).collect())
}

#[cfg(windows)]
#[link(name = "Kernel32")]
extern "system" {
    fn ReplaceFileW(
        replaced_file_name: *const u16,
        replacement_file_name: *const u16,
        backup_file_name: *const u16,
        replace_flags: u32,
        exclude: *mut std::ffi::c_void,
        reserved: *mut std::ffi::c_void,
    ) -> i32;
}

fn rehash_surface_command(
    surface_path: &Path,
    repository_root: &Path,
) -> Result<String, &'static str> {
    let surface = CompatibilitySurfaceManifest::from_json(&read_bounded(surface_path)?)
        .map_err(|error| safe_error_code(&error))?;
    let (rehashed, changed) = surface
        .rehash_files(repository_root)
        .map_err(|error| safe_error_code(&error))?;
    let json = serde_json::to_string_pretty(&rehashed).map_err(|_| "serialization_failed")?;
    eprintln!("rehash_surface_changed:{changed}");
    Ok(json)
}

fn repoint_matrix_command(matrix_path: &Path, surface_path: &Path) -> Result<String, &'static str> {
    let matrix = SupportClaimsManifest::from_json(&read_bounded(matrix_path)?)
        .map_err(|error| safe_error_code(&error))?;
    let surface = CompatibilitySurfaceManifest::from_json(&read_bounded(surface_path)?)
        .map_err(|error| safe_error_code(&error))?;
    let bytes = matrix
        .repoint_surface(&surface)
        .and_then(|repointed| repointed.to_pretty_json())
        .map_err(|error| safe_error_code(&error))?;
    String::from_utf8(bytes).map_err(|_| "serialization_failed")
}

fn gate_command(
    support_path: &Path,
    surface_path: &Path,
    trust_path: &Path,
    evidence_dir: &Path,
    repository_root: &Path,
) -> Result<String, &'static str> {
    let support = SupportClaimsManifest::from_json(&read_bounded(support_path)?)
        .map_err(|error| safe_error_code(&error))?;
    let surface = CompatibilitySurfaceManifest::from_json(&read_bounded(surface_path)?)
        .map_err(|error| safe_error_code(&error))?;
    let trust = TrustedEvidenceKeys::from_json(&read_bounded(trust_path)?)
        .map_err(|error| safe_error_code(&error))?;
    let mut receipts = Vec::new();
    let mut attestations = Vec::new();
    if evidence_dir.exists() {
        let entries = fs::read_dir(evidence_dir).map_err(|_| "evidence_directory_unavailable")?;
        for (index, entry) in entries.enumerate() {
            if index >= 128 {
                return Err("evidence_file_limit");
            }
            let path = entry.map_err(|_| "evidence_directory_unavailable")?.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or("evidence_filename_invalid")?;
            if name.ends_with(".receipt.json") {
                receipts.push(
                    LiveCompatibilityReceipt::from_json(&read_bounded(&path)?)
                        .map_err(|error| safe_error_code(&error))?,
                );
            } else if name.ends_with(".attestation.json") {
                attestations.push(
                    parse_artifact::<ReviewedEvidenceAttestation>(&read_bounded(&path)?)
                        .map_err(|error| safe_error_code(&error))?,
                );
            } else if name != "README.md" && name != ".gitkeep" {
                return Err("evidence_filename_invalid");
            }
        }
    }
    let report = enforce_support_gate(
        &support,
        &surface,
        &trust,
        &receipts,
        &attestations,
        repository_root,
        now_unix_ms().map_err(|error| safe_error_code(&error))?,
    )
    .map_err(|error| safe_error_code(&error))?;
    Ok(format_gate_success(&report))
}

fn one_path(args: &mut impl Iterator<Item = String>) -> Result<PathBuf, &'static str> {
    let path = next_path(args, "missing_path")?;
    if args.next().is_some() {
        return Err("unexpected_argument");
    }
    Ok(path)
}

fn next_path(
    args: &mut impl Iterator<Item = String>,
    missing: &'static str,
) -> Result<PathBuf, &'static str> {
    args.next().map(PathBuf::from).ok_or(missing)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, &'static str> {
    let metadata = fs::metadata(path).map_err(|_| "artifact_unavailable")?;
    if metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_BYTES as u64 {
        return Err("artifact_size_invalid");
    }
    fs::read(path).map_err(|_| "artifact_unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_path_is_optional_and_requires_its_flag() {
        let mut no_output = Vec::<String>::new().into_iter();
        assert_eq!(optional_output_path(&mut no_output).unwrap(), None);

        let mut output = vec!["--output".to_string(), "next.json".to_string()].into_iter();
        assert_eq!(
            optional_output_path(&mut output).unwrap(),
            Some(PathBuf::from("next.json"))
        );

        let mut missing_path = vec!["--output".to_string()].into_iter();
        assert_eq!(
            optional_output_path(&mut missing_path),
            Err("missing_output_path")
        );
    }

    #[test]
    fn output_file_is_utf8_without_bom_and_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let output_path = directory.path().join("surface.json");
        fs::write(&output_path, b"previous").unwrap();

        emit_output(CommandOutput::output_file(
            "{\"a\":1}".to_string(),
            Some(output_path.clone()),
        ))
        .unwrap();

        assert_eq!(fs::read(&output_path).unwrap(), br#"{"a":1}"#);
    }

    #[cfg(unix)]
    #[test]
    fn output_file_preserves_existing_destination_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let output_path = directory.path().join("surface.json");
        fs::write(&output_path, b"previous").unwrap();
        fs::set_permissions(&output_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        emit_output(CommandOutput::output_file(
            "{\"a\":1}".to_string(),
            Some(output_path.clone()),
        ))
        .unwrap();

        assert_eq!(
            fs::metadata(output_path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn failed_command_does_not_create_requested_output_file() {
        let directory = tempfile::tempdir().unwrap();
        let output_path = directory.path().join("surface.json");
        let missing_surface = directory.path().join("missing-surface.json");
        let error = run_from_args(
            vec![
                "rehash-surface".to_string(),
                missing_surface.display().to_string(),
                directory.path().display().to_string(),
                "--output".to_string(),
                output_path.display().to_string(),
            ]
            .into_iter(),
        )
        .unwrap_err();

        assert_eq!(error, "artifact_unavailable");
        assert!(!output_path.exists());
    }
}
