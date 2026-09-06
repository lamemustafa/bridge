//! Recoverable proof publication under the import admission lock.
//! Multiple file replacements are not crash-atomic. An interrupted transaction
//! retains its backups and blocks admission until its state is reconciled.
use super::*;

const TRANSACTION: &str = ".proof-publication";
const BUILD_TRANSACTION: &str = ".build-publication";

pub(super) fn require_settled(imports: &Path) -> Result<(), String> {
    for (name, error_code) in [
        (TRANSACTION, "proof_publication_recovery_required"),
        (BUILD_TRANSACTION, "import_publication_recovery_required"),
    ] {
        match fs::symlink_metadata(imports.join(name)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(error_code.into()),
        }
    }
    Ok(())
}

pub(super) fn persist_build(
    imports: &Path,
    line: &ImportLedgerLine,
    xml: &[u8],
    append_status: impl FnOnce() -> Result<(), String>,
) -> Result<Option<String>, String> {
    let path = imports.join(format!("{}.xml", line.batch_id));
    write_private(&path, xml)?;
    let transaction = imports.join(BUILD_TRANSACTION);
    let stage = (|| {
        fs::create_dir(&transaction)
            .map_err(|_| "import_publication_recovery_required".to_string())?;
        set_private_dir(&transaction)?;
        write_private(
            &transaction.join("update.json"),
            &serde_json::to_vec_pretty(line)
                .map_err(|_| "import_ledger_serialization_failed".to_string())?,
        )
    })();
    if let Err(error) = stage {
        // No ledger append has happened, but preserve any journal for recovery.
        return Err(remove_orphaned_import_file(&path, error));
    }
    match append_status() {
        Err(error) if error == "import_ledger_rollback_failed" => Ok(Some(error)),
        Err(error) => {
            let error = remove_orphaned_import_file(&path, error);
            fs::remove_dir_all(&transaction)
                .map_err(|_| "import_publication_recovery_required".to_string())?;
            Err(error)
        }
        Ok(()) => match fs::remove_dir_all(&transaction) {
            Ok(()) => Ok(None),
            Err(_) => Ok(Some("import_publication_recovery_required".into())),
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PublicationStep {
    StageJson,
    StageMarkdown,
    BackupJson,
    BackupMarkdown,
    PublishJson,
    PublishMarkdown,
    AppendStatus,
}

pub(super) fn publish_proofs(
    imports: &Path,
    update: &ImportLedgerLine,
    json: &[u8],
    markdown: &[u8],
    append_status: impl FnOnce() -> Result<(), String>,
    mut before: impl FnMut(PublicationStep) -> Result<(), String>,
) -> Result<(), String> {
    let transaction = imports.join(TRANSACTION);
    fs::create_dir(&transaction).map_err(|_| "proof_publication_recovery_required".to_string())?;
    let targets = [
        imports.join(format!("{}.proof.json", update.batch_id)),
        imports.join(format!("{}.proof.md", update.batch_id)),
    ];
    let staged = [transaction.join("next.json"), transaction.join("next.md")];
    let backups = [
        transaction.join("previous.json"),
        transaction.join("previous.md"),
    ];
    let mut backed_up = [false; 2];
    let mut published = [false; 2];
    let result = (|| {
        set_private_dir(&transaction)?;
        // Preserve enough context for explicit recovery after process death.
        write_private(
            &transaction.join("update.json"),
            &serde_json::to_vec_pretty(update)
                .map_err(|_| "proof_serialization_failed".to_string())?,
        )?;
        before(PublicationStep::StageJson)?;
        write_private(&staged[0], json)?;
        before(PublicationStep::StageMarkdown)?;
        write_private(&staged[1], markdown)?;
        for (index, step) in [PublicationStep::BackupJson, PublicationStep::BackupMarkdown]
            .into_iter()
            .enumerate()
        {
            before(step)?;
            match fs::symlink_metadata(&targets[index]) {
                Ok(metadata) if metadata.is_file() => {
                    fs::rename(&targets[index], &backups[index])
                        .map_err(|_| "proof_publication_failed".to_string())?;
                    backed_up[index] = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err("proof_publication_failed".into()),
            }
        }
        for (index, step) in [
            PublicationStep::PublishJson,
            PublicationStep::PublishMarkdown,
        ]
        .into_iter()
        .enumerate()
        {
            before(step)?;
            fs::rename(&staged[index], &targets[index])
                .map_err(|_| "proof_publication_failed".to_string())?;
            published[index] = true;
        }
        before(PublicationStep::AppendStatus)?;
        append_status()
    })();
    if let Err(error) = result {
        let mut rollback_failed = false;
        for index in (0..2).rev() {
            if published[index] && fs::remove_file(&targets[index]).is_err() {
                rollback_failed = true;
                continue;
            }
            if backed_up[index] && fs::rename(&backups[index], &targets[index]).is_err() {
                rollback_failed = true;
            }
        }
        // A failed ledger rollback is also indeterminate; retain transaction
        // evidence and block later reads/builds instead of concealing it.
        if rollback_failed || error == "import_ledger_rollback_failed" {
            return Err("proof_publication_rollback_failed".into());
        }
        fs::remove_dir_all(&transaction)
            .map_err(|_| "proof_publication_recovery_required".to_string())?;
        return Err(error);
    }
    fs::remove_dir_all(&transaction).map_err(|_| "proof_publication_recovery_required".to_string())
}

#[cfg(test)]
#[path = "agent_import_persistence_tests.rs"]
mod tests;
