//! Adversarial error-path tests: malformed input, unwritable targets,
//! and security-relevant invariants that the happy-path suites don't touch.

mod common;

use common::*;
use predicates::prelude::*;
use std::fs;

#[test]
fn pipe_mode_invalid_utf8_stdin_fails_cleanly() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "hello", "goodbye"])
        .write_stdin(&b"hello \xff\xfe world\n"[..])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("UTF-8"));
}

#[test]
fn autodetect_invalid_utf8_stdin_fails_cleanly() {
    // Without --pipe, piped stdin goes through the auto-detect path,
    // which reads stdin as a String. Invalid UTF-8 must produce a
    // diagnostic and exit 1, never a panic.
    let dir = setup_files(&[("test.txt", "content\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .write_stdin(&b"\xff\xfe\xfd"[..])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("stdin"));
}

#[test]
fn explicit_json_with_empty_stdin_returns_invalid_request() {
    // --json promises a JSON response on stdout even when the request
    // is unusable; an empty request must not fall through to file mode.
    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin("")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "empty request should exit non-zero"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout)
        .expect("--json must emit a JSON response even for an empty request");
    assert_eq!(resp["success"], false);
    assert_eq!(resp["errors"][0]["code"], "invalid_request");
}

#[cfg(unix)]
#[test]
fn undo_log_is_written_with_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = setup_files(&[("test.txt", "hello world\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    // The undo log stores pre-edit file contents, which may be sensitive.
    let log_path = dir.path().join(".ripsed/undo.jsonl");
    assert!(log_path.exists(), "undo log should exist after a write");
    let mode = fs::metadata(&log_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "undo log must be 0600, got {mode:o}");
}

#[cfg(unix)]
#[test]
fn unwritable_directory_fails_gracefully_without_modifying_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = setup_files(&[("sub/test.txt", "hello world\n")]);
    let sub = dir.path().join("sub");
    let mut perms = fs::metadata(&sub).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&sub, perms).unwrap();

    // Privileged processes (root, CAP_DAC_OVERRIDE) ignore directory
    // permissions, so the failure can't be provoked — skip.
    if fs::write(sub.join("probe"), "x").is_ok() {
        let _ = fs::remove_file(sub.join("probe"));
        return;
    }

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("test.txt"));

    let content = fs::read_to_string(sub.join("test.txt")).unwrap();
    assert_eq!(
        content, "hello world\n",
        "file must be untouched when its directory is unwritable"
    );

    // Restore permissions so TempDir cleanup can remove the tree.
    let mut perms = fs::metadata(&sub).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&sub, perms).unwrap();
}

#[test]
fn binary_file_is_never_modified() {
    let dir = setup_files(&[("text.txt", "hello world\n")]);
    let bin_path = dir.path().join("data.bin");
    let bin_content: &[u8] = b"hello\x00world\x00hello";
    fs::write(&bin_path, bin_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    // The text file changes, the binary file (NUL bytes) must not.
    let text = fs::read_to_string(dir.path().join("text.txt")).unwrap();
    assert_eq!(text, "goodbye world\n");
    let bin = fs::read(&bin_path).unwrap();
    assert_eq!(
        bin, bin_content,
        "binary files must be skipped, not rewritten"
    );
}
