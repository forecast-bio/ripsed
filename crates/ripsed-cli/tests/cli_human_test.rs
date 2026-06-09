mod common;

use common::*;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use tempfile::TempDir;

#[test]
fn simple_replace_modifies_file() {
    let dir = setup_single_file("test.txt", "hello world\nhello again\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("goodbye world"));
    assert!(content.contains("goodbye again"));
    assert!(!content.contains("hello"));
}

#[test]
fn regex_replace_with_captures() {
    let dir = setup_single_file(
        "code.rs",
        "fn old_handler() {\n    old_handler();\n}\nfn old_parser() {}\n",
    );

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-e", r"fn\s+old_(\w+)", "fn new_$1", "--glob", "*.rs"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("code.rs")).unwrap();
    assert!(content.contains("fn new_handler"));
    assert!(content.contains("fn new_parser"));
}

#[test]
fn no_matches_exits_with_code_1() {
    let dir = setup_single_file("test.txt", "hello world\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["zzz_nonexistent_pattern", "replacement"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(1);
}

#[test]
fn dry_run_does_not_modify_files() {
    let original = "hello world\nhello again\n";
    let dir = setup_single_file("test.txt", original);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--dry-run", "hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success()
        // The preview must actually show the proposed replacement — a
        // dry run that silently does nothing would also leave the file
        // unchanged, so checking the file alone proves nothing.
        .stdout(predicate::str::contains("goodbye world"))
        .stdout(predicate::str::contains("goodbye again"));

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(
        content, original,
        "File should be unchanged in dry-run mode"
    );
}

#[test]
fn dry_run_prints_diff_to_stdout() {
    let dir = setup_single_file("test.txt", "hello world\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--dry-run", "hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success()
        // A diff shows both sides: the line being removed and the line
        // replacing it. Requiring only one of the two would pass even if
        // the diff rendered nothing useful.
        .stdout(predicate::str::contains("hello world"))
        .stdout(predicate::str::contains("goodbye world"));
}

#[test]
fn pipe_mode_reads_stdin_writes_stdout() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["foo", "bar"])
        .write_stdin("foo baz foo\nanother foo line\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("bar baz bar"))
        .stdout(predicate::str::contains("another bar line"));
}

#[test]
fn pipe_mode_no_matches_outputs_original() {
    // Input passes through unchanged; exit code 1 = clean no-match
    // (ripgrep convention: 0 matched, 1 no matches, 2 error).
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["zzz", "yyy"])
        .write_stdin("hello world\n")
        .assert()
        .failure()
        .code(1)
        .stdout("hello world\n");
}

#[test]
fn delete_lines_removes_matching_lines() {
    let dir = setup_single_file(
        "test.txt",
        "keep this\ndelete this line\nkeep this too\ndelete also\n",
    );

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-d", "delete"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("keep this"));
    assert!(content.contains("keep this too"));
    assert!(!content.contains("delete this line"));
    assert!(!content.contains("delete also"));
}

#[test]
fn case_insensitive_replace() {
    let dir = setup_single_file("test.txt", "Hello World\nHELLO WORLD\nhello world\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--case-insensitive", "hello", "greetings"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content.matches("greetings").count(), 3);
    assert!(!content.contains("Hello"));
    assert!(!content.contains("HELLO"));
    assert!(!content.contains("hello"));
}

#[test]
fn glob_filter_only_touches_matching_files() {
    let dir = setup_files(&[
        ("code.rs", "old_name\n"),
        ("readme.txt", "old_name\n"),
        ("data.rs", "old_name\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["old_name", "new_name", "--glob", "*.rs"])
        .current_dir(dir.path())
        .assert()
        .success();

    let rs_content = fs::read_to_string(dir.path().join("code.rs")).unwrap();
    let txt_content = fs::read_to_string(dir.path().join("readme.txt")).unwrap();
    let data_content = fs::read_to_string(dir.path().join("data.rs")).unwrap();

    assert!(
        rs_content.contains("new_name"),
        "*.rs files should be modified"
    );
    assert!(
        data_content.contains("new_name"),
        "*.rs files should be modified"
    );
    assert_eq!(txt_content, "old_name\n", "*.txt files should be untouched");
}

#[test]
fn count_mode_prints_number() {
    let dir = setup_single_file("test.txt", "foo bar\nfoo baz\nno match\nfoo end\n");

    // Single run: -c outputs the count and modifies the file
    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-c", "foo", "replaced"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let count_str = String::from_utf8(output.stdout).unwrap();
    assert!(
        predicate::str::is_match(r"^\d+\n$")
            .unwrap()
            .eval(&count_str)
    );
    let count: usize = count_str.trim().parse().unwrap();
    assert_eq!(count, 3);
}

#[test]
fn quiet_mode_no_output() {
    let dir = setup_single_file("test.txt", "hello world\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-q", "hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    // File should still be modified
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("goodbye"));
}

#[test]
fn backup_mode_creates_bak_file() {
    let dir = setup_single_file("test.txt", "original content\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("modified content"));

    // Backup should exist with original content
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(backup_path.exists(), "Backup file should exist");
    let backup = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(backup, "original content\n");
}

#[test]
fn pipe_mode_regex_replace() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-e", r"(\d+)", "NUM"])
        .write_stdin("there are 42 cats and 7 dogs\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("there are NUM cats and NUM dogs"));
}

#[test]
fn pipe_mode_delete_lines() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-d", "remove"])
        .write_stdin("keep\nremove this\nkeep too\nremove also\n")
        .assert()
        .success()
        .stdout("keep\nkeep too\n");
}

#[test]
fn pipe_mode_case_insensitive() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--case-insensitive", "FOO", "bar"])
        .write_stdin("Foo is foo and FOO\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("bar is bar and bar"));
}

#[test]
fn replace_preserves_trailing_newline() {
    let dir = setup_single_file("test.txt", "hello\nworld\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "hi"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(
        content.ends_with('\n'),
        "Trailing newline should be preserved"
    );
}

#[test]
fn missing_find_pattern_fails() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .write_stdin("some input\n")
        .assert()
        .failure();
}

#[test]
fn insert_after_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["marker", "--after", "INSERTED LINE"])
        .write_stdin("before\nmarker line\nafter\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("marker line\nINSERTED LINE\n"));
}

#[test]
fn insert_before_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["marker", "--before", "INSERTED LINE"])
        .write_stdin("before\nmarker line\nafter\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("INSERTED LINE\nmarker line\n"));
}

#[test]
fn replace_line_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["old_line", "--replace-line", "completely new line"])
        .write_stdin("keep\nold_line content\nkeep too\n")
        .assert()
        .success()
        .stdout("keep\ncompletely new line\nkeep too\n");
}

#[test]
fn multiple_files_in_directory() {
    let dir = setup_files(&[
        ("a.txt", "hello from a\n"),
        ("b.txt", "hello from b\n"),
        ("sub/c.txt", "hello from c\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    for (name, _) in &[("a.txt", ()), ("b.txt", ()), ("sub/c.txt", ())] {
        let content = fs::read_to_string(dir.path().join(name)).unwrap();
        assert!(
            content.contains("goodbye"),
            "File {name} should contain 'goodbye'"
        );
    }
}

#[test]
fn hidden_files_ignored_by_default() {
    let dir = setup_files(&[
        ("visible.txt", "target_text\n"),
        (".hidden.txt", "target_text\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_text", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let visible = fs::read_to_string(dir.path().join("visible.txt")).unwrap();
    let hidden = fs::read_to_string(dir.path().join(".hidden.txt")).unwrap();

    assert!(
        visible.contains("replaced"),
        "Visible file should be modified"
    );
    assert_eq!(
        hidden, "target_text\n",
        "Hidden file should be untouched by default"
    );
}

#[test]
fn hidden_files_included_with_flag() {
    let dir = setup_files(&[
        ("visible.txt", "target_text\n"),
        (".hidden.txt", "target_text\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--hidden", "target_text", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let visible = fs::read_to_string(dir.path().join("visible.txt")).unwrap();
    let hidden = fs::read_to_string(dir.path().join(".hidden.txt")).unwrap();

    assert!(visible.contains("replaced"));
    assert!(
        hidden.contains("replaced"),
        "Hidden file should be modified with --hidden"
    );
}

#[test]
fn replace_empty_string_removes_occurrences() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["remove_me", ""])
        .write_stdin("keep remove_me keep\n")
        .assert()
        .success()
        .stdout("keep  keep\n");
}

// ── Parallel discovery wiring ──

#[test]
fn discover_files_auto_finds_files_in_small_directory() {
    // Verifies that discover_files_auto (wired in file_mode) works with a small
    // directory. This exercises the serial path of discover_files_auto.
    let dir = setup_files(&[
        ("a.txt", "hello world\n"),
        ("b.txt", "hello world\n"),
        ("sub/c.txt", "hello world\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    for name in &["a.txt", "b.txt", "sub/c.txt"] {
        let content = fs::read_to_string(dir.path().join(name)).unwrap();
        assert!(
            content.contains("goodbye"),
            "File {name} should have been discovered and modified"
        );
    }
}

// ── Config default merging ──

#[test]
fn config_defaults_backup_creates_bak_file() {
    let dir = setup_single_file("test.txt", "original content\n");
    // Write a .ripsed.toml that enables backup by default
    fs::write(
        dir.path().join(".ripsed.toml"),
        "[defaults]\nbackup = true\n",
    )
    .unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("modified content"));

    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        backup_path.exists(),
        "Config defaults.backup=true should create backup files"
    );
    let backup = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(backup, "original content\n");
}

#[test]
fn config_defaults_max_depth_limits_recursion() {
    let dir = setup_files(&[
        ("shallow.txt", "target_text\n"),
        ("a/medium.txt", "target_text\n"),
        ("a/b/deep.txt", "target_text\n"),
    ]);
    // max_depth = 1 means only the root directory itself
    fs::write(
        dir.path().join(".ripsed.toml"),
        "[defaults]\nmax_depth = 1\n",
    )
    .unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_text", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let shallow = fs::read_to_string(dir.path().join("shallow.txt")).unwrap();
    assert!(
        shallow.contains("replaced"),
        "Shallow file should be modified"
    );

    let deep = fs::read_to_string(dir.path().join("a/b/deep.txt")).unwrap();
    assert_eq!(
        deep, "target_text\n",
        "Deep file should be untouched due to max_depth=1"
    );
}

#[test]
fn config_defaults_gitignore_false_includes_gitignored_files() {
    let dir = setup_single_file("ignored.log", "target_text\n");
    // Set up a git repo with .gitignore
    fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    // Initialize a git repo so .gitignore is respected
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // With default config (gitignore=true), the .log file should be ignored.
    // With gitignore=false in config, it should be found.
    fs::write(
        dir.path().join(".ripsed.toml"),
        "[defaults]\ngitignore = false\n",
    )
    .unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["target_text", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("ignored.log")).unwrap();
    assert!(
        content.contains("replaced"),
        "With gitignore=false, gitignored files should be processed"
    );
}

#[test]
fn cli_no_gitignore_overrides_config_gitignore_true() {
    let dir = setup_single_file("ignored.log", "target_text\n");
    fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Config says gitignore=true (the default), but CLI says --no-gitignore
    fs::write(
        dir.path().join(".ripsed.toml"),
        "[defaults]\ngitignore = true\n",
    )
    .unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--no-gitignore", "target_text", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("ignored.log")).unwrap();
    assert!(
        content.contains("replaced"),
        "--no-gitignore should override config defaults.gitignore=true"
    );
}

#[test]
fn cli_max_depth_overrides_config_max_depth() {
    let dir = setup_files(&[
        ("shallow.txt", "target_text\n"),
        ("a/b/deep.txt", "target_text\n"),
    ]);
    // Config sets max_depth = 1
    fs::write(
        dir.path().join(".ripsed.toml"),
        "[defaults]\nmax_depth = 1\n",
    )
    .unwrap();

    // CLI --max-depth 10 should override the config
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--max-depth", "10", "target_text", "replaced"])
        .current_dir(dir.path())
        .assert()
        .success();

    let deep = fs::read_to_string(dir.path().join("a/b/deep.txt")).unwrap();
    assert!(
        deep.contains("replaced"),
        "CLI --max-depth should override config defaults.max_depth"
    );
}

// ── --pipe flag ──

#[test]
fn pipe_flag_forces_pipe_mode() {
    // --pipe should read from stdin and write to stdout, even without
    // auto-detection of piped input.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "hello", "goodbye"])
        .write_stdin("hello world\nhello again\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("goodbye world"))
        .stdout(predicate::str::contains("goodbye again"));
}

#[test]
fn pipe_flag_short_form() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-p", "foo", "bar"])
        .write_stdin("foo baz\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("bar baz"));
}

// ── --follow flag ──

#[cfg(unix)]
#[test]
fn follow_flag_follows_symlinks() {
    let dir = TempDir::new().unwrap();
    let real_dir = dir.path().join("real");
    fs::create_dir_all(&real_dir).unwrap();
    fs::write(real_dir.join("target.txt"), "hello world\n").unwrap();

    // Create a symlink to the real directory
    let link_path = dir.path().join("link");
    symlink(&real_dir, &link_path).unwrap();

    // Without --follow, symlinked directories are not followed, so the file
    // under "link/" may not be discovered. With --follow it should be.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--follow", "hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    // The file under the real directory should be modified (found via both
    // real path and the followed symlink).
    let content = fs::read_to_string(real_dir.join("target.txt")).unwrap();
    assert!(
        content.contains("goodbye"),
        "With --follow, files behind symlinks should be discovered"
    );
}

// ── --in-place removed ──

#[test]
fn in_place_flag_is_removed() {
    // Verify that --in-place is no longer accepted
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--in-place", "hello", "goodbye"])
        .write_stdin("hello\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument"));
}
