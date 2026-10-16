//! Moving data between Faro and files.

pub mod backup;
pub mod export;
pub mod import;

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
    committed: bool,
}

impl PartialFile {
    pub fn new(target: &Path) -> Self {
        let name = target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "faro-output".into());
        Self {
            partial: target.with_file_name(format!(".{name}.partial")),
            target: target.to_path_buf(),
            committed: false,
        }
    }

    /// Where to write while the file is incomplete.
    pub fn path(&self) -> &Path {
        &self.partial
    }

    /// Move the finished file into place.
    pub fn commit(mut self) -> std::io::Result<()> {
        std::fs::rename(&self.partial, &self.target)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.partial);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("faro_partial_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_committed_file_appears_at_the_target_and_nothing_is_left_behind() {
        let target = dir().join("done.sql");
        let _ = std::fs::remove_file(&target);
        let file = PartialFile::new(&target);
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
            let file = PartialFile::new(&target);
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
            let file = PartialFile::new(&target);
            std::fs::write(file.path(), "half").unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "previous good backup"
        );
        let _ = std::fs::remove_file(&target);
    }
}
