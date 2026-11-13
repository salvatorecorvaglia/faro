//! Moving data between Faro and files.

pub mod backup;
pub mod export;
pub mod import;
mod writer;

pub use backup::{BackupOptions, BackupResult, RestoreOptions, RestoreResult};
pub use export::{ExportFormat, ExportOptions};
pub use import::{ColumnMapping, ImportFormat, ImportOptions, ImportPreview};

use std::path::{Path, PathBuf};

/// A file written under a temporary name and moved into place only when
/// complete.
///
/// Backups and exports used to write straight to the chosen path, so one that
/// failed or was cancelled partway left a file with a valid header and part of
/// the data — indistinguishable from a finished one until someone restored it.
/// Now the target either appears whole or not at all, and an existing file at
/// the target is only replaced by a complete one.
///
/// The temporary sits beside the target so the final rename stays on one
/// filesystem, where it is atomic. Dropping the guard without [`commit`]
/// removes it.
///
/// [`commit`]: PartialFile::commit
pub struct PartialFile {
    partial: PathBuf,
    target: PathBuf,
    _directory: tempfile::TempDir,
}

impl PartialFile {
    pub fn new(target: &Path) -> std::io::Result<Self> {
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Exclusive, private directory on the destination filesystem. Writers
        // create the file exclusively as well, refusing symlinks and collisions.
        let directory = tempfile::Builder::new()
            .prefix(".faro-")
            .tempdir_in(parent)?;
        Ok(Self {
            partial: directory.path().join("output.partial"),
            target: target.to_path_buf(),
            _directory: directory,
        })
    }

    /// Where to write while the file is incomplete.
    pub fn path(&self) -> &Path {
        &self.partial
    }

    /// Move the finished file into place.
    pub fn commit(self) -> std::io::Result<()> {
        std::fs::rename(&self.partial, &self.target)?;
        Ok(())
    }
}

/// Retain the returned handle for the duration of writing; never reopen or
/// follow an existing path when producing an export.
pub(crate) fn create_output(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("faro_partial_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn exclusive_output_refuses_symlinks_without_touching_the_target() {
        let directory = tempfile::tempdir().unwrap();
        let victim = directory.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let output = PartialFile::new(&directory.path().join("export.csv")).unwrap();
        std::os::unix::fs::symlink(&victim, output.path()).unwrap();
        assert!(create_output(output.path()).is_err());
        drop(output);
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "keep");
    }

    #[test]
    fn a_completed_output_replaces_an_existing_destination() {
        use std::io::Write;
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("export.csv");
        std::fs::write(&target, "old").unwrap();
        let output = PartialFile::new(&target).unwrap();
        let mut writer = create_output(output.path()).unwrap();
        writer.write_all(b"new").unwrap();
        writer.sync_all().unwrap();
        drop(writer);
        output.commit().unwrap();
        assert_eq!(std::fs::read_to_string(target).unwrap(), "new");
    }

    #[test]
    fn concurrent_outputs_own_separate_partial_files() {
        let target = dir().join("concurrent.sql");
        let first = PartialFile::new(&target).unwrap();
        let second = PartialFile::new(&target).unwrap();
        assert_ne!(first.path(), second.path());
        std::fs::write(first.path(), "first").unwrap();
        std::fs::write(second.path(), "second").unwrap();
        drop(first);
        assert_eq!(std::fs::read_to_string(second.path()).unwrap(), "second");
        second.commit().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second");
        std::fs::remove_file(target).unwrap();
    }

    #[test]
    fn a_committed_file_appears_at_the_target_and_nothing_is_left_behind() {
        let target = dir().join("done.sql");
        let _ = std::fs::remove_file(&target);
        let file = PartialFile::new(&target).unwrap();
        std::fs::write(file.path(), "complete").unwrap();
        let partial = file.path().to_path_buf();
        file.commit().unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "complete");
        assert!(!partial.exists());
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn an_abandoned_file_never_reaches_the_target_and_is_cleaned_up() {
        let target = dir().join("abandoned.sql");
        let _ = std::fs::remove_file(&target);
        let partial = {
            let file = PartialFile::new(&target).unwrap();
            std::fs::write(file.path(), "half a backup").unwrap();
            file.path().to_path_buf()
        };
        assert!(!target.exists(), "an incomplete file was published");
        assert!(!partial.exists(), "the temporary was left behind");
    }

    #[test]
    fn an_existing_file_survives_a_failed_replacement() {
        let target = dir().join("existing.sql");
        std::fs::write(&target, "previous good backup").unwrap();
        {
            let file = PartialFile::new(&target).unwrap();
            std::fs::write(file.path(), "half").unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "previous good backup"
        );
        let _ = std::fs::remove_file(&target);
    }
}
