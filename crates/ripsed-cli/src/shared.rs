use ripsed_core::config::Config;
use ripsed_core::operation::OpOptions;
use ripsed_core::undo::{UndoEntry, UndoLog, UndoRecord};
use ripsed_fs::discovery::DiscoveryOptions;
use ripsed_fs::encoding::SourceEncoding;
use ripsed_fs::lock::FileLock;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::args::Cli;

/// Exit codes, following the ripgrep convention:
/// `0` = ran and made (or previewed) changes,
/// [`EXIT_NO_MATCHES`] = ran cleanly but nothing matched,
/// [`EXIT_ERROR`] = something went wrong (bad regex, IO failure,
/// invalid request, lock timeout). Errors take precedence over
/// matches: a run with per-file errors exits 2 even if other files
/// were changed.
pub const EXIT_NO_MATCHES: i32 = 1;
pub const EXIT_ERROR: i32 = 2;

/// Convert operation options into file discovery options.
pub fn discovery_opts_from(opts: &OpOptions) -> DiscoveryOptions {
    DiscoveryOptions {
        root: opts
            .root
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
        glob: opts.glob.clone(),
        ignore_pattern: opts.ignore.clone(),
        gitignore: opts.gitignore,
        hidden: opts.hidden,
        max_depth: opts.max_depth,
        follow_links: false,
    }
}

/// Load configuration from --config path or auto-discover from cwd.
pub fn load_config(cli: &Cli) -> Result<Config, i32> {
    if let Some(ref path_str) = cli.config {
        Config::load(Path::new(path_str)).map_err(|e| {
            eprintln!("ripsed: {e}");
            EXIT_ERROR
        })
    } else {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match Config::discover(&cwd) {
            Ok(Some((_path, config))) => Ok(config),
            Ok(None) => Ok(Config::default()),
            Err(e) => {
                eprintln!("ripsed: {e}");
                Err(EXIT_ERROR)
            }
        }
    }
}

/// Resolve the undo directory: `.ripsed/` next to the config file, or in cwd.
pub fn undo_dir() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(".ripsed")
}

pub fn undo_log_path() -> PathBuf {
    undo_dir().join("undo.jsonl")
}

/// Load the undo log from disk.
pub fn load_undo_log(config: &Config) -> UndoLog {
    let path = undo_log_path();
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(content) => UndoLog::from_jsonl(&content, config.undo.max_entries),
            Err(_) => UndoLog::new(config.undo.max_entries),
        }
    } else {
        UndoLog::new(config.undo.max_entries)
    }
}

/// Record an undo entry in the log for a given file path.
///
/// `encoding` is the file's detected source encoding; plain UTF-8 is
/// stored as `None` to keep the log format unchanged for the common case.
pub fn record_undo(
    log: &mut UndoLog,
    file_path: &Path,
    entry: &UndoEntry,
    encoding: SourceEncoding,
) {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}", d.as_secs()))
        .unwrap_or_else(|_| "0".to_string());

    log.push(UndoRecord {
        encoding: (encoding != SourceEncoding::Utf8).then(|| encoding.tag().to_string()),
        timestamp,
        file_path: file_path.to_string_lossy().to_string(),
        entry: entry.clone(),
    });
}

/// Record an undo entry, honoring the per-file size cap.
///
/// Returns `false` (with a one-line stderr note) when the decoded text
/// exceeds `undo.max_file_bytes` — the undo log stores a full copy of the
/// original, which dominates the cost of editing very large files. The
/// edit itself still proceeds. A cap of `0` means unlimited.
pub fn record_undo_capped(
    log: &mut UndoLog,
    file_path: &Path,
    entry: &UndoEntry,
    encoding: SourceEncoding,
    undo_config: &ripsed_core::config::UndoConfig,
) -> bool {
    let size = entry.original_text.len() as u64;
    if undo_config.max_file_bytes > 0 && size > undo_config.max_file_bytes {
        eprintln!(
            "ripsed: undo skipped for {}: {size} bytes exceeds undo.max_file_bytes ({}); use --backup or raise the limit in .ripsed.toml",
            file_path.display(),
            undo_config.max_file_bytes
        );
        return false;
    }
    record_undo(log, file_path, entry, encoding);
    true
}

/// The source encoding to restore an undo record with (tag parse, with
/// plain UTF-8 for absent or unknown tags).
pub fn undo_record_encoding(record: &UndoRecord) -> SourceEncoding {
    record
        .encoding
        .as_deref()
        .and_then(SourceEncoding::from_tag)
        .unwrap_or_default()
}

/// Build OpOptions from CLI args and config, consolidating the shared logic
/// used by file_mode and script_mode.
pub fn build_op_options(cli: &Cli, config: &Config, glob: Option<String>) -> OpOptions {
    OpOptions {
        dry_run: cli.dry_run,
        root: None,
        gitignore: if cli.no_gitignore {
            false
        } else {
            config.defaults.gitignore
        },
        backup: cli.backup || config.defaults.backup,
        atomic: false,
        glob,
        ignore: cli.ignore_pattern.clone(),
        hidden: cli.hidden,
        max_depth: cli.max_depth.or(config.defaults.max_depth),
        line_range: cli.line_range,
        range: cli.range.clone(),
        record_undo: !cli.no_undo,
    }
}

/// Save the undo log to disk. Warns on stderr if the write fails.
///
/// Acquires an advisory file lock on the undo log to prevent concurrent
/// ripsed processes from clobbering each other's entries.
pub fn save_undo_log(log: &UndoLog) {
    let dir = undo_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "ripsed: warning: cannot create undo directory {}: {e}",
            dir.display()
        );
        return;
    }
    let path = undo_log_path();
    let _lock = match FileLock::try_lock_with_timeout(&path, Duration::from_secs(5)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ripsed: warning: cannot lock undo log: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(&path, log.to_jsonl()) {
        eprintln!(
            "ripsed: warning: cannot save undo log to {}: {e}",
            path.display()
        );
    } else {
        // Restrict permissions — undo log may contain sensitive file contents
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
}
