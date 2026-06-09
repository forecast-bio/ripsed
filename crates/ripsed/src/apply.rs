use std::io;
use std::path::Path;
use std::time::Duration;

use ripsed_core::diff::Change;
use ripsed_core::engine::{self, EngineOutput};
use ripsed_core::error::RipsedError;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::{LineRange, Op};
use ripsed_core::undo::UndoEntry;
use ripsed_fs::discovery::{DiscoveryOptions, WalkStrategy, discover_files_auto};
use ripsed_fs::lock::FileLock;
use ripsed_fs::reader;
use ripsed_fs::writer;

/// Default timeout for acquiring per-file advisory locks.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Options controlling how [`apply_to_file`] and [`apply_to_files`] behave.
#[derive(Debug, Clone)]
pub struct ApplyOptions {
    /// Preview changes without writing to disk.
    pub dry_run: bool,
    /// Create `.ripsed.bak` backup before writing.
    pub backup: bool,
    /// Restrict operations to a range of lines (1-indexed, inclusive).
    pub line_range: Option<LineRange>,
    /// Number of context lines around each change (for diffs).
    pub context_lines: usize,
    /// Timeout for acquiring the per-file advisory lock.
    pub lock_timeout: Duration,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            backup: false,
            line_range: None,
            context_lines: 3,
            lock_timeout: LOCK_TIMEOUT,
        }
    }
}

/// Apply an operation to a single file with full read-modify-write locking.
///
/// Acquires an advisory [`FileLock`] on the target path **before** reading,
/// holds it through the backup and write, then releases on return. This
/// prevents concurrent `ripsed` processes from clobbering each other's
/// changes on the same file.
///
/// Returns the engine output (changes, modified text, undo entry).
/// If the file has no matches, `output.changes` will be empty and
/// `output.text` will be `None`.
pub fn apply_to_file(
    path: &Path,
    op: &Op,
    options: &ApplyOptions,
) -> Result<EngineOutput, ApplyError> {
    // Only lock when we might write — dry_run is read-only.
    let _lock = if !options.dry_run {
        Some(
            FileLock::try_lock_with_timeout(path, options.lock_timeout)
                .map_err(|e| ApplyError::Lock(path.to_path_buf(), e))?,
        )
    } else {
        None
    };

    let content = reader::read_file(path).map_err(|e| ApplyError::Read(path.to_path_buf(), e))?;

    let matcher = Matcher::new(op).map_err(ApplyError::Engine)?;

    let output = engine::apply(
        &content,
        op,
        &matcher,
        options.line_range,
        options.context_lines,
    )
    .map_err(ApplyError::Engine)?;

    if !options.dry_run && !output.changes.is_empty() {
        if options.backup {
            writer::create_backup(path).map_err(|e| ApplyError::Backup(path.to_path_buf(), e))?;
        }
        if let Some(ref text) = output.text {
            writer::write_atomic(path, text)
                .map_err(|e| ApplyError::Write(path.to_path_buf(), e))?;
        }
    }

    Ok(output)
}

/// Apply an operation to all files discovered from the given options.
///
/// Each file is independently locked, read, modified, and written.
/// Files that produce no matches are silently skipped.
///
/// Returns one [`FileResult`] per file that had at least one change.
pub fn apply_to_files(
    op: &Op,
    discovery: &DiscoveryOptions,
    options: &ApplyOptions,
) -> Result<Vec<FileResult>, ApplyError> {
    let files =
        discover_files_auto(discovery, WalkStrategy::Auto).map_err(ApplyError::Discovery)?;

    let mut results = Vec::new();

    for path in &files {
        let output = apply_to_file(path, op, options)?;

        if !output.changes.is_empty() {
            results.push(FileResult {
                path: path.to_path_buf(),
                changes: output.changes,
                undo: output.undo,
            });
        }
    }

    Ok(results)
}

/// Result of applying an operation to a single file (returned by [`apply_to_files`]).
#[derive(Debug)]
pub struct FileResult {
    /// The path of the file that was modified.
    pub path: std::path::PathBuf,
    /// Structured diff of changes made.
    pub changes: Vec<Change>,
    /// Undo entry to reverse this operation (None if dry-run).
    pub undo: Option<UndoEntry>,
}

/// Errors that can occur during [`apply_to_file`] or [`apply_to_files`].
#[derive(Debug)]
pub enum ApplyError {
    /// Failed to acquire the advisory file lock.
    Lock(std::path::PathBuf, io::Error),
    /// Failed to read the file.
    Read(std::path::PathBuf, io::Error),
    /// Failed to create a backup.
    Backup(std::path::PathBuf, io::Error),
    /// Failed to write the file.
    Write(std::path::PathBuf, io::Error),
    /// Engine or matcher error (bad regex, etc.).
    Engine(RipsedError),
    /// File discovery failed.
    Discovery(io::Error),
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lock(p, e) => write!(f, "failed to lock {}: {e}", p.display()),
            Self::Read(p, e) => write!(f, "failed to read {}: {e}", p.display()),
            Self::Backup(p, e) => write!(f, "backup failed for {}: {e}", p.display()),
            Self::Write(p, e) => write!(f, "write failed for {}: {e}", p.display()),
            Self::Engine(e) => write!(f, "{e}"),
            Self::Discovery(e) => write!(f, "file discovery failed: {e}"),
        }
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lock(_, e) | Self::Read(_, e) | Self::Backup(_, e) | Self::Write(_, e) => Some(e),
            Self::Engine(e) => Some(e),
            Self::Discovery(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_replace_op(find: &str, replace: &str) -> Op {
        Op::Replace {
            multiline: false,
            find: find.to_string(),
            replace: replace.to_string(),
            regex: false,
            case_insensitive: false,
        }
    }

    #[test]
    fn apply_to_file_dry_run_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello world\n").unwrap();

        let op = make_replace_op("hello", "goodbye");
        let opts = ApplyOptions::default(); // dry_run = true

        let output = apply_to_file(&path, &op, &opts).unwrap();
        assert!(!output.changes.is_empty());
        // File should be unchanged
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello world\n");
    }

    #[test]
    fn apply_to_file_writes_when_not_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello world\n").unwrap();

        let op = make_replace_op("hello", "goodbye");
        let opts = ApplyOptions {
            dry_run: false,
            ..Default::default()
        };

        let output = apply_to_file(&path, &op, &opts).unwrap();
        assert!(!output.changes.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), "goodbye world\n");
    }

    #[test]
    fn apply_to_file_creates_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello\n").unwrap();

        let op = make_replace_op("hello", "goodbye");
        let opts = ApplyOptions {
            dry_run: false,
            backup: true,
            ..Default::default()
        };

        apply_to_file(&path, &op, &opts).unwrap();

        let backup = dir.path().join("test.txt.ripsed.bak");
        assert!(backup.exists());
        assert_eq!(fs::read_to_string(&backup).unwrap(), "hello\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "goodbye\n");
    }

    #[test]
    fn apply_to_file_no_match_skips_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello world\n").unwrap();

        let op = make_replace_op("nonexistent", "replaced");
        let opts = ApplyOptions {
            dry_run: false,
            ..Default::default()
        };

        let output = apply_to_file(&path, &op, &opts).unwrap();
        assert!(output.changes.is_empty());
        assert!(output.text.is_none());
    }

    #[test]
    fn apply_to_file_returns_undo_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "hello world\n").unwrap();

        let op = make_replace_op("hello", "goodbye");
        let opts = ApplyOptions {
            dry_run: false,
            ..Default::default()
        };

        let output = apply_to_file(&path, &op, &opts).unwrap();
        let undo = output.undo.unwrap();
        assert_eq!(undo.original_text, "hello world\n");
    }

    #[test]
    fn apply_to_file_nonexistent_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.txt");

        let op = make_replace_op("a", "b");
        let opts = ApplyOptions::default();

        let err = apply_to_file(&path, &op, &opts).unwrap_err();
        assert!(matches!(err, ApplyError::Read(_, _)));
    }

    #[test]
    fn apply_to_files_processes_multiple() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
        fs::write(dir.path().join("b.txt"), "hello\n").unwrap();
        fs::write(dir.path().join("c.txt"), "nothing\n").unwrap();

        let op = make_replace_op("hello", "bye");
        let discovery = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            ..Default::default()
        };
        let opts = ApplyOptions {
            dry_run: false,
            ..Default::default()
        };

        let results = apply_to_files(&op, &discovery, &opts).unwrap();
        // a.txt and b.txt should match, c.txt should be skipped
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn apply_error_display_includes_path() {
        let err = ApplyError::Read(
            std::path::PathBuf::from("/tmp/test.txt"),
            io::Error::new(io::ErrorKind::NotFound, "not found"),
        );
        let msg = err.to_string();
        assert!(msg.contains("test.txt"));
        assert!(msg.contains("not found"));
    }
}
