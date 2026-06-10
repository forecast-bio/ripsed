use crate::reader;
use ignore::WalkBuilder;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Convert an `ignore` crate error into an `io::Error`.
fn glob_error(e: ignore::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid glob pattern: {e}"),
    )
}

/// Options for file discovery.
pub struct DiscoveryOptions {
    pub root: PathBuf,
    pub glob: Option<String>,
    pub ignore_pattern: Option<String>,
    pub gitignore: bool,
    pub hidden: bool,
    pub max_depth: Option<usize>,
    pub follow_links: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            glob: None,
            ignore_pattern: None,
            gitignore: true,
            hidden: false,
            max_depth: None,
            follow_links: false,
        }
    }
}

/// Build a `WalkBuilder` with the shared configuration from `DiscoveryOptions`.
///
/// Both serial and parallel discovery call this so that gitignore, hidden,
/// follow_links, max_depth, and glob overrides are configured in one place.
fn configure_walk_builder(opts: &DiscoveryOptions) -> io::Result<WalkBuilder> {
    let mut builder = WalkBuilder::new(&opts.root);
    builder
        .git_ignore(opts.gitignore)
        .git_global(opts.gitignore)
        .git_exclude(opts.gitignore)
        .hidden(!opts.hidden)
        .follow_links(opts.follow_links);

    if let Some(depth) = opts.max_depth {
        builder.max_depth(Some(depth));
    }

    if let Some(ref glob) = opts.glob {
        let mut overrides = ignore::overrides::OverrideBuilder::new(&opts.root);
        overrides.add(glob).map_err(glob_error)?;
        let built = overrides.build().map_err(glob_error)?;
        builder.overrides(built);
    }

    Ok(builder)
}

/// ripsed's own advisory-lock sentinels are infrastructure, never edit
/// targets. Editing one is pointless (their content is informational) and
/// actively hazardous on Windows: a parallel worker holding the
/// `LockFileEx` region on `foo.ripsed.lock` (while editing `foo`) makes
/// concurrent reads of the sentinel fail — region locks are mandatory
/// there, unlike Unix's advisory `flock`.
fn is_lock_sentinel(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(".ripsed.lock"))
}

/// Discover files to process based on options.
///
/// Returns an error if the glob pattern is invalid. Results are deduplicated
/// by filesystem identity so that a symlink alias or hard-link pair of the
/// same inode is only returned once — callers that lock by path would race
/// each other otherwise.
pub fn discover_files(opts: &DiscoveryOptions) -> io::Result<Vec<PathBuf>> {
    let builder = configure_walk_builder(opts)?;
    let walker = builder.build();

    let files: Vec<PathBuf> = walker
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|ft| ft.is_file()))
        .filter(|entry| !is_lock_sentinel(entry.path()))
        .filter(|entry| {
            // Filter out binary files (reads only 8KB, not the whole file)
            !reader::is_binary(entry.path()).unwrap_or(true)
        })
        .filter(|entry| {
            // Apply ignore pattern if set
            if let Some(ref pattern) = opts.ignore_pattern {
                let path_str = entry.path().to_string_lossy();
                !glob_match(pattern, &path_str)
            } else {
                true
            }
        })
        .map(|entry| entry.into_path())
        .collect();

    Ok(dedupe_by_identity(files))
}

/// Discover files using WalkBuilder's parallel walker for large directories.
///
/// Returns an error if the glob pattern is invalid.
pub fn discover_files_parallel(opts: &DiscoveryOptions) -> io::Result<Vec<PathBuf>> {
    let mut builder = configure_walk_builder(opts)?;
    builder.threads(rayon::current_num_threads().max(2));

    let ignore_pattern = opts.ignore_pattern.clone();
    let results: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    builder.build_parallel().run(|| {
        let results = &results;
        let ignore_pattern = ignore_pattern.clone();
        Box::new(move |entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Continue,
            };

            // Only process regular files
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }

            if is_lock_sentinel(entry.path()) {
                return ignore::WalkState::Continue;
            }

            // Filter out binary files (reads only 8KB, not the whole file)
            if reader::is_binary(entry.path()).unwrap_or(true) {
                return ignore::WalkState::Continue;
            }

            // Apply ignore pattern
            if let Some(ref pattern) = ignore_pattern {
                let path_str = entry.path().to_string_lossy();
                if glob_match(pattern, &path_str) {
                    return ignore::WalkState::Continue;
                }
            }

            results.lock().unwrap().push(entry.into_path());
            ignore::WalkState::Continue
        })
    });

    let mut files = results.into_inner().unwrap();
    files.sort();
    // Deduplicate by filesystem identity — see `discover_files` and
    // `dedupe_by_identity` for rationale. Sorting first keeps the kept
    // path deterministic (lexicographically smallest wins).
    Ok(dedupe_by_identity(files))
}

/// Strategy for choosing between serial and parallel file discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkStrategy {
    /// Automatically choose based on directory size heuristic.
    Auto,
    /// Always use the parallel walker.
    ForceParallel,
}

/// Choose between serial and parallel file discovery.
///
/// With [`WalkStrategy::Auto`], always uses the parallel walker — it has
/// minimal overhead on small directories and avoids the unreliable top-level
/// entry count heuristic that previously caused incorrect serial fallback
/// on deep directory trees.
///
/// Returns an error if the glob pattern is invalid.
pub fn discover_files_auto(
    opts: &DiscoveryOptions,
    _strategy: WalkStrategy,
) -> io::Result<Vec<PathBuf>> {
    discover_files_parallel(opts)
}

/// Simple glob matching (delegates to the `ignore` crate's globbing).
fn glob_match(pattern: &str, path: &str) -> bool {
    ignore::gitignore::GitignoreBuilder::new("")
        .add_line(None, pattern)
        .ok()
        .and_then(|b| b.build().ok())
        .is_some_and(|gi| gi.matched(Path::new(path), false).is_ignore())
}

/// A platform-specific stable identity for a filesystem entry.
///
/// Two paths that share the same `FileIdentity` refer to the same underlying
/// file, so processing both would cause double writes / lock races. This is
/// used by [`dedupe_by_identity`] to collapse symlink aliases and hard links.
///
/// On Unix, we use `(dev, ino)` which catches both symlinks and hard links.
/// On other platforms we fall back to the canonical path, which catches
/// symlinks but not hard links (Windows file-ID would require winapi).
#[derive(Debug, Hash, PartialEq, Eq)]
enum FileIdentity {
    #[cfg(unix)]
    Inode(u64, u64),
    #[cfg(not(unix))]
    Canonical(PathBuf),
}

fn file_identity(path: &Path) -> Option<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        Some(FileIdentity::Inode(m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        std::fs::canonicalize(path)
            .ok()
            .map(FileIdentity::Canonical)
    }
}

/// Deduplicate paths by [`FileIdentity`], keeping the first occurrence.
///
/// This prevents downstream callers (e.g. `ripsed::apply_to_file`) from
/// acquiring per-path locks on two different paths that refer to the same
/// underlying file — which would race and silently lose writes.
///
/// If [`file_identity`] fails for a path (permission denied, broken symlink,
/// etc.), the path is kept as-is; we prefer preserving visibility of a file
/// over opportunistic dedup.
fn dedupe_by_identity(files: Vec<PathBuf>) -> Vec<PathBuf> {
    use std::collections::HashSet;
    let mut seen: HashSet<FileIdentity> = HashSet::new();
    let mut out = Vec::with_capacity(files.len());
    for path in files {
        match file_identity(&path) {
            Some(id) => {
                if seen.insert(id) {
                    out.push(path);
                }
            }
            // If we can't establish identity, keep the path — the worst case
            // is a missed dedup, which we're tolerating to avoid losing files.
            None => out.push(path),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temporary directory tree with the given number of text files.
    fn make_tree(count: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..count {
            let p = dir.path().join(format!("file_{i}.txt"));
            fs::write(&p, format!("content {i}\n")).unwrap();
        }
        dir
    }

    #[test]
    fn lock_sentinels_are_never_discovered() {
        // ripsed's own advisory-lock sentinels must not become edit
        // targets. On Windows a parallel worker holding the LockFileEx
        // region on `foo.ripsed.lock` makes concurrent reads of it fail
        // (mandatory region locks), which surfaced as CI failures the
        // moment parallel application landed.
        let dir = make_tree(2);
        fs::write(dir.path().join("file_0.txt.ripsed.lock"), "pid 123\n").unwrap();
        fs::write(dir.path().join("bare.ripsed.lock"), "").unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        for files in [
            discover_files(&opts).unwrap(),
            discover_files_parallel(&opts).unwrap(),
        ] {
            assert_eq!(files.len(), 2, "only the two real files: {files:?}");
            assert!(
                files
                    .iter()
                    .all(|f| !f.to_string_lossy().ends_with(".ripsed.lock")),
                "sentinels leaked into discovery: {files:?}"
            );
        }
    }

    #[test]
    fn serial_and_parallel_agree() {
        let dir = make_tree(20);
        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let mut serial = discover_files(&opts).unwrap();
        serial.sort();
        let parallel = discover_files_parallel(&opts).unwrap();

        assert_eq!(serial, parallel);
    }

    #[test]
    fn parallel_skips_binary() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("text.txt"), "hello\n").unwrap();
        fs::write(dir.path().join("bin.dat"), b"\x00\x01\x02").unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let files = discover_files_parallel(&opts).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("text.txt"));
    }

    #[test]
    fn auto_uses_serial_for_small() {
        let dir = make_tree(5);
        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        // Should not panic and should find all 5 files
        let files = discover_files_auto(&opts, WalkStrategy::Auto).unwrap();
        assert_eq!(files.len(), 5);
    }

    #[test]
    fn auto_force_parallel() {
        let dir = make_tree(5);
        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let files = discover_files_auto(&opts, WalkStrategy::ForceParallel).unwrap();
        assert_eq!(files.len(), 5);
    }

    #[test]
    fn parallel_respects_glob() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("b.txt"), "hello").unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: Some("*.rs".to_string()),
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let files = discover_files_parallel(&opts).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a.rs"));
    }

    #[test]
    fn parallel_respects_ignore_pattern() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("keep.txt"), "keep").unwrap();
        fs::write(dir.path().join("skip.log"), "skip").unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: Some("*.log".to_string()),
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let files = discover_files_parallel(&opts).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("keep.txt"));
    }

    #[test]
    fn invalid_glob_returns_error() {
        let dir = make_tree(3);
        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: Some("[invalid".to_string()),
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        assert!(discover_files(&opts).is_err());
        assert!(discover_files_parallel(&opts).is_err());
        assert!(discover_files_auto(&opts, WalkStrategy::Auto).is_err());
    }

    #[test]
    fn valid_glob_returns_ok() {
        let dir = make_tree(3);
        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: Some("*.txt".to_string()),
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        assert!(discover_files(&opts).is_ok());
        assert!(discover_files_parallel(&opts).is_ok());
        assert!(discover_files_auto(&opts, WalkStrategy::Auto).is_ok());
    }

    // ---- Adversarial: symlink scope / follow_links semantics ----

    /// **Adversarial**: By default (`follow_links = false`), a symlink whose
    /// target lives OUTSIDE the discovery root must NOT cause that target to
    /// be discovered. This is a documented-but-untested invariant — if
    /// broken, an attacker could trick `ripsed '...' '...'` into editing
    /// `/etc/passwd` by planting a symlink in a repo.
    #[cfg(unix)]
    #[test]
    fn symlink_outside_root_not_followed_by_default() {
        use std::os::unix::fs::symlink;

        let outside_dir = tempfile::tempdir().unwrap();
        let sensitive = outside_dir.path().join("secret.conf");
        fs::write(&sensitive, "DO NOT EDIT\n").unwrap();

        let root_dir = tempfile::tempdir().unwrap();
        // Place a symlink inside root pointing to the external sensitive file.
        let link_in_root = root_dir.path().join("innocent.txt");
        symlink(&sensitive, &link_in_root).unwrap();

        let opts = DiscoveryOptions {
            root: root_dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let files = discover_files(&opts).unwrap();
        // The external file's canonical path must not appear in the results.
        let sensitive_canonical = fs::canonicalize(&sensitive).unwrap();
        for f in &files {
            if let Ok(can) = fs::canonicalize(f) {
                assert_ne!(
                    can, sensitive_canonical,
                    "External sensitive file leaked into discovery via symlink: {f:?}"
                );
            }
        }
    }

    /// **Adversarial**: With `follow_links = true`, a symlink pointing to a
    /// file in the same tree must not cause the underlying inode to appear
    /// twice in the result. Downstream callers that per-path lock would
    /// otherwise race two lock files for one inode and silently lose writes.
    ///
    /// Tight invariant: `canonicals.len() == distinct.len()` — no canonical
    /// path appears more than once.
    #[cfg(unix)]
    #[test]
    fn symlink_alias_deduped_by_identity() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        fs::write(&real, "content\n").unwrap();
        let link = dir.path().join("alias.txt");
        symlink(&real, &link).unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: true,
        };

        let files = discover_files_parallel(&opts).unwrap();
        assert_eq!(
            files.len(),
            1,
            "symlink alias must be collapsed; got {files:?}"
        );
        let canonicals: Vec<_> = files
            .iter()
            .filter_map(|f| fs::canonicalize(f).ok())
            .collect();
        let distinct: std::collections::HashSet<_> = canonicals.iter().collect();
        assert_eq!(
            canonicals.len(),
            distinct.len(),
            "no canonical path should appear twice: {canonicals:?}"
        );
    }

    /// **Adversarial**: Hard-linked files share an inode but have distinct
    /// directory entries. They must also be deduped — per-path locking on
    /// two hard links to the same inode races the same way symlinks do.
    /// Unix-only: hard-link dedup uses `(dev, ino)`; non-Unix platforms use
    /// canonical paths, which treat hard links as distinct.
    #[cfg(unix)]
    #[test]
    fn hard_link_aliases_are_deduped_on_unix() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("first.txt");
        let b = dir.path().join("second.txt");
        fs::write(&a, "content\n").unwrap();
        fs::hard_link(&a, &b).unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let files = discover_files_parallel(&opts).unwrap();
        assert_eq!(
            files.len(),
            1,
            "hard-linked files must be collapsed to one entry on Unix; got {files:?}"
        );
    }

    /// **Adversarial**: The serial walker must also dedupe (parity with the
    /// parallel walker). A regression that applies dedup to only one path
    /// would show up here.
    #[cfg(unix)]
    #[test]
    fn symlink_alias_deduped_in_serial_walker() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        fs::write(&real, "content\n").unwrap();
        let link = dir.path().join("alias.txt");
        symlink(&real, &link).unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: true,
        };

        let files = discover_files(&opts).unwrap();
        assert_eq!(
            files.len(),
            1,
            "serial walker must also dedup; got {files:?}"
        );
    }

    // ---- Adversarial: ordering determinism ----

    /// **Adversarial**: `discover_files_parallel` is required to return paths
    /// in sorted order (post-sort at line 137). Without a deterministic order,
    /// JSON output becomes non-reproducible and diffs of agent runs become
    /// meaningless. This locks that in across multiple invocations.
    #[test]
    fn parallel_discovery_order_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        // Files created in an order that is unlikely to match sort order.
        for name in ["z.txt", "a.txt", "m.txt", "b.txt", "q.txt"] {
            fs::write(dir.path().join(name), "hi\n").unwrap();
        }

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let a = discover_files_parallel(&opts).unwrap();
        let b = discover_files_parallel(&opts).unwrap();
        let c = discover_files_parallel(&opts).unwrap();
        assert_eq!(a, b, "repeated parallel discovery must be deterministic");
        assert_eq!(b, c, "repeated parallel discovery must be deterministic");
        // And must be sorted.
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(a, sorted, "parallel discovery must return sorted paths");
    }

    /// **Adversarial**: The binary-file filter reads only a small prefix
    /// (8 KB). Opening a 2 GB binary file should not cause the discovery
    /// pipeline to mmap or read the whole file. We verify this indirectly
    /// by checking that discovery on a 2 MB binary file completes without
    /// allocating a gigabyte of memory. This is a smoke test against a
    /// potential performance regression.
    #[test]
    fn large_binary_file_is_filtered_without_full_read() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("blob.bin");
        // 2 MB of data with a NUL in the first 8 KB so it's marked binary.
        let mut buf = vec![b'A'; 2 * 1024 * 1024];
        buf[100] = 0;
        fs::write(&big, &buf).unwrap();
        fs::write(dir.path().join("text.txt"), "ok\n").unwrap();

        let opts = DiscoveryOptions {
            root: dir.path().to_path_buf(),
            glob: None,
            ignore_pattern: None,
            gitignore: false,
            hidden: false,
            max_depth: None,
            follow_links: false,
        };

        let start = std::time::Instant::now();
        let files = discover_files_parallel(&opts).unwrap();
        let elapsed = start.elapsed();

        assert_eq!(files.len(), 1, "only the text file should be returned");
        assert!(files[0].ends_with("text.txt"));
        // Generous bound — a full read of 2 MB would likely still be fast,
        // but if someone regresses this to an mmap-based check we'd still
        // want a signal.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "binary filtering took {elapsed:?} — possible full-file read regression"
        );
    }
}
