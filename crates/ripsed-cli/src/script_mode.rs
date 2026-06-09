use ripsed_core::config::Config;
use ripsed_core::engine;
use ripsed_core::matcher::Matcher;
use ripsed_core::script::{Script, parse_script};
use ripsed_fs::discovery::{WalkStrategy, discover_files_auto};
use ripsed_fs::lock::FileLock;
use ripsed_fs::reader;
use ripsed_fs::writer;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use crate::args::Cli;
use crate::human;
use crate::shared::{build_op_options, load_undo_log, record_undo, save_undo_log};

/// Run ripsed in script mode: read a .rip file and execute each operation.
pub fn run_script_mode(script_path: &str, cli: &Cli, config: &Config) -> Result<(), i32> {
    let script_content = match std::fs::read_to_string(script_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ripsed: cannot read script '{script_path}': {e}");
            return Err(1);
        }
    };

    let script: Script = match parse_script(&script_content) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(1);
        }
    };

    if script.operations.is_empty() {
        eprintln!("ripsed: script '{script_path}' contains no operations");
        return Err(1);
    }

    let mut total_changes = 0usize;
    let mut files_modified_set: HashSet<PathBuf> = HashSet::new();

    // Advisory locks held from first read of each file through final write.
    let mut file_locks: HashMap<PathBuf, FileLock> = HashMap::new();

    // Load undo log for recording changes (only when not dry-run)
    let mut undo_log = if !cli.dry_run {
        Some(load_undo_log(config))
    } else {
        None
    };

    for script_op in &script.operations {
        let op = &script_op.op;

        let matcher = match Matcher::new(op) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("ripsed: {e}");
                return Err(1);
            }
        };

        // Build options using per-op glob or fall back to CLI glob
        let effective_glob = script_op.glob.clone().or_else(|| cli.glob.clone());
        let options = build_op_options(cli, config, effective_glob);

        let mut discovery_opts = crate::shared::discovery_opts_from(&options);
        discovery_opts.follow_links = cli.follow;
        let files = match discover_files_auto(&discovery_opts, WalkStrategy::Auto) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("ripsed: {e}");
                continue;
            }
        };

        if files.is_empty() {
            continue;
        }

        for file_path in &files {
            // Acquire advisory lock on first access to this file.
            // Skipped in dry-run mode (read-only, no lock files needed).
            if !cli.dry_run && !file_locks.contains_key(file_path) {
                match FileLock::try_lock_with_timeout(file_path, Duration::from_secs(5)) {
                    Ok(lock) => {
                        file_locks.insert(file_path.clone(), lock);
                    }
                    Err(e) => {
                        eprintln!("ripsed: {}: {e}", file_path.display());
                        continue;
                    }
                }
            }

            let content = match reader::read_file(file_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("ripsed: {}: {e}", file_path.display());
                    continue;
                }
            };

            let output = match engine::apply(&content, op, &matcher, options.range_spec(), 3) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("ripsed: {}: {e}", file_path.display());
                    continue;
                }
            };

            if output.changes.is_empty() {
                continue;
            }

            total_changes += output.changes.len();

            if cli.count {
                // Just count, don't print diffs
            } else if !cli.quiet {
                human::print_file_diff(file_path, &output.changes);
            }

            if !cli.dry_run {
                if options.backup
                    && !files_modified_set.contains(file_path)
                    && let Err(e) = writer::create_backup(file_path)
                {
                    eprintln!("ripsed: backup failed for {}: {e}", file_path.display());
                    continue;
                }
                if let Some(ref text) = output.text {
                    // Record undo entry before writing
                    if let Some(ref mut log) = undo_log
                        && let Some(ref undo_entry) = output.undo
                    {
                        record_undo(log, file_path, undo_entry);
                    }

                    match writer::write_atomic(file_path, text) {
                        Ok(()) => {
                            files_modified_set.insert(file_path.clone());
                        }
                        Err(e) => {
                            eprintln!("ripsed: write failed for {}: {e}", file_path.display());
                        }
                    }
                }
            }
        }
    }

    // Save undo log after all changes
    if let Some(ref log) = undo_log {
        save_undo_log(log);
    }

    if cli.count {
        println!("{total_changes}");
    } else if !cli.quiet {
        human::print_summary(files_modified_set.len(), total_changes, cli.dry_run);
    }

    if total_changes == 0 { Err(1) } else { Ok(()) }
}
