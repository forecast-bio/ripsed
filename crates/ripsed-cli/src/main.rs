mod args;
mod file_mode;
mod human;
mod interactive;
mod json_mode;
mod pipe_mode;
mod script_mode;
mod shared;

use args::Cli;
use clap::Parser;
use ripsed_json::detect::{InputMode, detect_stdin};
use std::io::{BufRead, IsTerminal, Read};
use std::process;

fn main() {
    // The only process::exit in the binary: run() has fully returned by now,
    // so every Drop (file locks, temp files) has already executed.
    let exit_code = match run() {
        Ok(()) => 0,
        Err(code) => code,
    };
    process::exit(exit_code);
}

fn run() -> Result<(), i32> {
    let cli = Cli::parse();

    // Load config from --config or auto-discover
    let config = shared::load_config(&cli)?;

    // Handle --script before other modes
    if let Some(ref script_path) = cli.script {
        return script_mode::run_script_mode(script_path, &cli, &config);
    }

    // Handle --undo-list before anything else
    if cli.undo_list {
        file_mode::handle_undo_list(&config);
        return Ok(());
    }

    // Handle --undo before anything else
    if let Some(count) = cli.undo {
        return file_mode::handle_undo(count, &config);
    }

    // Force pipe mode when --pipe is set
    if cli.pipe {
        return run_pipe(&cli);
    }

    // Check if stdin has data (pipe mode detection)
    let stdin_is_tty = std::io::stdin().is_terminal();

    if cli.json || (!stdin_is_tty && !cli.no_json) {
        // Attempt JSON/agent mode
        if !stdin_is_tty {
            if cli.json {
                let input = read_stdin_string()?;
                return json_mode::run_json_mode(&input, &config, cli.jsonl);
            }

            // Auto-detect: peek at the buffered first chunk without
            // consuming it. Anything starting with '{' might be a JSON
            // request and is buffered fully for the existing detection;
            // everything else streams through pipe mode line by line.
            let stdin = std::io::stdin();
            let mut reader = std::io::BufReader::new(stdin.lock());
            let first_chunk = match reader.fill_buf() {
                Ok(chunk) => chunk,
                Err(e) => {
                    eprintln!("ripsed: failed to read stdin: {e}");
                    return Err(shared::EXIT_ERROR);
                }
            };

            // Empty stdin without explicit --json: fall through to file
            // mode (subprocess/test environments often have stdin as a
            // closed pipe rather than a tty).
            if first_chunk.is_empty() {
                drop(reader);
                return file_mode::run_file_mode(&cli, &config);
            }

            let starts_with_brace = first_chunk
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|&b| b == b'{');

            if starts_with_brace || cli.multiline {
                // Possible JSON request, or a multiline op that needs the
                // whole buffer either way.
                let mut data = Vec::new();
                if let Err(e) = reader.read_to_end(&mut data) {
                    eprintln!("ripsed: failed to read stdin: {e}");
                    return Err(shared::EXIT_ERROR);
                }
                let mut cursor = std::io::Cursor::new(data);
                match detect_stdin(&mut cursor) {
                    Ok(InputMode::Json(json)) => {
                        json_mode::run_json_mode(&json, &config, cli.jsonl)
                    }
                    Ok(InputMode::Pipe(data)) => pipe_mode::run_pipe_mode(&cli, &data),
                    Err(e) => {
                        eprintln!("ripsed: failed to read stdin: {e}");
                        Err(shared::EXIT_ERROR)
                    }
                }
            } else {
                let stdout = std::io::stdout();
                let mut out = std::io::BufWriter::new(stdout.lock());
                pipe_mode::run_pipe_mode_streaming(&cli, &mut reader, &mut out)
            }
        } else if let Some(ref json_arg) = cli.json_input {
            json_mode::run_json_mode(json_arg, &config, cli.jsonl)
        } else {
            eprintln!("ripsed: --json requires input via stdin or argument");
            Err(shared::EXIT_ERROR)
        }
    } else if !stdin_is_tty {
        // Pipe mode: stdin -> stdout (--no-json was set)
        run_pipe(&cli)
    } else {
        // File mode
        file_mode::run_file_mode(&cli, &config)
    }
}

/// Run pipe mode over stdin: streaming line-by-line, except multiline
/// operations which need the whole buffer.
fn run_pipe(cli: &Cli) -> Result<(), i32> {
    if cli.multiline {
        let data = read_stdin_bytes()?;
        return pipe_mode::run_pipe_mode(cli, &data);
    }
    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    pipe_mode::run_pipe_mode_streaming(cli, &mut reader, &mut out)
}

fn read_stdin_bytes() -> Result<Vec<u8>, i32> {
    let mut data = Vec::new();
    std::io::stdin().read_to_end(&mut data).map_err(|e| {
        eprintln!("ripsed: failed to read stdin: {e}");
        crate::shared::EXIT_ERROR
    })?;
    Ok(data)
}

fn read_stdin_string() -> Result<String, i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| {
        eprintln!("ripsed: failed to read stdin: {e}");
        crate::shared::EXIT_ERROR
    })?;
    Ok(input)
}
