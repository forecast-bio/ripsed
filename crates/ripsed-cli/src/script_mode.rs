use ripsed_core::config::Config;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::{Op, OpOptions};
use ripsed_core::script::{Script, parse_script};
use ripsed_fs::discovery::{WalkStrategy, discover_files_auto};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::args::Cli;
use crate::file_mode::{FileOutcome, process_one_file};
use crate::human;
use crate::shared::{build_op_options, load_undo_log, record_undo_capped, save_undo_log};

/// One operation's pass over its files, fanned out across workers.
/// Outcomes come back in discovery order.
fn process_script_pass(
    files: &[PathBuf],
    op: &Op,
    matcher: &Matcher,
    options: &OpOptions,
    cli: &Cli,
    already_backed_up: &HashSet<PathBuf>,
    config: &Config,
) -> Vec<FileOutcome> {
    use rayon::prelude::*;

    let work = || {
        files
            .par_iter()
            .map(|path| {
                let proc = crate::file_mode::ProcessOptions {
                    dry_run: cli.dry_run,
                    // Back up only before a file's first modification in
                    // the script, so the .bak reflects pre-script content.
                    skip_backup: already_backed_up.contains(path),
                    context_lines: crate::file_mode::display_context_lines(cli),
                    undo_max_file_bytes: config.undo.max_file_bytes,
                    stream_min_bytes: config.defaults.stream_min_bytes,
                };
                process_one_file(path, op, matcher, options, proc)
            })
            .collect::<Vec<_>>()
    };

    match cli.threads {
        Some(n) => match rayon::ThreadPoolBuilder::new().num_threads(n).build() {
            Ok(pool) => pool.install(work),
            Err(e) => {
                eprintln!("ripsed: warning: cannot build {n}-thread pool ({e}); using default");
                work()
            }
        },
        None => work(),
    }
}

/// Run ripsed in script mode: read a .rip file and execute each operation.
pub fn run_script_mode(script_path: &str, cli: &Cli, config: &Config) -> Result<(), i32> {
    let script_content = match std::fs::read_to_string(script_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ripsed: cannot read script '{script_path}': {e}");
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    let script: Script = match parse_script(&script_content) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    if script.operations.is_empty() {
        eprintln!("ripsed: script '{script_path}' contains no operations");
        return Err(crate::shared::EXIT_ERROR);
    }

    let mut total_changes = 0usize;
    let mut had_errors = false;
    let mut files_modified_set: HashSet<PathBuf> = HashSet::new();

    // Load undo log for recording changes (skipped for dry runs and
    // --no-undo bulk runs)
    let mut undo_log = if !cli.dry_run && !cli.no_undo {
        Some(load_undo_log(config))
    } else {
        None
    };

    // Operations run in script order (each sees the previous one's output
    // on disk); within one operation, files fan out across workers. The
    // per-file advisory lock is scoped to each (operation, file) pass.
    for script_op in &script.operations {
        let op = &script_op.op;

        let matcher = match Matcher::new(op) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("ripsed: {e}");
                return Err(crate::shared::EXIT_ERROR);
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
                had_errors = true;
                continue;
            }
        };

        if files.is_empty() {
            continue;
        }

        let outcomes = process_script_pass(
            &files,
            op,
            &matcher,
            &options,
            cli,
            &files_modified_set,
            config,
        );

        for outcome in outcomes {
            for err in &outcome.errors {
                had_errors = true;
                eprintln!("{err}");
            }
            for note in &outcome.notes {
                eprintln!("{note}");
            }
            if outcome.total_line_changes == 0 {
                continue;
            }
            total_changes += outcome.total_line_changes;
            if !cli.count && !cli.quiet {
                human::print_file_diff(&outcome.path, &outcome.changes, outcome.total_line_changes);
            }
            if let Some(ref mut log) = undo_log
                && let Some((ref entry, encoding)) = outcome.undo
            {
                record_undo_capped(log, &outcome.path, entry, encoding, &config.undo);
            }
            if outcome.modified {
                files_modified_set.insert(outcome.path.clone());
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

    crate::file_mode::exit_result(had_errors, total_changes)
}
