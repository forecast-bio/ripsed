use ripsed_core::diff::Change;
use std::path::Path;

const BOLD: anstyle::Style = anstyle::Style::new().bold();
const RED: anstyle::Style =
    anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red)));
const GREEN: anstyle::Style =
    anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green)));
const RESET: anstyle::Reset = anstyle::Reset;

/// Most diff hunks printed per file. Past this nobody is reading the
/// scrollback, and rendering a million hunks costs more than the edit
/// itself (measured: ~2.8 s of a 4.6 s 64 MiB run was printing).
const MAX_PRINTED_CHANGES: usize = 50;

/// Print a colored diff for a file's changes (capped per file).
///
/// `total_changes` is the file's true changed-line count — it can exceed
/// `changes.len()` when the producer collected only a display sample
/// (the streaming path).
pub fn print_file_diff(path: &Path, changes: &[Change], total_changes: usize) {
    anstream::println!("{BOLD}{}{RESET}", path.display());

    for change in changes.iter().take(MAX_PRINTED_CHANGES) {
        // Print context before
        if let Some(ref ctx) = change.context {
            for line in &ctx.before {
                anstream::println!("  {line}");
            }
        }

        // Print the change
        anstream::println!("{RED}- {}{RESET}", change.before);
        if let Some(ref after) = change.after {
            // For insert operations, after may contain newlines
            for line in after.lines() {
                anstream::println!("{GREEN}+ {line}{RESET}");
            }
        }

        // Print context after
        if let Some(ref ctx) = change.context {
            for line in &ctx.after {
                anstream::println!("  {line}");
            }
        }

        anstream::println!();
    }

    let printed = changes.len().min(MAX_PRINTED_CHANGES);
    if total_changes > printed {
        anstream::println!(
            "  … and {} more change(s) in this file\n",
            total_changes - printed
        );
    }
}

/// Print a summary line.
pub fn print_summary(files_matched: usize, total_changes: usize, dry_run: bool) {
    if dry_run {
        anstream::eprintln!(
            "ripsed: dry run — {total_changes} change(s) in {files_matched} file(s) (not applied)"
        );
    } else {
        anstream::eprintln!("ripsed: {total_changes} change(s) applied in {files_matched} file(s)");
    }
}
