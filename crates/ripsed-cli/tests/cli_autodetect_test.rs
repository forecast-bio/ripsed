mod common;

use common::*;
use predicates::prelude::*;

#[test]
fn json_stdin_detected_as_agent_mode() {
    let dir = setup_files(&[("test.txt", "old_value content\n")]);

    // Pipe a valid ripsed JSON request via stdin without --json flag.
    // The auto-detect logic should recognize it as JSON because it starts
    // with '{' and contains "operations".
    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "old_value", "replace": "new_value"}}],
            "options": {{"dry_run": true, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .write_stdin(request)
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Should produce a JSON response (agent mode was auto-detected)
    let resp: serde_json::Value = serde_json::from_str(&stdout)
        .expect("Auto-detected JSON input should produce a JSON response");

    assert_eq!(resp["version"], "1");
    assert!(resp["success"].is_boolean());
    assert!(resp["results"].is_array());
}

#[test]
fn non_json_stdin_uses_pipe_mode() {
    // Plain text piped in should be treated as pipe mode, not JSON mode.
    // We need to provide find/replace args for pipe mode to work.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .write_stdin("hello world\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("goodbye world"));
}

#[test]
fn json_without_operations_key_uses_pipe_mode() {
    // A JSON object that does NOT have "operations" should fall through
    // to pipe mode (auto-detect sees JSON but not a ripsed request).
    // In pipe mode with no find pattern, it should fail.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .write_stdin(r#"{"key": "value", "nested": {"data": true}}"#)
        .assert()
        .failure();
}

#[test]
fn json_without_operations_key_pipe_mode_with_args() {
    // JSON without "operations" key piped in, but with find/replace args:
    // should be treated as plain text in pipe mode.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["key", "KEY"])
        .write_stdin(r#"{"key": "value"}"#)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""KEY""#));
}

#[test]
fn no_json_flag_forces_pipe_mode() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    // Even though this is valid ripsed JSON, --no-json forces pipe mode.
    let request = format!(
        r#"{{"operations": [{{"op": "replace", "find": "x", "replace": "y"}}], "options": {{"root": "{}"}}}}"#,
        json_path(&dir)
    );

    // With --no-json and find/replace args, it treats the JSON as plain text
    // and performs a text replacement on the JSON string itself.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--no-json", "operations", "OPERATIONS"])
        .write_stdin(request)
        .assert()
        .success()
        .stdout(predicate::str::contains("OPERATIONS"));
}

#[test]
fn no_json_flag_without_args_fails() {
    // --no-json forces pipe mode, but no find pattern means failure.
    let request = r#"{"operations": [{"op": "replace", "find": "x", "replace": "y"}]}"#;

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--no-json"])
        .write_stdin(request)
        .assert()
        .failure();
}

#[test]
fn json_flag_explicit_forces_json_mode() {
    let dir = setup_files(&[("test.txt", "target_text\n")]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "target_text", "replace": "replaced"}}],
            "options": {{"dry_run": true, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json flag should produce JSON response");

    assert_eq!(resp["version"], "1");
    assert_eq!(resp["success"], true);
}

#[test]
fn json_with_leading_whitespace_detected() {
    let dir = setup_files(&[("test.txt", "hello\n")]);

    // JSON with leading whitespace should still be auto-detected
    let request = format!(
        r#"   {{
            "version": "1",
            "operations": [{{"op": "replace", "find": "hello", "replace": "bye"}}],
            "options": {{"dry_run": true, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .write_stdin(request)
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout)
        .expect("JSON with leading whitespace should still be detected as agent mode");

    assert_eq!(resp["version"], "1");
}

#[test]
fn plain_text_starting_with_brace_but_invalid_json() {
    // Text that starts with { but is not valid JSON should use pipe mode.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .write_stdin("{hello} world\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("{goodbye} world"));
}
