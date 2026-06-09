mod common;

use common::*;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

// ── Integration tests for --script mode ──

#[test]
fn script_replace_modifies_files() {
    let dir = setup_files(&[
        ("a.txt", "hello world\nhello again\n"),
        ("b.txt", "hello from b\n"),
    ]);

    let script_content = r#"
# Rename hello to goodbye
replace "hello" "goodbye"
"#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let a = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert!(a.contains("goodbye world"), "a.txt should be modified");
    assert!(
        a.contains("goodbye again"),
        "a.txt second line should be modified"
    );
    assert!(!a.contains("hello"), "a.txt should not contain 'hello'");

    let b = fs::read_to_string(dir.path().join("b.txt")).unwrap();
    assert!(b.contains("goodbye from b"), "b.txt should be modified");
}

#[test]
fn script_multi_operations() {
    let dir = setup_single_file(
        "code.txt",
        "old_name = value\n# TODO: remove this\nkeep this line\nold_name = other\n",
    );

    let script_content = r#"
# Step 1: rename old_name to new_name
replace "old_name" "new_name"

# Step 2: delete TODO lines
delete -e "^#\s*TODO:"
"#;
    let script_path = dir.path().join("refactor.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("code.txt")).unwrap();
    assert!(
        content.contains("new_name"),
        "old_name should be replaced with new_name"
    );
    assert!(
        !content.contains("old_name"),
        "old_name should no longer appear"
    );
    assert!(!content.contains("TODO"), "TODO line should be deleted");
    assert!(
        content.contains("keep this line"),
        "non-matching lines should be preserved"
    );
}

#[test]
fn script_dry_run_does_not_modify() {
    let original = "hello world\n";
    let dir = setup_single_file("test.txt", original);

    let script_content = r#"replace "hello" "goodbye""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--dry-run", "--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(
        content, original,
        "File should be unchanged in dry-run mode"
    );
}

#[test]
fn script_quiet_mode_suppresses_output() {
    let dir = setup_single_file("test.txt", "hello world\n");

    let script_content = r#"replace "hello" "goodbye""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-q", "--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("goodbye"), "File should still be modified");
}

#[test]
fn script_count_mode_prints_number() {
    let dir = setup_single_file("test.txt", "foo bar\nfoo baz\nno match\nfoo end\n");

    // Use --glob *.txt to avoid the script file itself being processed
    let script_content = r#"replace "foo" "replaced" --glob "*.txt""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-c", "--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let count_str = String::from_utf8(output.stdout).unwrap();
    let count: usize = count_str.trim().parse().unwrap();
    assert_eq!(count, 3);
}

#[test]
fn script_per_op_glob_scopes_operation() {
    let dir = setup_files(&[("code.rs", "old_name\n"), ("readme.txt", "old_name\n")]);

    // Only replace in .rs files
    let script_content = r#"replace "old_name" "new_name" --glob "*.rs""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let rs_content = fs::read_to_string(dir.path().join("code.rs")).unwrap();
    let txt_content = fs::read_to_string(dir.path().join("readme.txt")).unwrap();

    assert!(
        rs_content.contains("new_name"),
        "*.rs files should be modified"
    );
    assert_eq!(
        txt_content, "old_name\n",
        "*.txt files should be untouched when --glob scopes to *.rs"
    );
}

#[test]
fn script_backup_creates_bak_files() {
    let dir = setup_single_file("test.txt", "original content\n");

    let script_content = r#"replace "original" "modified""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("modified content"));

    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(backup_path.exists(), "Backup file should exist");
    let backup = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(backup, "original content\n");
}

#[test]
fn script_multiline_replace_spans_lines() {
    let dir = setup_single_file("test.txt", "one\ntwo\nthree\n");

    // \n inside a double-quoted script string is an escape for a real newline.
    let script_content = "replace -U \"one\\ntwo\" \"1\\n2\" --glob \"*.txt\"\n";
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "1\n2\nthree\n");
}

#[test]
fn script_multiline_rejected_for_transform() {
    let dir = TempDir::new().unwrap();
    let script_path = dir.path().join("bad.rip");
    fs::write(&script_path, "transform \"x\" --mode upper --multiline\n").unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--multiline is not supported"));
}

#[test]
fn script_nonexistent_file_exits_with_error() {
    let dir = TempDir::new().unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", "/nonexistent/path/ops.rip"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read script"));
}

#[test]
fn script_parse_error_exits_with_error() {
    let dir = TempDir::new().unwrap();
    let script_path = dir.path().join("bad.rip");
    fs::write(&script_path, "frobnicate \"hello\" \"world\"\n").unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown operation"));
}

#[test]
fn script_empty_script_exits_with_error() {
    let dir = TempDir::new().unwrap();
    let script_path = dir.path().join("empty.rip");
    fs::write(&script_path, "# Only comments here\n\n").unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("no operations"));
}

#[test]
fn script_delete_operation() {
    let dir = setup_single_file("test.txt", "keep this\ndelete this line\nkeep this too\n");

    let script_content = r#"delete "delete this""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("keep this"));
    assert!(content.contains("keep this too"));
    assert!(!content.contains("delete this line"));
}

#[test]
fn script_insert_after_operation() {
    let dir = setup_single_file("test.txt", "use serde;\nfn main() {}\n");

    let script_content = r#"insert_after "use serde;" "use serde_json;""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(
        content.contains("use serde;\nuse serde_json;\n"),
        "insert_after should add the line after the match. Got: {content}"
    );
}

#[test]
fn script_insert_before_operation() {
    let dir = setup_single_file("test.txt", "fn main() {\n    println!(\"hi\");\n}\n");

    let script_content = r#"insert_before "fn main" "// Entry point""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(
        content.contains("// Entry point\nfn main"),
        "insert_before should add the line before the match. Got: {content}"
    );
}

#[test]
fn script_replace_line_operation() {
    let dir = setup_single_file("config.txt", "version = 1\nname = app\n");

    let script_content = r#"replace_line "version = 1" "version = 2""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("config.txt")).unwrap();
    assert!(content.contains("version = 2"), "Line should be replaced");
    assert!(!content.contains("version = 1"), "Old line should be gone");
    assert!(
        content.contains("name = app"),
        "Other lines should be preserved"
    );
}

#[test]
fn script_no_matches_exits_with_code_1() {
    let dir = setup_single_file("test.txt", "hello world\n");

    // Use --glob *.txt to avoid the script file itself being processed
    let script_content = r#"replace "zzz_nonexistent" "replacement" --glob "*.txt""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(1);
}

#[test]
fn script_regex_replace() {
    let dir = setup_single_file("code.txt", "fn old_handler() {\n}\nfn old_parser() {\n}\n");

    let script_content = r#"replace -e "fn old_(\w+)" "fn new_$1""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("code.txt")).unwrap();
    assert!(
        content.contains("fn new_handler"),
        "Should have new_handler"
    );
    assert!(content.contains("fn new_parser"), "Should have new_parser");
    assert!(
        !content.contains("fn old_"),
        "Should not have old_ functions"
    );
}

#[test]
fn script_case_insensitive_replace() {
    let dir = setup_single_file("test.txt", "Hello World\nHELLO WORLD\nhello world\n");

    let script_content = r#"replace -i "hello" "greetings""#;
    let script_path = dir.path().join("ops.rip");
    fs::write(&script_path, script_content).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--script", script_path.to_str().unwrap()])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content.matches("greetings").count(), 3);
}
