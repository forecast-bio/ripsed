//! Shared helpers for the CLI integration tests.
//!
//! Each integration test binary compiles this module independently via
//! `mod common;`, so not every test file uses every helper.
#![allow(dead_code)]

use std::fs;
use tempfile::TempDir;

/// Create a temp dir populated with the given (relative path, content) files.
pub fn setup_files(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (name, content) in files {
        let file_path = dir.path().join(name);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&file_path, content).unwrap();
    }
    dir
}

/// Create a temp dir containing a single file.
pub fn setup_single_file(filename: &str, content: &str) -> TempDir {
    setup_files(&[(filename, content)])
}

/// Render a temp dir path for embedding in a JSON request string
/// (escapes backslashes for Windows paths).
pub fn json_path(dir: &TempDir) -> String {
    dir.path().display().to_string().replace('\\', "\\\\")
}

/// Build a v1 JSON request from raw operation and option fragments.
pub fn json_request(operations: &str, options: &str) -> String {
    format!(r#"{{"version": "1", "operations": [{operations}], "options": {{{options}}}}}"#)
}
