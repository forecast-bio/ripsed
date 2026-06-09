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
use std::io::{IsTerminal, Read};
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
        let data = read_stdin_bytes()?;
        return pipe_mode::run_pipe_mode(&cli, &data);
    }

    // Check if stdin has data (pipe mode detection)
    let stdin_is_tty = std::io::stdin().is_terminal();

    if cli.json || (!stdin_is_tty && !cli.no_json) {
        // Attempt JSON/agent mode
        if !stdin_is_tty {
            let input = read_stdin_string()?;

            // If stdin was empty and --json wasn't explicitly requested,
            // fall through to file mode (subprocess/test environments often
            // have stdin as a closed pipe rather than a tty).
            if input.is_empty() && !cli.json {
                return file_mode::run_file_mode(&cli, &config);
            }

            if cli.json {
                return json_mode::run_json_mode(&input, &config, cli.jsonl);
            }

            // Auto-detect
            let mut cursor = std::io::Cursor::new(input.as_bytes());
            match detect_stdin(&mut cursor) {
                Ok(InputMode::Json(json)) => json_mode::run_json_mode(&json, &config, cli.jsonl),
                Ok(InputMode::Pipe(data)) => pipe_mode::run_pipe_mode(&cli, &data),
                Err(e) => {
                    eprintln!("ripsed: failed to read stdin: {e}");
                    Err(1)
                }
            }
        } else if let Some(ref json_arg) = cli.json_input {
            json_mode::run_json_mode(json_arg, &config, cli.jsonl)
        } else {
            eprintln!("ripsed: --json requires input via stdin or argument");
            Err(1)
        }
    } else if !stdin_is_tty {
        // Pipe mode: stdin -> stdout (--no-json was set)
        let data = read_stdin_bytes()?;
        pipe_mode::run_pipe_mode(&cli, &data)
    } else {
        // File mode
        file_mode::run_file_mode(&cli, &config)
    }
}

fn read_stdin_bytes() -> Result<Vec<u8>, i32> {
    let mut data = Vec::new();
    std::io::stdin().read_to_end(&mut data).map_err(|e| {
        eprintln!("ripsed: failed to read stdin: {e}");
        1
    })?;
    Ok(data)
}

fn read_stdin_string() -> Result<String, i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).map_err(|e| {
        eprintln!("ripsed: failed to read stdin: {e}");
        1
    })?;
    Ok(input)
}
