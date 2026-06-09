use std::io::{BufRead, Write};

use ripsed_core::engine::{self, LineProcessor};
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::{Op, RangeSpec};

use crate::args::Cli;
use crate::file_mode::build_op_from_cli;

/// Build the operation, matcher, and range filter shared by both pipe paths.
fn build_pipe_op(cli: &Cli) -> Result<(Op, Matcher, Option<RangeSpec>), i32> {
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

    let range = if let Some(ref patterns) = cli.range {
        Some(RangeSpec::Patterns(patterns.clone()))
    } else {
        cli.line_range.map(RangeSpec::Lines)
    };
    Ok((op, matcher, range))
}

/// Buffered pipe mode: the whole input is in memory.
///
/// Used for multiline operations (which need the full buffer) and for
/// input the auto-detect path already had to read completely.
pub fn run_pipe_mode(cli: &Cli, data: &[u8]) -> Result<(), i32> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ripsed: stdin is not valid UTF-8: {e}");
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    let (op, matcher, range) = build_pipe_op(cli)?;
    match engine::apply(text, &op, &matcher, range, 0) {
        Ok(output) => {
            if cli.count {
                println!("{}", output.changes.len());
            } else {
                print!("{}", output.text.as_deref().unwrap_or(text));
            }
            crate::file_mode::exit_result(false, output.changes.len())
        }
        Err(e) => {
            eprintln!("ripsed: {e}");
            Err(crate::shared::EXIT_ERROR)
        }
    }
}

/// Streaming pipe mode: read, transform, and write line by line without
/// buffering the input, so arbitrarily large (or infinite) streams work
/// in constant memory — like sed in a pipeline.
///
/// Each line keeps its own terminator (`\r\n` or `\n`), so mixed-ending
/// streams pass through byte-exact — no majority-vote normalization.
/// Inserted lines take the current line's terminator (`\n` at EOF when
/// the final line has none). A closed downstream (`| head`) terminates
/// quietly with success, matching pipeline conventions.
///
/// Multiline operations cannot stream; callers buffer those instead
/// (`LineProcessor::new` rejects them).
pub fn run_pipe_mode_streaming(
    cli: &Cli,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<(), i32> {
    let (op, matcher, range) = build_pipe_op(cli)?;
    let mut processor = match LineProcessor::new(&op, &matcher, range) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("ripsed: {e}");
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    let mut total_changes = 0usize;
    let mut buf = String::new();
    loop {
        buf.clear();
        let bytes_read = match input.read_line(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("ripsed: failed to read stdin: {e}");
                return Err(crate::shared::EXIT_ERROR);
            }
        };
        if bytes_read == 0 {
            break; // EOF
        }

        // Split off this line's own terminator so it can be re-attached
        // exactly (per-line CRLF passthrough).
        let (content, terminator) = if let Some(stripped) = buf.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = buf.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (buf.as_str(), "") // final line without a newline
        };

        let result = processor.process_line(content);
        if result.changed {
            total_changes += 1;
        }
        if cli.count {
            continue;
        }

        // Separator between multiple emitted lines (inserts): the line's
        // own terminator, or LF when the source line had none (EOF).
        let inner_sep = if terminator.is_empty() {
            "\n"
        } else {
            terminator
        };
        let last = result.lines.len().saturating_sub(1);
        for (i, line) in result.lines.iter().enumerate() {
            let sep = if i == last { terminator } else { inner_sep };
            if let Err(e) = output
                .write_all(line.as_bytes())
                .and_then(|()| output.write_all(sep.as_bytes()))
            {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    // Downstream closed (e.g. `| head`) — normal termination.
                    return Ok(());
                }
                eprintln!("ripsed: failed to write output: {e}");
                return Err(crate::shared::EXIT_ERROR);
            }
        }
    }

    if cli.count {
        let _ = writeln!(output, "{total_changes}");
    }
    if let Err(e) = output.flush()
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        eprintln!("ripsed: failed to write output: {e}");
        return Err(crate::shared::EXIT_ERROR);
    }
    crate::file_mode::exit_result(false, total_changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::Cursor;

    fn cli(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("ripsed").chain(args.iter().copied()))
    }

    fn stream(cli_args: &[&str], input: &str) -> (String, Result<(), i32>) {
        let c = cli(cli_args);
        let mut reader = Cursor::new(input.as_bytes().to_vec());
        let mut out = Vec::new();
        let result = run_pipe_mode_streaming(&c, &mut reader, &mut out);
        (String::from_utf8(out).unwrap(), result)
    }

    #[test]
    fn streaming_replace_matches_buffered_output() {
        let (out, res) = stream(&["hello", "goodbye"], "hello world\nplain\nhello\n");
        assert_eq!(res, Ok(()));
        assert_eq!(out, "goodbye world\nplain\ngoodbye\n");
    }

    #[test]
    fn streaming_preserves_mixed_terminators_exactly() {
        // Buffered mode majority-votes line endings; streaming must pass
        // each line's own terminator through untouched.
        let (out, _) = stream(&["x", "y"], "x\r\nx\nx\r\nplain\n");
        assert_eq!(out, "y\r\ny\ny\r\nplain\n");
    }

    #[test]
    fn streaming_final_line_without_newline_stays_bare() {
        let (out, _) = stream(&["x", "y"], "x\nx");
        assert_eq!(out, "y\ny");
    }

    #[test]
    fn streaming_insert_after_uses_line_terminator() {
        let (out, _) = stream(&["--after", "inserted", "mark"], "mark\r\nmark");
        assert_eq!(out, "mark\r\ninserted\r\nmark\ninserted");
    }

    #[test]
    fn streaming_delete_drops_lines() {
        let (out, _) = stream(&["-d", "gone"], "keep\ngone\nkeep\n");
        assert_eq!(out, "keep\nkeep\n");
    }

    #[test]
    fn streaming_count_prints_total_and_no_content() {
        let (out, _) = stream(&["-c", "x", "y"], "x\nplain\nx\nx\n");
        assert_eq!(out, "3\n");
    }

    #[test]
    fn streaming_line_range_applies() {
        let (out, _) = stream(&["-n", "2:3", "x", "y"], "x\nx\nx\nx\n");
        assert_eq!(out, "x\ny\ny\nx\n");
    }

    #[test]
    fn streaming_pattern_range_applies() {
        let (out, _) = stream(
            &["--range", "/BEGIN/,/END/", "x", "y"],
            "x\nBEGIN\nx\nEND\nx\n",
        );
        assert_eq!(out, "x\nBEGIN\ny\nEND\nx\n");
    }

    #[test]
    fn streaming_first_in_file_budget_spans_lines() {
        let (out, _) = stream(&["--first-in-file", "x", "y"], "x\nx\nx\n");
        assert_eq!(out, "y\nx\nx\n");
    }

    #[test]
    fn streaming_broken_pipe_is_quiet_success() {
        struct BrokenPipe;
        impl Write for BrokenPipe {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "epipe"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let c = cli(&["x", "y"]);
        let mut reader = Cursor::new(b"x\nx\n".to_vec());
        let mut out = BrokenPipe;
        assert_eq!(run_pipe_mode_streaming(&c, &mut reader, &mut out), Ok(()));
    }
}
