use ripsed_core::engine;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::RangeSpec;

use crate::args::Cli;
use crate::file_mode::build_op_from_cli;

pub fn run_pipe_mode(cli: &Cli, data: &[u8]) -> Result<(), i32> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ripsed: stdin is not valid UTF-8: {e}");
            return Err(1);
        }
    };

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

    let range = if let Some(ref patterns) = cli.range {
        Some(RangeSpec::Patterns(patterns.clone()))
    } else {
        cli.line_range.map(RangeSpec::Lines)
    };
    match engine::apply(text, &op, &matcher, range, 0) {
        Ok(output) => {
            if cli.count {
                println!("{}", output.changes.len());
            } else {
                print!("{}", output.text.as_deref().unwrap_or(text));
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("ripsed: {e}");
            Err(1)
        }
    }
}
