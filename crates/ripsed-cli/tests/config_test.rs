mod common;

use common::*;
use std::fs;

// ---------------------------------------------------------------------------
// Task 3: config file handling tests
// ---------------------------------------------------------------------------

#[test]
fn config_backup_true_creates_bak_file() {
    let dir = setup_files(&[
        ("test.txt", "original content\n"),
        (".ripsed.toml", "[defaults]\nbackup = true\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(
        content.contains("modified"),
        "File should contain the replacement"
    );

    // Backup file should exist with original content
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        backup_path.exists(),
        "Backup file should exist when config has backup = true"
    );
    let backup_content = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(
        backup_content, "original content\n",
        "Backup should contain original content"
    );
}

#[test]
fn config_flag_loads_specific_config_file() {
    let dir = setup_files(&[
        ("test.txt", "original content\n"),
        ("custom-config.toml", "[defaults]\nbackup = true\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args([
            "--config",
            dir.path().join("custom-config.toml").to_str().unwrap(),
            "original",
            "modified",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(
        content.contains("modified"),
        "File should contain the replacement"
    );

    // Backup file should exist (config had backup = true)
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        backup_path.exists(),
        "Backup file should exist when --config points to file with backup = true"
    );
}

#[test]
fn config_discovery_walks_up_directories() {
    let dir = setup_files(&[
        (".ripsed.toml", "[defaults]\nbackup = true\n"),
        ("child/deep/test.txt", "original content\n"),
    ]);

    // Run ripsed from a deeply nested child directory; it should discover
    // the .ripsed.toml in the parent (root of temp dir).
    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["original", "modified"])
        .current_dir(dir.path().join("child/deep"))
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("child/deep/test.txt")).unwrap();
    assert!(
        content.contains("modified"),
        "File should contain the replacement"
    );

    // Backup should be created because the discovered config has backup = true
    let backup_path = dir.path().join("child/deep/test.txt.ripsed.bak");
    assert!(
        backup_path.exists(),
        "Backup file should exist when config is discovered from parent directory"
    );
}

#[test]
fn cli_flag_overrides_config_backup() {
    // Config says backup = false (the default), but we pass --backup on CLI
    let dir = setup_files(&[
        ("test.txt", "original content\n"),
        (".ripsed.toml", "[defaults]\nbackup = false\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["--backup", "original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Backup should still be created because --backup flag overrides config
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        backup_path.exists(),
        "CLI --backup flag should override config backup = false"
    );
}

#[test]
fn config_without_backup_does_not_create_bak_file() {
    let dir = setup_files(&[
        ("test.txt", "original content\n"),
        (".ripsed.toml", "[defaults]\nbackup = false\n"),
    ]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args(["original", "modified"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should be modified
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(content.contains("modified"));

    // No backup should exist
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        !backup_path.exists(),
        "No backup file should be created when config has backup = false"
    );
}

#[test]
fn missing_config_file_via_flag_exits_with_error() {
    let dir = setup_files(&[("test.txt", "content\n")]);

    assert_cmd::cargo_bin_cmd!("ripsed")
        .args([
            "--config",
            "/nonexistent/path/config.toml",
            "content",
            "replaced",
        ])
        .current_dir(dir.path())
        .assert()
        .failure();
}

// ---------------------------------------------------------------------------
// JSON mode config tests
// ---------------------------------------------------------------------------

#[test]
fn json_mode_backup_option_creates_bak_file() {
    let dir = setup_files(&[("test.txt", "original content\n")]);

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

    // File should be modified
    let content = fs::read_to_string(dir.path().join("test.txt")).unwrap();
    assert!(
        content.contains("modified"),
        "File should contain the replacement"
    );

    // Backup file should exist with original content
    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        backup_path.exists(),
        "Backup .bak file should exist when backup: true in JSON options"
    );
    let backup_content = fs::read_to_string(&backup_path).unwrap();
    assert_eq!(backup_content, "original content\n");
}

#[test]
fn json_mode_without_backup_option_does_not_create_bak() {
    let dir = setup_files(&[("test.txt", "original content\n")]);

    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "original", "replace": "modified"}}],
            "options": {{"dry_run": false, "backup": false, "root": "{}"}}
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
    assert!(content.contains("modified"));

    let backup_path = dir.path().join("test.txt.ripsed.bak");
    assert!(
        !backup_path.exists(),
        "No backup should be created when backup: false"
    );
}

#[test]
fn json_mode_dry_run_true_does_not_modify_file() {
    let dir = setup_files(&[("test.txt", "original content\n")]);

    // dry_run defaults to true in JSON mode
    let request = format!(
        r#"{{
            "version": "1",
            "operations": [{{"op": "replace", "find": "original", "replace": "modified"}}],
            "options": {{"root": "{}"}}
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
        "File should be unchanged when dry_run defaults to true"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(resp["dry_run"], true);
}
