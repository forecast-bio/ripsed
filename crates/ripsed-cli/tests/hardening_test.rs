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
fn multiline_flag_replaces_across_lines_in_file_mode() {
    let dir = setup_single_file("code.rs", "fn old(\n    x: u32,\n) {}\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-U", "old(\n    x: u32,\n)", "new(x: u32)"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("code.rs")).unwrap();
    assert_eq!(content, "fn new(x: u32) {}\n");
}

#[test]
fn multiline_flag_works_in_pipe_mode() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "-U", "-e", r"(\w+)\n(\w+)\n", "$2\n$1\n"])
        .write_stdin("alpha\nbeta\n")
        .assert()
        .success()
        .stdout("beta\nalpha\n");
}

#[test]
fn multiline_delete_removes_span_in_file_mode() {
    let dir = setup_single_file("doc.txt", "keep [S]\ngone\n[E] keep\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-d", "-U", "[S]\ngone\n[E]"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("doc.txt")).unwrap();
    assert_eq!(content, "keep  keep\n");
}

#[test]
fn multiline_conflicts_with_line_scoped_flags() {
    for conflicting in [
        vec!["-U", "--indent", "2", "x"],
        vec!["-U", "--after", "y", "x"],
        vec!["-U", "--transform", "upper", "x"],
        vec!["-U", "-n", "1:2", "x", "y"],
    ] {
        assert_cmd::cargo_bin_cmd!("ripsed")
            .args(&conflicting)
            .assert()
            .failure()
            .stderr(predicate::str::contains("cannot be used with"));
    }
}

#[test]
fn multiline_json_op_replaces_across_lines() {
    let dir = setup_files(&[("test.txt", "one\ntwo\nthree\n")]);
    let request = json_request(
        r#"{"op": "replace", "find": "one\ntwo", "replace": "1\n2", "multiline": true}"#,
        &format!(r#""dry_run": false, "root": "{}""#, json_path(&dir)),
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();
    assert!(output.status.success());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "1\n2\nthree\n");
}

#[test]
fn multiline_json_op_rejected_for_insert() {
    let request = json_request(
        r#"{"op": "insert_after", "find": "a", "content": "b", "multiline": true}"#,
        r#""dry_run": true"#,
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();
    assert!(!output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["errors"][0]["code"], "invalid_request");
    assert!(
        resp["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("multiline")
    );
}

#[test]
fn first_flag_replaces_one_occurrence_per_line() {
    let dir = setup_single_file("test.txt", "a a a\na a\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--first", "a", "B"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "B a a\nB a\n");
}

#[test]
fn first_in_file_flag_replaces_single_occurrence() {
    let dir = setup_single_file("test.txt", "a a\na\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--first-in-file", "a", "B"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "B a\na\n");
}

#[test]
fn max_replacements_caps_occurrences_per_file() {
    let dir = setup_single_file("test.txt", "a a\na a\n");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--max-replacements", "3", "a", "B"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "B B\nB a\n");
}

#[test]
fn max_replacements_zero_is_rejected_by_clap() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--max-replacements", "0", "a", "B"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("at least 1"));
}

#[test]
fn count_flags_conflict_with_each_other_and_delete() {
    for args in [
        vec!["--first", "--first-in-file", "a", "B"],
        vec!["--first", "--max-replacements", "2", "a", "B"],
        vec!["--first", "-U", "a", "B"],
        vec!["--first", "-d", "a"],
    ] {
        assert_cmd::cargo_bin_cmd!("ripsed")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("cannot be used with"));
    }
}

#[test]
fn count_json_op_first_per_line() {
    let dir = setup_files(&[("test.txt", "a a\na a\n")]);
    let request = json_request(
        r#"{"op": "replace", "find": "a", "replace": "B", "count": "first_per_line"}"#,
        &format!(r#""dry_run": false, "root": "{}""#, json_path(&dir)),
    );

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();
    assert!(output.status.success());

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(content, "B a\nB a\n");
}

#[test]
fn pattern_range_scopes_replacement_to_region() {
    let dir = setup_single_file(
        "conf.toml",
        "x = 1\n[dependencies]\nx = 2\n[dev-dependencies]\nx = 3\n",
    );

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args([
            "--range",
            r"/\[dependencies\]/,/\[dev-dependencies\]/",
            "x",
            "y",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("conf.toml")).unwrap();
    // The region opens at [dependencies] and closes at [dev-dependencies]
    // (both inclusive), so only x = 2 inside it is replaced; x = 1 before
    // and x = 3 after the region survive.
    assert_eq!(
        content,
        "x = 1\n[dependencies]\ny = 2\n[dev-dependencies]\nx = 3\n"
    );
}

#[test]
fn pattern_range_conflicts_with_line_range_and_multiline() {
    for args in [
        vec!["--range", "/a/,/b/", "-n", "1:2", "x", "y"],
        vec!["--range", "/a/,/b/", "-U", "x", "y"],
    ] {
        assert_cmd::cargo_bin_cmd!("ripsed")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("cannot be used with"));
    }
}

#[test]
fn pattern_range_malformed_syntax_rejected() {
    for bad in ["start,end", "/start/", "/a/,/b", "a/,/b/"] {
        assert_cmd::cargo_bin_cmd!("ripsed")
            .args(["--range", bad, "x", "y"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("/start/,/end/"));
    }
}

#[test]
fn pattern_range_invalid_regex_rejected_at_parse() {
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--range", "/(unclosed/,/end/", "x", "y"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid start pattern"));
}

#[test]
fn pattern_range_works_in_json_mode() {
    let dir = setup_files(&[("test.txt", "x\nBEGIN\nx\nEND\nx\n")]);
    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "x", "replace": "y"}}],
            "options": {{
                "dry_run": false,
                "root": "{}",
                "range": {{"start_pattern": "BEGIN", "end_pattern": "END"}}
            }}
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
    assert_eq!(content, "x\nBEGIN\ny\nEND\nx\n");
}

#[test]
fn json_rejects_both_line_range_and_pattern_range() {
    let request = r#"{
        "version": "1",
        "operations": [{"op": "replace", "find": "x", "replace": "y"}],
        "options": {
            "line_range": {"start": 1, "end": 2},
            "range": {"start_pattern": "a", "end_pattern": "b"}
        }
    }"#;

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--json"])
        .write_stdin(request)
        .output()
        .unwrap();
    assert!(!output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["errors"][0]["code"], "invalid_request");
}

// ── Encoding: BOM detection and UTF-16 transcoding ──

#[test]
fn utf16le_file_roundtrips_encoding_and_bom() {
    use ripsed_fs::encoding::{SourceEncoding, encode};

    let dir = setup_files(&[]);
    let path = dir.path().join("wide.txt");
    fs::write(
        &path,
        encode("hello world\nsecond line\n", SourceEncoding::Utf16Le),
    )
    .unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        bytes,
        encode("goodbye world\nsecond line\n", SourceEncoding::Utf16Le),
        "file must stay UTF-16LE with BOM, content replaced"
    );
}

#[test]
fn utf16be_file_roundtrips_encoding_and_bom() {
    use ripsed_fs::encoding::{SourceEncoding, encode};

    let dir = setup_files(&[]);
    let path = dir.path().join("wide-be.txt");
    fs::write(&path, encode("ünïcode hello 🎉\n", SourceEncoding::Utf16Be)).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        bytes,
        encode("ünïcode goodbye 🎉\n", SourceEncoding::Utf16Be)
    );
}

#[test]
fn utf8_bom_preserved_and_not_treated_as_content() {
    use ripsed_fs::encoding::UTF8_BOM;

    let dir = setup_files(&[]);
    let path = dir.path().join("bom.txt");
    let mut bytes = UTF8_BOM.to_vec();
    bytes.extend_from_slice(b"hello world\n");
    fs::write(&path, &bytes).unwrap();

    // Anchored regex must match at the real start of line 1 — i.e. the BOM
    // is stripped before matching, not left as invisible content.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["-e", "^hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    let out = fs::read(&path).unwrap();
    let mut expected = UTF8_BOM.to_vec();
    expected.extend_from_slice(b"goodbye world\n");
    assert_eq!(out, expected, "BOM re-attached, anchor matched after it");
}

#[test]
fn mixed_encoding_tree_all_files_replaced() {
    use ripsed_fs::encoding::{SourceEncoding, encode};

    let dir = setup_files(&[("plain.txt", "hello\n")]);
    fs::write(
        dir.path().join("wide.txt"),
        encode("hello\n", SourceEncoding::Utf16Le),
    )
    .unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(dir.path().join("plain.txt")).unwrap(),
        "goodbye\n"
    );
    assert_eq!(
        fs::read(dir.path().join("wide.txt")).unwrap(),
        encode("goodbye\n", SourceEncoding::Utf16Le)
    );
}

#[test]
fn undo_restores_utf16_file_byte_exact() {
    use ripsed_fs::encoding::{SourceEncoding, encode};

    let dir = setup_files(&[]);
    let path = dir.path().join("wide.txt");
    let original = encode("hello world\n", SourceEncoding::Utf16Le);
    fs::write(&path, &original).unwrap();

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success();
    assert_ne!(fs::read(&path).unwrap(), original, "edit landed");

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--undo"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert_eq!(
        fs::read(&path).unwrap(),
        original,
        "undo must restore the original bytes exactly, including BOM and encoding"
    );
}

#[test]
fn truncated_utf16_file_fails_cleanly() {
    let dir = setup_files(&[("good.txt", "hello\n")]);
    // UTF-16LE BOM followed by an odd number of payload bytes.
    fs::write(dir.path().join("bad.txt"), [0xFF, 0xFE, 0x68, 0x00, 0x65]).unwrap();

    // The malformed file is reported and skipped; the good file still
    // gets modified.
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["hello", "goodbye"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("truncated"));

    assert_eq!(
        fs::read_to_string(dir.path().join("good.txt")).unwrap(),
        "goodbye\n"
    );
}

// ── Streaming pipe mode ──

#[test]
fn pipe_mode_streams_large_input() {
    // 4 MB through the streaming path: correctness check that the
    // line-by-line loop handles volume (the old path buffered everything;
    // the cap-free streaming path must produce identical output).
    let line = "needle in a haystack line\n";
    let count = 4 * 1024 * 1024 / line.len();
    let input: String = line.repeat(count);

    let output = assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--pipe", "needle", "thread"])
        .write_stdin(input)
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.lines().count(), count);
    assert!(stdout.starts_with("thread in a haystack line\n"));
    assert!(!stdout.contains("needle"));
}

#[test]
fn pipe_mode_closed_downstream_exits_cleanly() {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    // ripsed writes a large stream to a stdout we close after one line —
    // the EPIPE must terminate it quietly with success, like sed | head.
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("ripsed"))
        .args(["--pipe", "x", "y"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        // Enough data to overflow OS pipe buffers so the child actually
        // hits EPIPE; ignore the write error when the child exits early.
        let chunk = "x\n".repeat(64 * 1024);
        for _ in 0..64 {
            if stdin.write_all(chunk.as_bytes()).is_err() {
                break;
            }
        }
    });

    // Read one line's worth, then drop stdout to close the pipe.
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 2];
    stdout.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"y\n");
    drop(stdout);

    let status = child.wait().unwrap();
    writer.join().unwrap();
    assert!(
        status.success(),
        "closed downstream must be quiet success, got {status:?}"
    );
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
