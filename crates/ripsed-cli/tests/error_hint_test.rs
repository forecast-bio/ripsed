mod common;

use common::*;
use std::fs;

#[test]
fn invalid_regex_error_has_hint_and_pattern_in_context() {
    let dir = setup_files(&[("test.txt", "some content\n")]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{
                "op": "replace",
                "find": "fn (unclosed",
                "replace": "x",
                "regex": true
            }}],
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
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let errors = resp["errors"].as_array().unwrap();
    assert!(
        !errors.is_empty(),
        "Should produce an error for invalid regex"
    );

    let error = &errors[0];
    assert_eq!(error["code"], "invalid_regex");

    // Hint must be non-empty
    let hint = error["hint"].as_str().unwrap();
    assert!(
        !hint.is_empty(),
        "Hint should be non-empty for invalid_regex"
    );

    // Hint should include the pattern
    assert!(
        hint.contains("fn (unclosed"),
        "Hint should mention the invalid pattern, got: {hint}"
    );

    // Context should include the pattern
    let context = &error["context"];
    assert_eq!(
        context["pattern"], "fn (unclosed",
        "Context should include the regex pattern"
    );
}

#[test]
fn invalid_request_error_has_hint() {
    let request = r#"{"version": "1", "operations": []}"#;

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let errors = resp["errors"].as_array().unwrap();
    assert!(!errors.is_empty());

    let error = &errors[0];
    assert_eq!(error["code"], "invalid_request");

    let hint = error["hint"].as_str().unwrap();
    assert!(
        !hint.is_empty(),
        "Hint should be non-empty for invalid_request"
    );

    let message = error["message"].as_str().unwrap();
    assert!(!message.is_empty(), "Message should be non-empty");
}

#[test]
fn malformed_json_error_has_hint() {
    let request = "not json at all {{{";

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let errors = resp["errors"].as_array().unwrap();
    assert!(!errors.is_empty());

    let error = &errors[0];
    assert_eq!(error["code"], "invalid_request");

    let hint = error["hint"].as_str().unwrap();
    assert!(
        !hint.is_empty(),
        "Hint should be non-empty for malformed JSON"
    );
    assert!(
        hint.contains("JSON") || hint.contains("json") || hint.contains("schema"),
        "Hint should mention JSON format, got: {hint}"
    );
}

#[test]
fn unknown_version_error_has_hint_with_supported_versions() {
    let request =
        r#"{"version": "42", "operations": [{"op": "replace", "find": "a", "replace": "b"}]}"#;

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let errors = resp["errors"].as_array().unwrap();
    assert!(!errors.is_empty());

    let error = &errors[0];
    let hint = error["hint"].as_str().unwrap();
    assert!(!hint.is_empty());
    // Hint should tell the user what version to use
    assert!(
        hint.contains("1"),
        "Hint should mention supported version '1', got: {hint}"
    );
}

#[test]
fn invalid_regex_error_includes_operation_index() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    // Send a batch where the second operation has an invalid regex
    let request = format!(
        r#"{{
            "version": "1",
            "operations": [
                {{"op": "replace", "find": "content", "replace": "ok"}},
                {{"op": "replace", "find": "[invalid", "replace": "x", "regex": true}}
            ],
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
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let errors = resp["errors"].as_array().unwrap();
    assert!(!errors.is_empty());

    let error = &errors[0];
    assert_eq!(error["code"], "invalid_regex");
    // The error should reference operation index 1 (the second operation)
    assert_eq!(
        error["operation_index"], 1,
        "Error should reference the second operation (index 1)"
    );
}

#[test]
fn no_matches_in_human_mode_prints_to_stderr() {
    let dir = setup_files(&[("test.txt", "hello world\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["zzz_no_match_pattern_zzz", "replacement"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(1);

    // File should be unchanged
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "hello world\n");
}

#[test]
fn invalid_regex_in_human_mode_prints_error() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-e", "[unclosed", "replacement"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicates::str::contains("ripsed:"));
}

#[test]
fn all_error_codes_produce_nonempty_hints() {
    // Test that each error constructor produces a non-empty hint.
    // We test this through the JSON API by triggering different error conditions.

    // 1. invalid_request: empty operations
    {
        let output = assert_cmd::cargo_bin_cmd!("ripsed")
            .args(["--json"])
            .write_stdin(r#"{"version": "1", "operations": []}"#)
            .output()
            .unwrap();

        let stdout = String::from_utf8(output.stdout).unwrap();
        let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let errors = resp["errors"].as_array().unwrap();
        assert!(!errors.is_empty(), "invalid_request: should have errors");
        let hint = errors[0]["hint"].as_str().unwrap();
        assert!(
            !hint.is_empty(),
            "invalid_request error should have non-empty hint"
        );
    }

    // 2. invalid_regex: bad regex pattern
    {
        let dir = setup_files(&[("t.txt", "x\n")]);
        let request = format!(
            r#"{{
                "version": "1",
                "operations": [{{"op": "replace", "find": "(bad", "replace": "x", "regex": true}}],
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
        let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let errors = resp["errors"].as_array().unwrap();
        assert!(!errors.is_empty(), "invalid_regex: should have errors");
        let hint = errors[0]["hint"].as_str().unwrap();
        assert!(
            !hint.is_empty(),
            "invalid_regex error should have non-empty hint"
        );
    }

    // 3. invalid_request: malformed JSON
    {
        let output = assert_cmd::cargo_bin_cmd!("ripsed")
            .args(["--json"])
            .write_stdin("{ broken }")
            .output()
            .unwrap();

        let stdout = String::from_utf8(output.stdout).unwrap();
        let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let errors = resp["errors"].as_array().unwrap();
        assert!(!errors.is_empty(), "malformed json: should have errors");
        let hint = errors[0]["hint"].as_str().unwrap();
        assert!(
            !hint.is_empty(),
            "malformed JSON error should have non-empty hint"
        );
    }

    // 4. invalid_request: unknown version
    {
        let output = assert_cmd::cargo_bin_cmd!("ripsed")
            .args(["--json"])
            .write_stdin(r#"{"version": "999", "operations": [{"op": "replace", "find": "a", "replace": "b"}]}"#)
            .output()
            .unwrap();

        let stdout = String::from_utf8(output.stdout).unwrap();
        let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        let errors = resp["errors"].as_array().unwrap();
        assert!(!errors.is_empty(), "unknown version: should have errors");
        let hint = errors[0]["hint"].as_str().unwrap();
        assert!(
            !hint.is_empty(),
            "unknown version error should have non-empty hint"
        );
    }
}
