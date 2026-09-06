//! Creation and admission of directories holding local agent artifacts.
use std::{fs, path::Path};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum DirectoryAdmissionError {
    Unavailable,
    Permissions,
}

pub(super) fn ensure_private_directory(path: &Path) -> Result<(), DirectoryAdmissionError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // Creation mode applies only to directories we create. Existing parent
        // and leaf permissions are never changed, including on a creation race.
        builder.mode(0o700);
    }
    builder
        .create(path)
        .map_err(|_| DirectoryAdmissionError::Unavailable)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| DirectoryAdmissionError::Unavailable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DirectoryAdmissionError::Unavailable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: geteuid takes no arguments and reads process identity only.
        let owner = unsafe { libc::geteuid() };
        if metadata.uid() != owner || metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(DirectoryAdmissionError::Permissions);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Reject junctions and other reparse-point leaves as well as symlinks.
        // Ordinary directories retain their inherited Windows ACLs.
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(DirectoryAdmissionError::Unavailable);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn existing_shared_directory_is_refused_without_changing_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o770)).unwrap();
        let before = fs::metadata(directory.path()).unwrap().permissions().mode();
        let result = ensure_private_directory(directory.path());
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode(),
            before
        );
        assert_eq!(result, Err(DirectoryAdmissionError::Permissions));
    }
    #[cfg(unix)]
    #[test]
    fn new_private_leaf_preserves_shared_parent_and_can_be_reused() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o770)).unwrap();
        let parent_mode = fs::metadata(directory.path()).unwrap().permissions().mode();
        let leaf = directory.path().join("agent").join("imports");
        ensure_private_directory(&leaf).unwrap();
        ensure_private_directory(&leaf).unwrap();
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode(),
            parent_mode
        );
        for path in [&leaf, &directory.path().join("agent")] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_leaf_is_refused_without_changing_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("shared");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o770)).unwrap();
        let link = directory.path().join("agent");
        symlink(&target, &link).unwrap();
        assert_eq!(
            ensure_private_directory(&link),
            Err(DirectoryAdmissionError::Unavailable)
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o770
        );
    }

    #[test]
    fn existing_file_is_refused_and_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent");
        fs::write(&path, b"existing contents").unwrap();
        assert_eq!(
            ensure_private_directory(&path),
            Err(DirectoryAdmissionError::Unavailable)
        );
        assert_eq!(fs::read(path).unwrap(), b"existing contents");
    }

    #[cfg(windows)]
    #[test]
    fn ordinary_windows_directory_is_created_and_reused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent");
        ensure_private_directory(&path).unwrap();
        ensure_private_directory(&path).unwrap();
        assert!(path.is_dir());
    }
}
