mod common;

use common::*;
use predicates::prelude::*;
use std::fs;

// ---------------------------------------------------------------------------
// Task 1: undo end-to-end tests
// ---------------------------------------------------------------------------

#[test]
fn undo_restores_file_after_replacement() {
    let dir = setup_files(&[("test.txt", "hello world\n")]);

    // Apply replacement (file mode, cwd = temp dir)
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Verify the file was changed
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "goodbye world\n");

    // Undo
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Verify the file is restored
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "hello world\n");
}

#[test]
fn undo_multiple_operations_with_count() {
    let dir = setup_files(&[("a.txt", "alpha content\n"), ("b.txt", "alpha content\n")]);

    // First replacement: alpha -> beta (affects both files)
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["alpha", "beta"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Verify both files changed
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "beta content\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "beta content\n"
    );

    // Undo with count=2 to restore both files
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo", "2"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Verify both files are restored
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "alpha content\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "alpha content\n"
    );
}

#[test]
fn undo_on_empty_log_exits_with_code_1() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    // No prior operations, so undo log is empty
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("nothing to undo"));
}

#[test]
fn undo_list_shows_entries_after_operations() {
    let dir = setup_files(&[("test.txt", "hello world\n")]);

    // Apply a replacement to create an undo entry
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Run --undo-list and check output lists the file
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo-list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("test.txt"));
}

#[test]
fn undo_list_on_empty_log_prints_message() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    // No prior operations
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo-list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("undo log is empty"));
}

#[test]
fn undo_after_multiple_separate_operations() {
    let dir = setup_files(&[("test.txt", "aaa bbb ccc\n")]);

    // First replacement: aaa -> xxx
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["aaa", "xxx"])
        .current_dir(dir.path())
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(dir.path().join("test.txt")).unwrap(),
        "xxx bbb ccc\n"
    );

    // Second replacement: bbb -> yyy
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["bbb", "yyy"])
        .current_dir(dir.path())
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(dir.path().join("test.txt")).unwrap(),
        "xxx yyy ccc\n"
    );

    // Undo last operation (bbb -> yyy)
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be back to after first replacement
    assert_eq!(
        fs::read_to_string(dir.path().join("test.txt")).unwrap(),
        "xxx bbb ccc\n"
    );

    // Undo again (aaa -> xxx)
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be fully restored
    assert_eq!(
        fs::read_to_string(dir.path().join("test.txt")).unwrap(),
        "aaa bbb ccc\n"
    );
}

// ---------------------------------------------------------------------------
// JSON mode undo tests
// ---------------------------------------------------------------------------

#[test]
fn json_undo_restores_file_after_replacement() {
    let dir = setup_files(&[("test.txt", "hello world\n")]);

    // Apply replacement via JSON mode (dry_run: false)
    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "hello", "replace": "goodbye"}}],
            "options": {{"dry_run": false, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    // Verify the file was changed
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "goodbye world\n");

    // Undo via JSON mode
    let undo_request = r#"{"undo": {"last": 1}}"#;
    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(undo_request)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    // Verify undo response schema
    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["version"], "1");
    assert_eq!(resp["success"], true);
    assert_eq!(resp["undo"]["operations_reverted"], 1);
    assert_eq!(resp["undo"]["files_restored"], 1);

    // Verify the file is restored
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "hello world\n");
}

#[test]
fn json_undo_on_empty_log_reverts_zero() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    let undo_request = r#"{"undo": {"last": 1}}"#;
    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(undo_request)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["undo"]["operations_reverted"], 0);
    assert_eq!(resp["undo"]["files_restored"], 0);
}

#[test]
fn json_undo_multiple_with_count() {
    let dir = setup_files(&[("a.txt", "alpha content\n"), ("b.txt", "alpha content\n")]);

    // Replace alpha -> beta via JSON (affects both files)
    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "alpha", "replace": "beta"}}],
            "options": {{"dry_run": false, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    // Undo with count=2 to restore both files
    let undo_request = r#"{"undo": {"last": 2}}"#;
    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(undo_request)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["undo"]["files_restored"], 2);

    // Verify both files are restored
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "alpha content\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "alpha content\n"
    );
}
