mod common;

use common::*;
use std::fs;

// ---------------------------------------------------------------------------
// Task 5: atomic write behavior tests
// ---------------------------------------------------------------------------

#[test]
fn backup_flag_creates_bak_file() {
    let dir = setup_files(&[("data.txt", "original content\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("data.txt")).unwrap();
    assert!(
        content.contains("modified content"),
        "File should be modified after replacement"
    );

    // Backup file should exist with original content
    let backup_path = dir.path().join("data.txt.ripsed.bak");
    assert!(backup_path.exists(), "Backup .bak file should exist");
    let backup_content = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(
        backup_content, "original content\n",
        "Backup should contain original file content"
    );
}

#[test]
fn backup_with_numbered_suffixes_when_backup_exists() {
    let dir = setup_files(&[("data.txt", "version one\n")]);

    // First replacement with backup
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "version one", "version two"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Verify first backup
    let bak_path = dir.path().join("data.txt.ripsed.bak");
    assert!(bak_path.exists(), "First backup should exist");
    assert_eq!(
        fs::read_to_string(&bak_path).unwrap(),
        "version one\n",
        "First backup should contain version one"
    );

    // Second replacement with backup
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "version two", "version three"])
        .current_dir(dir.path())
        .assert()
        .success();

    // First backup should still exist unchanged
    assert_eq!(
        fs::read_to_string(&bak_path).unwrap(),
        "version one\n",
        "First backup should still contain version one"
    );

    // Second backup should have a numbered suffix
    let bak1_path = dir.path().join("data.txt.ripsed.bak.1");
    assert!(
        bak1_path.exists(),
        "Second backup should exist with .bak.1 suffix"
    );
    assert_eq!(
        fs::read_to_string(&bak1_path).unwrap(),
        "version two\n",
        "Second backup should contain version two"
    );

    // Current file should have the latest content
    let content = fs::read_to_string(dir.path().join("data.txt")).unwrap();
    assert_eq!(
        content, "version three\n",
        "File should contain version three"
    );
}

#[test]
fn backup_with_three_successive_backups() {
    let dir = setup_files(&[("notes.txt", "rev_a\n")]);

    // Replacement 1
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "rev_a", "rev_b"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Replacement 2
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "rev_b", "rev_c"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Replacement 3
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "rev_c", "rev_d"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Verify all backups exist with correct content
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.txt.ripsed.bak")).unwrap(),
        "rev_a\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.txt.ripsed.bak.1")).unwrap(),
        "rev_b\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.txt.ripsed.bak.2")).unwrap(),
        "rev_c\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "rev_d\n"
    );
}

#[test]
fn backup_for_file_without_extension() {
    let dir = setup_files(&[("Makefile", "old_target: build\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "old_target", "new_target"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("Makefile")).unwrap();
    assert!(content.contains("new_target"));

    // Backup should use .ripsed.bak suffix even without an extension
    let backup_path = dir.path().join("Makefile.ripsed.bak");
    assert!(
        backup_path.exists(),
        "Backup should exist for files without extensions"
    );
    assert_eq!(
        fs::read_to_string(&backup_path).unwrap(),
        "old_target: build\n"
    );
}

#[test]
fn dry_run_does_not_create_backup() {
    let dir = setup_files(&[("test.txt", "original content\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "--dry-run", "original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should NOT be modified (dry run)
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(
        content, "original content\n",
        "File should be unchanged in dry-run mode"
    );

    // No backup should be created
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        !backup_path.exists(),
        "No backup should be created in dry-run mode"
    );
}

#[test]
fn backup_across_multiple_files() {
    let dir = setup_files(&[
        ("first.txt", "common_word here\n"),
        ("second.txt", "common_word there\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "common_word", "replaced_word"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Both files should be modified
    assert!(
        fs::read_to_string(dir.path().join("first.txt"))
            .unwrap()
            .contains("replaced_word")
    );
    assert!(
        fs::read_to_string(dir.path().join("second.txt"))
            .unwrap()
            .contains("replaced_word")
    );

    // Both backups should exist
    let bak1 = dir.path().join("first.txt.ripsed.bak");
    let bak2 = dir.path().join("second.txt.ripsed.bak");
    assert!(bak1.exists(), "Backup for first.txt should exist");
    assert!(bak2.exists(), "Backup for second.txt should exist");

    assert_eq!(fs::read_to_string(&bak1).unwrap(), "common_word here\n");
    assert_eq!(fs::read_to_string(&bak2).unwrap(), "common_word there\n");
}

// ---------------------------------------------------------------------------
// JSON mode atomic write / backup tests
// ---------------------------------------------------------------------------

#[test]
fn json_backup_creates_bak_file() {
    let dir = setup_files(&[("data.txt", "original content\n")]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "original", "replace": "modified"}}],
            "options": {{"dry_run": false, "backup": true, "root": "{}"}}
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

    let content = fs::read_to_string(dir.path().join("data.txt")).unwrap();
    assert!(
        content.contains("modified content"),
        "File should be modified"
    );

    let backup_path = dir.path().join("data.txt.ripsed.bak");
    assert!(backup_path.exists(), "Backup .bak file should exist");
    assert_eq!(
        fs::read_to_string(&backup_path).unwrap(),
        "original content\n"
    );
}

#[test]
fn json_dry_run_does_not_create_backup() {
    let dir = setup_files(&[("test.txt", "original content\n")]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "original", "replace": "modified"}}],
            "options": {{"dry_run": true, "backup": true, "root": "{}"}}
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

    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert_eq!(
        content, "original content\n",
        "File should be unchanged in dry-run"
    );

    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        !backup_path.exists(),
        "No backup should be created in dry-run mode"
    );
}

#[test]
fn json_atomic_batch_writes_all_files() {
    let dir = setup_files(&[
        ("first.txt", "common_word here\n"),
        ("second.txt", "common_word there\n"),
    ]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "common_word", "replace": "replaced_word"}}],
            "options": {{"dry_run": false, "atomic": true, "root": "{}"}}
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

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["success"], true);

    // Both files should be modified
    assert!(
        fs::read_to_string(dir.path().join("first.txt"))
            .unwrap()
            .contains("replaced_word")
    );
    assert!(
        fs::read_to_string(dir.path().join("second.txt"))
            .unwrap()
            .contains("replaced_word")
    );
}

#[test]
fn json_backup_across_multiple_files() {
    let dir = setup_files(&[
        ("first.txt", "common_word here\n"),
        ("second.txt", "common_word there\n"),
    ]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "common_word", "replace": "replaced_word"}}],
            "options": {{"dry_run": false, "backup": true, "root": "{}"}}
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

    let bak1 = dir.path().join("first.txt.ripsed.bak");
    let bak2 = dir.path().join("second.txt.ripsed.bak");
    assert!(bak1.exists(), "Backup for first.txt should exist");
    assert!(bak2.exists(), "Backup for second.txt should exist");

    assert_eq!(fs::read_to_string(&bak1).unwrap(), "common_word here\n");
    assert_eq!(fs::read_to_string(&bak2).unwrap(), "common_word there\n");
}
