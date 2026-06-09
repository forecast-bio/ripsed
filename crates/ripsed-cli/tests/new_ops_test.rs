mod common;

use common::*;
use predicates::prelude::*;
use std::fs;

// ── Transform tests ──

#[test]
fn transform_upper_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "hello", "--transform", "upper"])
        .write_stdin("hello\n")
        .assert()
        .success()
        .stdout("HELLO\n");
}

#[test]
fn transform_lower_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "HELLO", "--transform", "lower"])
        .write_stdin("HELLO WORLD\n")
        .assert()
        .success()
        .stdout("hello WORLD\n");
}

#[test]
fn transform_camel_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "snake_case_name", "--transform", "camel"])
        .write_stdin("snake_case_name\n")
        .assert()
        .success()
        .stdout("snakeCaseName\n");
}

#[test]
fn transform_snake_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "camelCase", "--transform", "snake"])
        .write_stdin("camelCase\n")
        .assert()
        .success()
        .stdout("camel_case\n");
}

#[test]
fn transform_title_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "hello world", "--transform", "title"])
        .write_stdin("hello world\n")
        .assert()
        .success()
        .stdout("Hello World\n");
}

#[test]
fn transform_upper_via_json() {
    let request = r#"{
        "version": "1",
        "operations": [{"op": "transform", "find": "text", "mode": "upper"}],
        "options": {"dry_run": true}
    }"#;

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .assert()
        .success();
}

#[test]
fn transform_upper_file_mode() {
    let dir = setup_single_file("test.txt", "hello world\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "--transform", "upper"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "HELLO world\n");
}

#[test]
fn transform_upper_no_match_preserves_input() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "zzz_no_match", "--transform", "upper"])
        .write_stdin("hello world\n")
        .assert()
        .success()
        .stdout("hello world\n");
}

#[test]
fn transform_upper_multiple_lines() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "foo", "--transform", "upper"])
        .write_stdin("foo bar\nbaz foo\nno match\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("FOO bar"))
        .stdout(predicate::str::contains("baz FOO"))
        .stdout(predicate::str::contains("no match"));
}

// ── Surround tests ──

#[test]
fn surround_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "word", "--surround", "(", ")"])
        .write_stdin("word\n")
        .assert()
        .success()
        .stdout("(word)\n");
}

#[test]
fn surround_wraps_matching_lines_only() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "target", "--surround", ">>> ", " <<<"])
        .write_stdin("keep this\ntarget line\nkeep too\n")
        .assert()
        .success()
        .stdout("keep this\n>>> target line <<<\nkeep too\n");
}

#[test]
fn surround_via_json() {
    let request = r#"{
        "version": "1",
        "operations": [{"op": "surround", "find": "line", "prefix": ">>> ", "suffix": " <<<"}],
        "options": {"dry_run": true}
    }"#;

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .assert()
        .success();
}

#[test]
fn surround_file_mode() {
    let dir = setup_single_file("test.txt", "before\ntarget line\nafter\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target", "--surround", "[", "]"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("[target line]"));
    assert!(content.contains("before"));
    assert!(content.contains("after"));
}

#[test]
fn surround_multiple_matching_lines() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--surround", "<<", ">>"])
        .write_stdin("match one\nno hit\nmatch two\n")
        .assert()
        .success()
        .stdout("<<match one>>\nno hit\n<<match two>>\n");
}

// ── Indent tests ──

#[test]
fn indent_adds_spaces_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--indent", "4"])
        .write_stdin("match line\nother\n")
        .assert()
        .success()
        .stdout("    match line\nother\n");
}

#[test]
fn indent_only_affects_matching_lines() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "target", "--indent", "2"])
        .write_stdin("keep\ntarget here\nalso keep\n")
        .assert()
        .success()
        .stdout("keep\n  target here\nalso keep\n");
}

#[test]
fn indent_file_mode() {
    let dir = setup_single_file("test.txt", "no indent\nindent me\nno indent\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["indent me", "--indent", "4"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("    indent me"));
    assert!(content.contains("no indent"));
}

// ── Dedent tests ──

#[test]
fn dedent_removes_leading_spaces_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--dedent", "2"])
        .write_stdin("  match line\nother\n")
        .assert()
        .success()
        .stdout("match line\nother\n");
}

#[test]
fn dedent_removes_only_up_to_amount() {
    // Line has 4 leading spaces, dedent by 2 should leave 2
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--dedent", "2"])
        .write_stdin("    match line\n")
        .assert()
        .success()
        .stdout("  match line\n");
}

#[test]
fn dedent_does_not_remove_more_than_available() {
    // Line has 1 leading space, dedent by 4 should remove only the 1 available space
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--dedent", "4"])
        .write_stdin(" match line\n")
        .assert()
        .success()
        .stdout("match line\n");
}

#[test]
fn dedent_only_affects_matching_lines() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "target", "--dedent", "4"])
        .write_stdin("    keep this\n    target here\n    also keep\n")
        .assert()
        .success()
        .stdout("    keep this\ntarget here\n    also keep\n");
}

// ── Indent + Dedent round-trip ──

#[test]
fn indent_then_dedent_roundtrip_restores_original() {
    let original = "match line\nother stuff\nmatch again\n";

    // Step 1: indent matching lines by 4
    let indented = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--indent", "4"])
        .write_stdin(original)
        .output()
        .unwrap();
    assert!(indented.status.success());
    let indented_text = String::from_utf8(indented.stdout).unwrap();

    // Verify indentation was applied
    assert!(indented_text.contains("    match line"));
    assert!(indented_text.contains("    match again"));

    // Step 2: dedent those same lines by 4
    let restored = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "match", "--dedent", "4"])
        .write_stdin(indented_text)
        .output()
        .unwrap();
    assert!(restored.status.success());
    let restored_text = String::from_utf8(restored.stdout).unwrap();

    assert_eq!(
        restored_text, original,
        "Round-trip indent/dedent should restore original text"
    );
}

// ── JSON mode tests with json_path helper ──

#[test]
fn transform_upper_via_json_with_file() {
    let dir = setup_single_file("test.txt", "hello world\n");

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "transform", "find": "hello", "mode": "upper"}}],
            "options": {{"dry_run": false, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    assert!(output.status.success());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "HELLO world\n");
}

#[test]
fn surround_via_json_with_file() {
    let dir = setup_single_file("test.txt", "target line\nother line\n");

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "surround", "find": "target", "prefix": ">>> ", "suffix": " <<<"}}],
            "options": {{"dry_run": false, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    assert!(output.status.success());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains(">>> target line <<<"));
    assert!(content.contains("other line"));
}

#[test]
fn indent_via_json_with_file() {
    let dir = setup_single_file("test.txt", "fn main() {\n    code();\n}\n");

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "indent", "find": "code", "amount": 4}}],
            "options": {{"dry_run": false, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    assert!(output.status.success());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("        code();"));
}

#[test]
fn dedent_via_json_with_file() {
    let dir = setup_single_file("test.txt", "    indented line\nnormal line\n");

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "dedent", "find": "indented", "amount": 4}}],
            "options": {{"dry_run": false, "root": "{}"}}
        }}"#,
        json_path(&dir)
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();

    assert!(output.status.success());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("indented line"));
    assert!(!content.contains("    indented line"));
    assert!(content.contains("normal line"));
}
