mod common;

use common::*;
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Task 4: cross-platform behavior tests
// ---------------------------------------------------------------------------

#[test]
fn crlf_line_endings_preserved_after_replacement() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("crlf.txt");
    // Write CRLF content as raw bytes to ensure exact line endings
    fs::write(&file_path, b"hello world\r\nfoo bar\r\n").unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read(&file_path).unwrap();
    let text = String::from_utf8(content).unwrap();

    assert!(
        text.contains("goodbye world"),
        "Replacement should have been applied"
    );
    // CRLF should be preserved
    assert!(
        text.contains("\r\n"),
        "CRLF line endings should be preserved after replacement"
    );
    // Verify no bare LF (without preceding CR) appears
    let lines_with_crlf = text.matches("\r\n").count();
    let lines_with_lf = text.matches('\n').count();
    assert_eq!(
        lines_with_crlf, lines_with_lf,
        "All newlines should be CRLF, not bare LF"
    );
}

#[test]
fn unicode_filenames_are_processed() {
    let dir = TempDir::new().unwrap();
    let unicode_name = "datos_\u{00e9}t\u{00e9}.txt"; // datos_ete.txt with accents
    let file_path = dir.path().join(unicode_name);
    fs::write(&file_path, "target_word in unicode file\n").unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_word", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(&file_path).unwrap();
    assert!(
        content.contains("replaced"),
        "File with unicode name should be processed"
    );
    assert!(
        !content.contains("target_word"),
        "Original pattern should be gone"
    );
}

#[test]
fn unicode_content_is_handled_correctly() {
    let dir = setup_files(&[("unicode.txt", "Hello \u{4e16}\u{754c}\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["Hello", "Goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("unicode.txt")).unwrap();
    assert_eq!(
        content, "Goodbye \u{4e16}\u{754c}\n",
        "Unicode content should be preserved after replacement"
    );
}

#[test]
fn json_path_helper_escapes_backslashes() {
    let dir = TempDir::new().unwrap();
    let escaped = json_path(&dir);

    // The escaped path should never contain a lone backslash.
    // On Windows, backslashes should be doubled for JSON embedding.
    // On Unix, there are typically no backslashes, so this is a no-op.
    assert!(
        !escaped.contains('\\') || escaped.contains("\\\\"),
        "json_path should escape all backslashes for JSON embedding"
    );

    // Verify it can be embedded in a JSON string without parse errors
    let json_str = format!(r#"{{"path": "{}"}}"#, escaped);
    let parsed: serde_json::Value = serde_json::from_str(&json_str)
        .expect("Path escaped by json_path() should produce valid JSON");
    assert!(parsed["path"].is_string(), "Parsed path should be a string");
}

#[test]
fn mixed_line_endings_in_same_file() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("mixed.txt");
    // Write a file with mixed LF and CRLF endings
    fs::write(&file_path, b"line_one\nline_two\r\nline_three\n").unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["line_two", "replaced_two"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read(&file_path).unwrap();
    let text = String::from_utf8(content).unwrap();
    assert!(
        text.contains("replaced_two"),
        "Replacement should be applied in files with mixed line endings"
    );
}

#[test]
fn json_mode_with_escaped_path() {
    let dir = setup_files(&[("test.txt", "old_value\n")]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "old_value", "replace": "new_value"}}],
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
    let resp: serde_json::Value = serde_json::from_str(&stdout)
        .expect("JSON request with escaped path should produce a valid response");

    assert_eq!(resp["version"], "1");
    assert_eq!(resp["success"], true);
}
