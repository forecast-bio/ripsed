use ripsed_core::config::Config;
use ripsed_core::engine;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::{Op, OpOptions, ReplaceCount};
use ripsed_fs::discovery::{WalkStrategy, discover_files_auto};
use ripsed_fs::encoding::SourceEncoding;
use ripsed_fs::lock::FileLock;
use ripsed_fs::reader;
use ripsed_fs::writer;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::args::Cli;
use crate::human;
use crate::interactive::{self, ConfirmAction};
use crate::shared::{
    build_op_options, discovery_opts_from, load_undo_log, record_undo_capped, save_undo_log,
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
        let encoding = crate::shared::undo_record_encoding(record);
        match writer::write_atomic_encoded(path, &record.entry.original_text, encoding) {
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
        return Err(crate::shared::EXIT_ERROR);
    };

    let op = build_op_from_cli(cli, find);
    let matcher = match Matcher::new(&op) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    let options = build_op_options(cli, config, cli.glob.clone());

    let mut discovery_opts = discovery_opts_from(&options);
    discovery_opts.follow_links = cli.follow;
    let files = match discover_files_auto(&discovery_opts, WalkStrategy::Auto) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    if files.is_empty() {
        // Nothing to search is a clean no-match, not an error.
        eprintln!("ripsed: no files found");
        return Err(crate::shared::EXIT_NO_MATCHES);
    }

    let mut total_changes = 0usize;
    let mut files_modified = 0usize;

    // Load undo log for recording changes (skipped for dry runs and
    // --no-undo bulk runs)
    let mut undo_log = if !cli.dry_run && !cli.no_undo {
        Some(load_undo_log(config))
    } else {
        None
    };

    // --confirm prompts interactively between apply and write, which is
    // inherently sequential; everything else goes through the worker
    // path (one file per rayon worker; also the streaming entry point —
    // a single file still gets streamed when eligible).
    let mut had_errors = false;

    if !cli.confirm {
        let outcomes = process_files_parallel(&files, &op, &matcher, &options, cli, config);

        // Outcomes are in discovery order regardless of which worker
        // finished first, so output stays deterministic.
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
            if outcome.modified {
                files_modified += 1;
            }
            if let Some(ref mut log) = undo_log
                && let Some((ref entry, encoding)) = outcome.undo
            {
                record_undo_capped(log, &outcome.path, entry, encoding, &config.undo);
            }
        }

        if let Some(ref log) = undo_log {
            save_undo_log(log);
        }
        if cli.count {
            println!("{total_changes}");
        } else if !cli.quiet {
            human::print_summary(files_modified, total_changes, cli.dry_run);
        }
        return exit_result(had_errors, total_changes);
    }

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
                    had_errors = true;
                    continue;
                }
            }
        } else {
            None
        };

        let (content, encoding) = match reader::read_file_with_encoding(file_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ripsed: {}: {e}", file_path.display());
                had_errors = true;
                continue;
            }
        };

        let output = match engine::apply(
            &content,
            &op,
            &matcher,
            options.range_spec(),
            display_context_lines(cli),
        ) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("ripsed: {}: {e}", file_path.display());
                had_errors = true;
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
            human::print_file_diff(file_path, &output.changes, output.changes.len());
        }

        if !cli.dry_run {
            if options.backup
                && let Err(e) = writer::create_backup(file_path)
            {
                eprintln!("ripsed: backup failed for {}: {e}", file_path.display());
                had_errors = true;
                continue;
            }
            if let Some(ref text) = output.text {
                // Record undo entry before writing
                if let Some(ref mut log) = undo_log
                    && let Some(ref undo_entry) = output.undo
                {
                    record_undo_capped(log, file_path, undo_entry, encoding, &config.undo);
                }

                match writer::write_atomic_encoded(file_path, text, encoding) {
                    Ok(()) => files_modified += 1,
                    Err(e) => {
                        eprintln!("ripsed: write failed for {}: {e}", file_path.display());
                        had_errors = true;
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

    exit_result(had_errors, total_changes)
}

/// Final exit decision per the taxonomy: errors take precedence (2),
/// then clean no-match (1), then success (0).
pub(crate) fn exit_result(had_errors: bool, total_changes: usize) -> Result<(), i32> {
    if had_errors {
        Err(crate::shared::EXIT_ERROR)
    } else if total_changes == 0 {
        Err(crate::shared::EXIT_NO_MATCHES)
    } else {
        Ok(())
    }
}

/// Context lines for human diff display — zero when nothing will be
/// displayed (`--quiet`, `--count`), so the engine skips per-change
/// context allocation entirely.
pub(crate) fn display_context_lines(cli: &Cli) -> usize {
    if cli.quiet || cli.count { 0 } else { 3 }
}

/// Everything one worker produced for one file, returned to the main
/// thread so printing, undo recording, and counting stay ordered and
/// single-threaded.
pub(crate) struct FileOutcome {
    pub(crate) path: PathBuf,
    /// Changes for display. The streaming path collects only the first
    /// [`MAX_COLLECTED_CHANGES`]; `total_line_changes` is always exact.
    pub(crate) changes: Vec<ripsed_core::diff::Change>,
    /// Exact count of changed lines (equals `changes.len()` on the
    /// buffered path).
    pub(crate) total_line_changes: usize,
    pub(crate) undo: Option<(ripsed_core::undo::UndoEntry, SourceEncoding)>,
    pub(crate) modified: bool,
    pub(crate) errors: Vec<String>,
    /// Informational stderr lines (e.g. the undo-skip notice) — printed
    /// by the main thread, never affect the exit code.
    pub(crate) notes: Vec<String>,
}

/// How many `Change`s the streaming path collects for display.
/// `human::print_file_diff` shows at most 50; collecting a couple of
/// orders of magnitude past that buys nothing on a multi-gigabyte file.
const MAX_COLLECTED_CHANGES: usize = 50;

/// Whether this file can take the streaming path: large enough to be
/// worth it, no undo entry will be recorded (streaming never holds the
/// original text, so it cannot produce one), and a line-scoped op.
/// BOM-carrying files are excluded by the caller (they need transcode).
fn stream_eligible(
    file_size: u64,
    op: &Op,
    options: &OpOptions,
    undo_max_file_bytes: u64,
    stream_min_bytes: u64,
) -> bool {
    if stream_min_bytes == 0 || file_size < stream_min_bytes || op.is_multiline() {
        return false;
    }
    // No undo entry will exist for this file iff recording is off or the
    // file exceeds a nonzero cap.
    !options.record_undo || (undo_max_file_bytes > 0 && file_size > undo_max_file_bytes)
}

/// Per-run scalars threaded into each file worker.
#[derive(Clone, Copy)]
pub(crate) struct ProcessOptions {
    pub(crate) dry_run: bool,
    pub(crate) skip_backup: bool,
    pub(crate) context_lines: usize,
    pub(crate) undo_max_file_bytes: u64,
    pub(crate) stream_min_bytes: u64,
}

/// Process one file end to end: lock, read, apply, backup, write.
/// Never touches stdout/stderr — diagnostics come back in `errors`.
pub(crate) fn process_one_file(
    file_path: &Path,
    op: &Op,
    matcher: &Matcher,
    options: &OpOptions,
    proc: ProcessOptions,
) -> FileOutcome {
    let ProcessOptions {
        dry_run,
        skip_backup,
        context_lines,
        undo_max_file_bytes,
        stream_min_bytes,
    } = proc;
    let mut outcome = FileOutcome {
        path: file_path.to_path_buf(),
        changes: Vec::new(),
        total_line_changes: 0,
        undo: None,
        modified: false,
        errors: Vec::new(),
        notes: Vec::new(),
    };

    // Acquire advisory lock before reading — holds through backup + write.
    // Skipped in dry-run mode (read-only, no lock files needed).
    let _lock = if !dry_run {
        match FileLock::try_lock_with_timeout(file_path, Duration::from_secs(5)) {
            Ok(l) => Some(l),
            Err(e) => {
                outcome
                    .errors
                    .push(format!("ripsed: {}: {e}", file_path.display()));
                return outcome;
            }
        }
    } else {
        None
    };

    // Large files whose original text won't be kept for undo stream
    // straight to the temp file in constant memory — sed-style. Files
    // with a BOM need transcoding and stay on the buffered path.
    let file_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);
    if stream_eligible(
        file_size,
        op,
        options,
        undo_max_file_bytes,
        stream_min_bytes,
    ) && !file_has_bom(file_path).unwrap_or(true)
    {
        stream_one_file(
            file_path,
            op,
            matcher,
            options,
            dry_run,
            skip_backup,
            &mut outcome,
        );
        return outcome;
    }

    let (content, encoding) = match reader::read_file_with_encoding(file_path) {
        Ok(c) => c,
        Err(e) => {
            outcome
                .errors
                .push(format!("ripsed: {}: {e}", file_path.display()));
            return outcome;
        }
    };

    let output = match engine::apply(&content, op, matcher, options.range_spec(), context_lines) {
        Ok(o) => o,
        Err(e) => {
            outcome
                .errors
                .push(format!("ripsed: {}: {e}", file_path.display()));
            return outcome;
        }
    };

    if output.changes.is_empty() {
        return outcome;
    }
    outcome.total_line_changes = output.changes.len();
    outcome.changes = output.changes;

    if !dry_run {
        if options.backup
            && !skip_backup
            && let Err(e) = writer::create_backup(file_path)
        {
            outcome.errors.push(format!(
                "ripsed: backup failed for {}: {e}",
                file_path.display()
            ));
            return outcome;
        }
        if let Some(ref text) = output.text {
            if let Some(undo_entry) = output.undo {
                outcome.undo = Some((undo_entry, encoding));
            }
            match writer::write_atomic_encoded(file_path, text, encoding) {
                Ok(()) => outcome.modified = true,
                Err(e) => {
                    // The write never landed, so there is nothing to undo.
                    outcome.undo = None;
                    outcome.errors.push(format!(
                        "ripsed: write failed for {}: {e}",
                        file_path.display()
                    ));
                }
            }
        }
    }

    outcome
}

/// Whether the file starts with any byte-order mark (UTF-8 or UTF-16) —
/// such files need transcoding and take the buffered path.
fn file_has_bom(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut prefix = [0u8; 3];
    let n = std::fs::File::open(path)?.read(&mut prefix)?;
    Ok(prefix[..n].starts_with(&ripsed_fs::encoding::UTF8_BOM)
        || ripsed_fs::encoding::has_utf16_bom(&prefix[..n]))
}

/// Stream one large file through the line processor straight into the
/// atomic temp file — constant memory, no undo entry (the original text
/// is never held). Each line keeps its own terminator. The temp file is
/// persisted only when something actually changed; the backup (if
/// requested) is taken just before persisting, so it captures the
/// original.
fn stream_one_file(
    file_path: &Path,
    op: &Op,
    matcher: &Matcher,
    options: &OpOptions,
    dry_run: bool,
    skip_backup: bool,
    outcome: &mut FileOutcome,
) {
    use ripsed_core::engine::LineProcessor;
    use std::io::{BufRead, BufReader, BufWriter, Write};

    let mut processor = match LineProcessor::new(op, matcher, options.range_spec()) {
        Ok(p) => p,
        Err(e) => {
            outcome
                .errors
                .push(format!("ripsed: {}: {e}", file_path.display()));
            return;
        }
    };

    let input = match std::fs::File::open(file_path) {
        Ok(f) => f,
        Err(e) => {
            outcome
                .errors
                .push(format!("ripsed: {}: {e}", file_path.display()));
            return;
        }
    };
    let mut reader = BufReader::new(input);

    // Dry runs transform into the void; real runs into an atomic temp
    // file beside the target.
    let parent = file_path.parent().unwrap_or(Path::new("."));
    let mut tmp = None;
    let mut sink: Box<dyn Write> = if dry_run {
        Box::new(std::io::sink())
    } else {
        match tempfile::NamedTempFile::new_in(parent) {
            Ok(t) => {
                let writer = match t.reopen() {
                    Ok(f) => f,
                    Err(e) => {
                        outcome
                            .errors
                            .push(format!("ripsed: {}: {e}", file_path.display()));
                        return;
                    }
                };
                tmp = Some(t);
                Box::new(BufWriter::new(writer))
            }
            Err(e) => {
                outcome
                    .errors
                    .push(format!("ripsed: {}: {e}", file_path.display()));
                return;
            }
        }
    };

    let mut line_num = 0usize;
    let mut buf = String::new();
    loop {
        buf.clear();
        match reader.read_line(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                outcome
                    .errors
                    .push(format!("ripsed: {}: {e}", file_path.display()));
                return; // temp dropped, original untouched
            }
        }
        line_num += 1;

        let (content, terminator) = if let Some(stripped) = buf.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = buf.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (buf.as_str(), "")
        };

        let result = processor.process_line(content);
        if result.changed {
            outcome.total_line_changes += 1;
            if outcome.changes.len() < MAX_COLLECTED_CHANGES {
                outcome.changes.push(ripsed_core::diff::Change {
                    line: line_num,
                    before: content.to_string(),
                    after: if result.lines.is_empty() {
                        None
                    } else {
                        Some(result.lines.join("\n"))
                    },
                    context: None,
                });
            }
        }

        let inner_sep = if terminator.is_empty() {
            "\n"
        } else {
            terminator
        };
        let last = result.lines.len().saturating_sub(1);
        for (i, line) in result.lines.iter().enumerate() {
            let sep = if i == last { terminator } else { inner_sep };
            if let Err(e) = sink
                .write_all(line.as_bytes())
                .and_then(|()| sink.write_all(sep.as_bytes()))
            {
                outcome.errors.push(format!(
                    "ripsed: write failed for {}: {e}",
                    file_path.display()
                ));
                return;
            }
        }
    }

    if let Err(e) = sink.flush() {
        outcome.errors.push(format!(
            "ripsed: write failed for {}: {e}",
            file_path.display()
        ));
        return;
    }
    drop(sink);
    // Close the input handle before persisting: replacing a file that
    // still has an open handle fails on Windows with Access Denied
    // (MoveFileEx over an open destination) — Unix rename doesn't care,
    // which is exactly why only the Windows runner caught this.
    drop(reader);

    if outcome.total_line_changes == 0 {
        return; // nothing matched; temp (if any) is dropped
    }

    outcome.notes.push(format!(
        "ripsed: undo skipped for {}: streamed large file (see undo.max_file_bytes)",
        file_path.display()
    ));

    if dry_run {
        return;
    }
    let Some(tmp) = tmp else { return };

    if options.backup
        && !skip_backup
        && let Err(e) = writer::create_backup(file_path)
    {
        outcome.errors.push(format!(
            "ripsed: backup failed for {}: {e}",
            file_path.display()
        ));
        return;
    }

    if let Ok(metadata) = std::fs::metadata(file_path) {
        let _ = std::fs::set_permissions(tmp.path(), metadata.permissions());
    }
    match tmp.persist(file_path) {
        Ok(_) => outcome.modified = true,
        Err(e) => {
            outcome.errors.push(format!(
                "ripsed: write failed for {}: {e}",
                file_path.display()
            ));
        }
    }
}

/// Fan files out across a rayon pool, one worker per file, returning
/// outcomes in the input (discovery) order.
pub(crate) fn process_files_parallel(
    files: &[PathBuf],
    op: &Op,
    matcher: &Matcher,
    options: &OpOptions,
    cli: &Cli,
    config: &Config,
) -> Vec<FileOutcome> {
    use rayon::prelude::*;

    let proc = ProcessOptions {
        dry_run: cli.dry_run,
        skip_backup: false,
        context_lines: display_context_lines(cli),
        undo_max_file_bytes: config.undo.max_file_bytes,
        stream_min_bytes: config.defaults.stream_min_bytes,
    };
    let work = || {
        files
            .par_iter()
            .map(|path| process_one_file(path, op, matcher, options, proc))
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

pub fn build_op_from_cli(cli: &Cli, find: &str) -> Op {
    let find = find.to_string();
    let regex = cli.regex;
    let case_insensitive = cli.case_insensitive;
    let count = if cli.first {
        ReplaceCount::FirstPerLine
    } else if cli.first_in_file {
        ReplaceCount::FirstInFile
    } else if let Some(n) = cli.max_replacements {
        ReplaceCount::Max(n)
    } else {
        ReplaceCount::All
    };

    if cli.delete {
        Op::Delete {
            multiline: cli.multiline,
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
            count,
            multiline: cli.multiline,
            find,
            replace: cli.replace.clone().unwrap_or_default(),
            regex,
            case_insensitive,
        }
    }
}
