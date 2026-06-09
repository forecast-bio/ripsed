use crate::encoding::{self, SourceEncoding};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

/// Write content to a file atomically using a temporary file + rename.
///
/// Writes plain UTF-8 with no BOM. When the content came from
/// [`crate::reader::read_file_with_encoding`], use [`write_atomic_encoded`]
/// instead so the file keeps its original encoding.
///
/// This is a low-level primitive that does **not** acquire a file lock.
/// For concurrent safety, callers should acquire a [`crate::lock::FileLock`]
/// before reading the file and hold it through this write. The `ripsed`
/// facade crate provides [`ripsed::apply_to_file`] which handles this.
pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    write_atomic_encoded(path, content, SourceEncoding::Utf8)
}

/// Write content to a file atomically, encoded as `encoding` (re-attaching
/// the BOM that was present on read). See [`write_atomic`] for locking.
pub fn write_atomic_encoded(
    path: &Path,
    content: &str,
    encoding: SourceEncoding,
) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(&encoding::encode(content, encoding))?;
    tmp.flush()?;

    // Preserve original file permissions if possible
    if let Ok(metadata) = fs::metadata(path) {
        let _ = fs::set_permissions(tmp.path(), metadata.permissions());
    }

    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Create a backup of a file before modifying it.
///
/// If the backup path already exists, numbered suffixes are tried:
/// `.bak`, `.bak.1`, `.bak.2`, etc.
pub fn create_backup(path: &Path) -> std::io::Result<PathBuf> {
    let base_backup = backup_path_for(path);

    let final_path = if !base_backup.exists() {
        base_backup
    } else {
        let mut n = 1u32;
        loop {
            let candidate = PathBuf::from(format!("{}.{n}", base_backup.display()));
            if !candidate.exists() {
                break candidate;
            }
            n = n
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("too many backup files"))?;
        }
    };

    fs::copy(path, &final_path)?;
    Ok(final_path)
}

/// Compute the base backup path for a given file.
fn backup_path_for(path: &Path) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.ripsed.bak")),
        None => {
            // No extension: append ".ripsed.bak" to the file name directly
            let mut s = path.as_os_str().to_os_string();
            s.push(".ripsed.bak");
            PathBuf::from(s)
        }
    }
}

/// Write multiple files transactionally (all-or-nothing).
///
/// All contents are staged to temporary files first. If every stage
/// succeeds, all files are committed (renamed) in sequence. If any
/// stage fails, none of the files are written and the error is returned.
pub fn write_atomic_batch(files: &[(&Path, &str)]) -> std::io::Result<()> {
    let mut batch = AtomicBatch::new();
    for (path, content) in files {
        batch.stage(path, content)?;
    }
    batch.commit()
}

/// Batch atomic writer: prepares all writes, then commits them all at once.
pub struct AtomicBatch {
    pending: Vec<(NamedTempFile, std::path::PathBuf)>,
}

impl AtomicBatch {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Stage a file write. The content is written to a temp file but not yet committed.
    pub fn stage(&mut self, path: &Path, content: &str) -> std::io::Result<()> {
        self.stage_encoded(path, content, SourceEncoding::Utf8)
    }

    /// Stage a file write in the given encoding (see [`write_atomic_encoded`]).
    pub fn stage_encoded(
        &mut self,
        path: &Path,
        content: &str,
        encoding: SourceEncoding,
    ) -> std::io::Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        let mut tmp = NamedTempFile::new_in(parent)?;
        tmp.write_all(&encoding::encode(content, encoding))?;
        tmp.flush()?;

        if let Ok(metadata) = fs::metadata(path) {
            let _ = fs::set_permissions(tmp.path(), metadata.permissions());
        }

        self.pending.push((tmp, path.to_path_buf()));
        Ok(())
    }

    /// Commit all staged writes atomically (all-or-nothing).
    ///
    /// This is a low-level primitive that does **not** acquire file locks.
    /// For concurrent safety, callers should acquire [`crate::lock::FileLock`]s
    /// on all target paths before calling this method.
    ///
    /// Before renaming, the original contents of each destination file are
    /// saved. If any rename fails mid-commit, all already-persisted files
    /// are restored from the saved originals.
    pub fn commit(self) -> std::io::Result<()> {
        // Phase 1: snapshot originals so we can roll back on partial failure.
        let mut originals: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::with_capacity(self.pending.len());
        for (_tmp, dest) in &self.pending {
            let content = match fs::read(dest) {
                Ok(data) => Some(data),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            originals.push((dest.clone(), content));
        }

        // Phase 2: persist all temp files to their destinations.
        let mut committed = 0usize;
        for (tmp, dest) in self.pending {
            match tmp.persist(&dest) {
                Ok(_) => committed += 1,
                Err(e) => {
                    // Phase 3 (rollback): restore already-committed files.
                    for (path, original) in originals.iter().take(committed) {
                        match original {
                            Some(data) => {
                                let _ = fs::write(path, data);
                            }
                            None => {
                                let _ = fs::remove_file(path);
                            }
                        }
                    }
                    return Err(e.error);
                }
            }
        }
        Ok(())
    }
}

impl Default for AtomicBatch {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ---- write_atomic tests ----

    #[test]
    fn write_atomic_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");

        write_atomic(&path, "hello").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn write_atomic_overwrites_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.txt");
        fs::write(&path, "old").unwrap();

        write_atomic(&path, "new").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    // ---- backup naming tests ----

    #[test]
    fn create_backup_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.txt");
        fs::write(&path, "original").unwrap();

        let backup = create_backup(&path).unwrap();
        assert_eq!(backup, dir.path().join("data.txt.ripsed.bak"));
        assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
    }

    #[test]
    fn create_backup_numbered_when_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.txt");
        fs::write(&path, "v1").unwrap();

        // First backup -> .bak
        let b1 = create_backup(&path).unwrap();
        assert_eq!(b1, dir.path().join("data.txt.ripsed.bak"));

        // Overwrite original
        fs::write(&path, "v2").unwrap();
        // Second backup -> .bak.1
        let b2 = create_backup(&path).unwrap();
        assert_eq!(b2, dir.path().join("data.txt.ripsed.bak.1"));

        // Overwrite original
        fs::write(&path, "v3").unwrap();
        // Third backup -> .bak.2
        let b3 = create_backup(&path).unwrap();
        assert_eq!(b3, dir.path().join("data.txt.ripsed.bak.2"));

        // Verify contents
        assert_eq!(fs::read_to_string(&b1).unwrap(), "v1");
        assert_eq!(fs::read_to_string(&b2).unwrap(), "v2");
        assert_eq!(fs::read_to_string(&b3).unwrap(), "v3");
    }

    #[test]
    fn create_backup_no_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Makefile");
        fs::write(&path, "all:").unwrap();

        let backup = create_backup(&path).unwrap();
        assert_eq!(backup, dir.path().join("Makefile.ripsed.bak"));
        assert_eq!(fs::read_to_string(&backup).unwrap(), "all:");
    }

    // ---- AtomicBatch tests ----

    #[test]
    fn atomic_batch_commit() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");

        let mut batch = AtomicBatch::new();
        batch.stage(&a, "aaa").unwrap();
        batch.stage(&b, "bbb").unwrap();
        batch.commit().unwrap();

        assert_eq!(fs::read_to_string(&a).unwrap(), "aaa");
        assert_eq!(fs::read_to_string(&b).unwrap(), "bbb");
    }

    #[test]
    fn atomic_batch_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");

        let mut batch = AtomicBatch::new();
        batch.stage(&a, "should not appear").unwrap();
        drop(batch);

        assert!(!a.exists());
    }

    // ---- write_atomic_batch tests ----

    #[test]
    fn write_atomic_batch_success() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("x.txt");
        let b = dir.path().join("y.txt");

        write_atomic_batch(&[(&a, "xx"), (&b, "yy")]).unwrap();

        assert_eq!(fs::read_to_string(&a).unwrap(), "xx");
        assert_eq!(fs::read_to_string(&b).unwrap(), "yy");
    }

    #[test]
    fn write_atomic_batch_empty() {
        // Should succeed with no files
        write_atomic_batch(&[]).unwrap();
    }

    #[test]
    fn write_atomic_batch_stage_failure_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.txt");
        // Stage to a non-existent directory so the second stage fails
        let bad = Path::new("/nonexistent_dir_12345/bad.txt");

        let result = write_atomic_batch(&[(&good, "data"), (bad, "nope")]);
        assert!(result.is_err());
        // The good file should not have been written either
        assert!(!good.exists());
    }

    // ---- Adversarial rollback tests ----
    //
    // The existing `atomic_batch_rollback` test only checks that dropping
    // a batch without committing doesn't leave files on disk. It does NOT
    // exercise the commit-time rollback path where some files have already
    // been renamed into place and a later rename fails. These tests do.

    /// **Adversarial**: When a commit fails partway through (rename #3 of 3
    /// fails), the already-committed files #1 and #2 must be restored to
    /// their original contents. This exercises the phase-3 rollback code in
    /// `AtomicBatch::commit` that was added to ensure all-or-nothing semantics.
    #[test]
    fn atomic_batch_commit_mid_failure_restores_originals() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        // `c` is a *directory* where a file is expected — so `persist` will
        // fail on it (can't replace a non-empty directory with a file).
        let c_dir = dir.path().join("c.txt");
        fs::create_dir(&c_dir).unwrap();
        fs::write(c_dir.join("sentinel"), "block").unwrap();

        fs::write(&a, "original_a").unwrap();
        fs::write(&b, "original_b").unwrap();

        let mut batch = AtomicBatch::new();
        batch.stage(&a, "new_a").unwrap();
        batch.stage(&b, "new_b").unwrap();
        // This stage is fine; failure happens at commit time when rename
        // onto `c_dir` fails (rename-file-onto-nonempty-dir is an error
        // on Linux and Windows).
        batch.stage(&c_dir, "new_c").unwrap();

        let result = batch.commit();
        assert!(
            result.is_err(),
            "commit should fail when target c.txt is a non-empty directory"
        );

        // Rollback invariant: a and b should be back to their originals.
        assert_eq!(
            fs::read_to_string(&a).unwrap(),
            "original_a",
            "a.txt should have been rolled back to original content"
        );
        assert_eq!(
            fs::read_to_string(&b).unwrap(),
            "original_b",
            "b.txt should have been rolled back to original content"
        );
        // The directory is still there (we never got to touch it successfully).
        assert!(c_dir.is_dir(), "c.txt directory should still exist");
    }

    /// **Adversarial**: Rollback when the file didn't exist before the commit
    /// must remove it (not leave a stale file behind from the successful
    /// part of the commit).
    #[test]
    fn atomic_batch_rollback_removes_files_that_didnt_exist_before() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("new_file.txt"); // doesn't exist yet
        let c_dir = dir.path().join("c.txt");
        fs::create_dir(&c_dir).unwrap();
        fs::write(c_dir.join("block"), "x").unwrap();

        assert!(!a.exists());

        let mut batch = AtomicBatch::new();
        batch.stage(&a, "created_by_batch").unwrap();
        batch.stage(&c_dir, "will_fail").unwrap();

        let result = batch.commit();
        assert!(result.is_err());

        // `a` should have been removed as part of rollback because it didn't
        // exist pre-commit.
        assert!(
            !a.exists(),
            "Files that didn't exist before commit should be removed on rollback"
        );
    }

    // ---- Permission preservation ----

    /// **Adversarial**: `write_atomic` preserves the source file's permissions.
    /// Without this, a file written with restrictive 0o600 mode could silently
    /// become world-readable after a ripsed edit.
    #[cfg(unix)]
    #[test]
    fn write_atomic_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.txt");
        fs::write(&path, "sensitive\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&path, "modified\n").unwrap();

        let after = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            after, 0o600,
            "write_atomic must preserve restrictive permissions on the target"
        );
    }

    /// **Adversarial**: A write to a non-writable directory fails cleanly and
    /// leaves no temp-file droppings behind from the atomic-write helper.
    #[cfg(unix)]
    #[test]
    fn write_atomic_failure_leaves_no_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let ro_dir = dir.path().join("readonly");
        fs::create_dir(&ro_dir).unwrap();
        let path = ro_dir.join("target.txt");
        // Pre-create the file so persist() is what fails, not the initial open.
        fs::write(&path, "original").unwrap();
        // Make the directory read-only so tempfile creation fails.
        fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o500)).unwrap();

        let result = write_atomic(&path, "new content");
        assert!(
            result.is_err(),
            "write_atomic must fail when tempfile creation is blocked"
        );

        // Restore dir perms so the tempdir can be dropped.
        fs::set_permissions(&ro_dir, fs::Permissions::from_mode(0o700)).unwrap();

        // Only the original file should remain — no .tmp droppings.
        let entries: Vec<_> = fs::read_dir(&ro_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "only the original file should remain, got {entries:?}"
        );
    }

    /// **Adversarial**: write_atomic on content containing an embedded NUL is
    /// preserved byte-for-byte. NULs can upset naive string pipelines; lock
    /// in the behavior we want.
    #[test]
    fn write_atomic_preserves_embedded_nul_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("with_nul.bin");

        let content = "before\x00after\n";
        write_atomic(&path, content).unwrap();
        let back = fs::read(&path).unwrap();
        assert_eq!(back.as_slice(), content.as_bytes());
    }
}
