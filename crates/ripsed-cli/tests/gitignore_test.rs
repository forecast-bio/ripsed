mod common;

use common::*;
use std::fs;
use tempfile::TempDir;

/// Run `git init` in the given directory.
fn git_init(dir: &TempDir) {
    let output = std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// Task 2: gitignore integration tests
// ---------------------------------------------------------------------------

#[test]
fn gitignore_excludes_matching_files() {
    let dir = setup_files(&[
        ("readme.txt", "target_word in readme\n"),
        ("debug.log", "target_word in log\n"),
        (".gitignore", "*.log\n"),
    ]);
    git_init(&dir);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_word", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    // .txt file should be modified
    let txt_content = fs::read_to_string(dir.path().join("readme.txt")).unwrap();
    assert!(
        txt_content.contains("replaced"),
        "readme.txt should be modified"
    );

    // .log file should be untouched (gitignored)
    let log_content = fs::read_to_string(dir.path().join("debug.log")).unwrap();
    assert_eq!(
        log_content, "target_word in log\n",
        ".log file should be untouched because it is gitignored"
    );
}

#[test]
fn no_gitignore_flag_includes_ignored_files() {
    let dir = setup_files(&[
        ("readme.txt", "target_word in readme\n"),
        ("debug.log", "target_word in log\n"),
        (".gitignore", "*.log\n"),
    ]);
    git_init(&dir);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--no-gitignore", "target_word", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Both files should be modified
    let txt_content = fs::read_to_string(dir.path().join("readme.txt")).unwrap();
    assert!(
        txt_content.contains("replaced"),
        "readme.txt should be modified"
    );

    let log_content = fs::read_to_string(dir.path().join("debug.log")).unwrap();
    assert!(
        log_content.contains("replaced"),
        ".log file should be modified with --no-gitignore"
    );
}

#[test]
fn nested_gitignore_in_subdirectory() {
    let dir = setup_files(&[
        ("top.txt", "target_word in top\n"),
        ("sub/code.txt", "target_word in sub\n"),
        ("sub/temp.tmp", "target_word in tmp\n"),
        (".gitignore", ""),
        ("sub/.gitignore", "*.tmp\n"),
    ]);
    git_init(&dir);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_word", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    // top.txt should be modified
    let top_content = fs::read_to_string(dir.path().join("top.txt")).unwrap();
    assert!(
        top_content.contains("replaced"),
        "top.txt should be modified"
    );

    // sub/code.txt should be modified
    let code_content = fs::read_to_string(dir.path().join("sub/code.txt")).unwrap();
    assert!(
        code_content.contains("replaced"),
        "sub/code.txt should be modified"
    );

    // sub/temp.tmp should be untouched (ignored by sub/.gitignore)
    let tmp_content = fs::read_to_string(dir.path().join("sub/temp.tmp")).unwrap();
    assert_eq!(
        tmp_content, "target_word in tmp\n",
        "sub/temp.tmp should be untouched because it is gitignored by nested .gitignore"
    );
}

#[test]
fn gitignore_with_directory_pattern() {
    let dir = setup_files(&[
        ("src/main.txt", "target_word in src\n"),
        ("build/output.txt", "target_word in build\n"),
        (".gitignore", "build/\n"),
    ]);
    git_init(&dir);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_word", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    // src/main.txt should be modified
    let src_content = fs::read_to_string(dir.path().join("src/main.txt")).unwrap();
    assert!(
        src_content.contains("replaced"),
        "src/main.txt should be modified"
    );

    // build/output.txt should be untouched (entire build/ directory is ignored)
    let build_content = fs::read_to_string(dir.path().join("build/output.txt")).unwrap();
    assert_eq!(
        build_content, "target_word in build\n",
        "build/ directory should be untouched because it is gitignored"
    );
}

// ---------------------------------------------------------------------------
// JSON mode gitignore tests
// ---------------------------------------------------------------------------

#[test]
fn json_gitignore_excludes_matching_files() {
    let dir = setup_files(&[
        ("readme.txt", "target_word in readme\n"),
        ("debug.log", "target_word in log\n"),
        (".gitignore", "*.log\n"),
    ]);
    git_init(&dir);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "target_word", "replace": "replaced"}}],
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

    // .txt file should be modified
    let txt_content = fs::read_to_string(dir.path().join("readme.txt")).unwrap();
    assert!(
        txt_content.contains("replaced"),
        "readme.txt should be modified"
    );

    // .log file should be untouched (gitignored)
    let log_content = fs::read_to_string(dir.path().join("debug.log")).unwrap();
    assert_eq!(
        log_content, "target_word in log\n",
        ".log file should be untouched because it is gitignored"
    );
}

#[test]
fn json_gitignore_false_includes_ignored_files() {
    let dir = setup_files(&[
        ("readme.txt", "target_word in readme\n"),
        ("debug.log", "target_word in log\n"),
        (".gitignore", "*.log\n"),
    ]);
    git_init(&dir);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "target_word", "replace": "replaced"}}],
            "options": {{"dry_run": false, "gitignore": false, "root": "{}"}}
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

    // Both files should be modified
    let txt_content = fs::read_to_string(dir.path().join("readme.txt")).unwrap();
    assert!(
        txt_content.contains("replaced"),
        "readme.txt should be modified"
    );

    let log_content = fs::read_to_string(dir.path().join("debug.log")).unwrap();
    assert!(
        log_content.contains("replaced"),
        ".log file should be modified with gitignore: false"
    );
}

#[test]
fn json_gitignore_respects_directory_pattern() {
    let dir = setup_files(&[
        ("src/main.txt", "target_word in src\n"),
        ("build/output.txt", "target_word in build\n"),
        (".gitignore", "build/\n"),
    ]);
    git_init(&dir);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "target_word", "replace": "replaced"}}],
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

    let src_content = fs::read_to_string(dir.path().join("src/main.txt")).unwrap();
    assert!(
        src_content.contains("replaced"),
        "src/main.txt should be modified"
    );

    let build_content = fs::read_to_string(dir.path().join("build/output.txt")).unwrap();
    assert_eq!(
        build_content, "target_word in build\n",
        "build/ directory should be untouched because it is gitignored"
    );
}
