use ripsed_core::config::Config;
use ripsed_core::engine;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::Op;
use ripsed_fs::discovery::{WalkStrategy, discover_files_auto};
use ripsed_fs::lock::FileLock;
use ripsed_fs::reader;
use ripsed_fs::writer;
use std::time::Duration;

use crate::args::Cli;
use crate::human;
use crate::interactive::{self, ConfirmAction};
use crate::shared::{
    build_op_options, discovery_opts_from, load_undo_log, record_undo, save_undo_log,
};

/// Handle `--undo N`: restore the last N files from the undo log.
pub fn handle_undo(count: usize, config: &Config) -> Result<(), i32> {
    let mut log = load_undo_log(config);
    if log.is_empty() {
        eprintln!("ripsed: nothing to undo");
        return Err(1);
    }

    let records = log.pop(count);
    if records.is_empty() {
        eprintln!("ripsed: nothing to undo");
        return Err(1);
    }

    for record in &records {
        let path = std::path::Path::new(&record.file_path);
        match writer::write_atomic(path, &record.entry.original_text) {
            Ok(()) => {
                eprintln!("ripsed: restored {}", record.file_path);
            }
            Err(e) => {
                eprintln!("ripsed: failed to restore {}: {e}", record.file_path);
            }
        }
    }

    save_undo_log(&log);
    Ok(())
}

/// Handle `--undo-list`: display recent undo log entries.
pub fn handle_undo_list(config: &Config) {
    let log = load_undo_log(config);
    if log.is_empty() {
        eprintln!("ripsed: undo log is empty");
        return;
    }

    let recent = log.recent(20);
    for (i, record) in recent.iter().enumerate() {
        println!("  {} {} ({})", i + 1, record.file_path, record.timestamp);
    }
}

pub fn run_file_mode(cli: &Cli, config: &Config) -> Result<(), i32> {
    let Some(ref find) = cli.find else {
        eprintln!("ripsed: missing FIND pattern");
        return Err(1);
    };

    let op = build_op_from_cli(cli, find);
    let matcher = match Matcher::new(&op) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(1);
        }
    };

    let options = build_op_options(cli, config, cli.glob.clone());

    let mut discovery_opts = discovery_opts_from(&options);
    discovery_opts.follow_links = cli.follow;
    let files = match discover_files_auto(&discovery_opts, WalkStrategy::Auto) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(1);
        }
    };

    if files.is_empty() {
        eprintln!("ripsed: no files found");
        return Err(1);
    }

    let mut total_changes = 0usize;
    let mut files_modified = 0usize;

    // Load undo log for recording changes (only when not dry-run)
    let mut undo_log = if !cli.dry_run {
        Some(load_undo_log(config))
    } else {
        None
    };

    let mut apply_all = false;

    for file_path in &files {
        // Acquire advisory lock before reading — holds through backup + write
        // to prevent concurrent ripsed processes from clobbering each other.
        // Skipped in dry-run mode (read-only, no lock files needed).
        let _lock = if !cli.dry_run {
            match FileLock::try_lock_with_timeout(file_path, Duration::from_secs(5)) {
                Ok(l) => Some(l),
                Err(e) => {
                    eprintln!("ripsed: {}: {e}", file_path.display());
                    continue;
                }
            }
        } else {
            None
        };

        let content = match reader::read_file(file_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ripsed: {}: {e}", file_path.display());
                continue;
            }
        };

        let output = match engine::apply(&content, &op, &matcher, options.line_range, 3) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("ripsed: {}: {e}", file_path.display());
                continue;
            }
        };

        if output.changes.is_empty() {
            continue;
        }

        // Handle --confirm: show changes and prompt per file
        if cli.confirm && !apply_all {
            let action = interactive::confirm_file(file_path, &output.changes);
            match action {
                ConfirmAction::Yes => {}
                ConfirmAction::No | ConfirmAction::SkipFile => continue,
                ConfirmAction::ApplyAll => {
                    apply_all = true;
                }
                ConfirmAction::Quit => {
                    // Save undo log before quitting
                    if let Some(ref log) = undo_log {
                        save_undo_log(log);
                    }
                    return Ok(());
                }
            }
        }

        total_changes += output.changes.len();

        if cli.count {
            // Just count, don't print diffs
        } else if !cli.quiet {
            human::print_file_diff(file_path, &output.changes);
        }

        if !cli.dry_run {
            if options.backup
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
                    Ok(()) => files_modified += 1,
                    Err(e) => {
                        eprintln!("ripsed: write failed for {}: {e}", file_path.display());
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
        human::print_summary(files_modified, total_changes, cli.dry_run);
    }

    if total_changes == 0 { Err(1) } else { Ok(()) }
}

pub fn build_op_from_cli(cli: &Cli, find: &str) -> Op {
    let find = find.to_string();
    let regex = cli.regex;
    let case_insensitive = cli.case_insensitive;

    if cli.delete {
        Op::Delete {
            multiline: false,
            find,
            regex,
            case_insensitive,
        }
    } else if let Some(ref content) = cli.after {
        Op::InsertAfter {
            find,
            content: content.clone(),
            regex,
            case_insensitive,
        }
    } else if let Some(ref content) = cli.before {
        Op::InsertBefore {
            find,
            content: content.clone(),
            regex,
            case_insensitive,
        }
    } else if let Some(ref content) = cli.replace_line {
        Op::ReplaceLine {
            find,
            content: content.clone(),
            regex,
            case_insensitive,
        }
    } else if let Some(mode) = cli.transform {
        Op::Transform {
            find,
            mode,
            regex,
            case_insensitive,
        }
    } else if let Some(ref parts) = cli.surround {
        Op::Surround {
            find,
            prefix: parts[0].clone(),
            suffix: parts[1].clone(),
            regex,
            case_insensitive,
        }
    } else if let Some(amount) = cli.indent {
        Op::Indent {
            find,
            amount,
            use_tabs: false,
            regex,
            case_insensitive,
        }
    } else if let Some(amount) = cli.dedent {
        Op::Dedent {
            find,
            amount,
            use_tabs: false,
            regex,
            case_insensitive,
        }
    } else {
        Op::Replace {
            multiline: false,
            find,
            replace: cli.replace.clone().unwrap_or_default(),
            regex,
            case_insensitive,
        }
    }
}
