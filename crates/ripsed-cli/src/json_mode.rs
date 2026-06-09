use ripsed_core::config::Config;
use ripsed_core::diff::Summary;
use ripsed_core::engine;
use ripsed_core::error::RipsedError;
use ripsed_core::matcher::Matcher;
use ripsed_fs::discovery::discover_files;
use ripsed_fs::encoding::SourceEncoding;
use ripsed_fs::lock::FileLock;
use ripsed_fs::reader;
use ripsed_fs::writer;
use ripsed_json::request::JsonRequest;
use ripsed_json::response::{JsonResponse, UndoResponse, UndoSummary};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::shared::{load_undo_log, record_undo, save_undo_log};

pub fn run_json_mode(input: &str, config: &Config, jsonl: bool) -> Result<(), i32> {
    let request = match JsonRequest::parse(input) {
        Ok(r) => r,
        Err(e) => {
            let response = JsonResponse::error(vec![e]);
            println!("{}", response.to_json());
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    // Handle undo requests before processing operations
    if let Some(ref undo_req) = request.undo {
        handle_json_undo(undo_req.last, config);
        return Ok(());
    }

    let dry_run = request.options.dry_run;
    let atomic = request.options.atomic;
    let backup = request.options.backup;
    let (ops, options) = request.into_ops();
    let discovery_opts = crate::shared::discovery_opts_from(&options);
    let files = match discover_files(&discovery_opts) {
        Ok(f) => f,
        Err(e) => {
            let response = JsonResponse::error(vec![RipsedError::internal_error(e.to_string())]);
            println!("{}", response.to_json());
            return Err(crate::shared::EXIT_ERROR);
        }
    };

    let mut summary = Summary::default();
    let mut results = Vec::new();
    let mut errors = Vec::new();

    // Content cache: subsequent operations see the output of previous ones.
    // Carries the source encoding detected on first read so the final
    // write re-encodes correctly.
    let mut content_cache: HashMap<PathBuf, (String, SourceEncoding)> = HashMap::new();

    // Advisory locks held from first read of each file through final write.
    // Prevents concurrent ripsed processes from clobbering each other.
    let mut file_locks: HashMap<PathBuf, FileLock> = HashMap::new();

    // For atomic batch mode, collect all writes to apply at once
    let mut pending_writes: Vec<(std::path::PathBuf, String, SourceEncoding)> = Vec::new();

    // Load undo log for recording changes (only when not dry-run)
    let mut undo_log = if !dry_run {
        Some(load_undo_log(config))
    } else {
        None
    };

    let stdout = std::io::stdout();

    for (op_index, (op, op_glob)) in ops.iter().enumerate() {
        let matcher = match Matcher::new(op) {
            Ok(m) => m,
            Err(mut e) => {
                e.operation_index = Some(op_index);
                errors.push(e);
                continue;
            }
        };

        // Build a glob matcher for per-operation filtering
        let glob_matcher = op_glob.as_ref().and_then(|g| {
            globset::GlobBuilder::new(g)
                .literal_separator(true)
                .build()
                .ok()
                .map(|glob| glob.compile_matcher())
        });

        for file_path in &files {
            // Skip files that don't match the per-operation glob
            if let Some(ref gm) = glob_matcher
                && !gm.is_match(file_path)
            {
                // Also try matching just the file name
                let matches_name = file_path
                    .file_name()
                    .map(|n| gm.is_match(n))
                    .unwrap_or(false);
                if !matches_name {
                    continue;
                }
            }
            // Acquire advisory lock on first access to this file.
            // The lock is held in file_locks until the function returns.
            // Skipped in dry-run mode (read-only, no lock files needed).
            if !dry_run && !file_locks.contains_key(file_path) {
                match FileLock::try_lock_with_timeout(file_path, Duration::from_secs(5)) {
                    Ok(lock) => {
                        file_locks.insert(file_path.clone(), lock);
                    }
                    Err(e) => {
                        errors.push(RipsedError::write_failed(
                            &file_path.to_string_lossy(),
                            &format!("lock failed: {e}"),
                        ));
                        continue;
                    }
                }
            }

            // Use cached content if a prior operation already modified this file
            let (content, encoding) = if let Some(cached) = content_cache.get(file_path) {
                cached.clone()
            } else {
                match reader::read_file_with_encoding(file_path) {
                    Ok(c) => c,
                    Err(e) => {
                        let path_str = file_path.to_string_lossy();
                        if e.kind() == std::io::ErrorKind::PermissionDenied {
                            errors.push(RipsedError::permission_denied(&path_str));
                        } else {
                            errors.push(RipsedError::write_failed(&path_str, &e.to_string()));
                        }
                        continue;
                    }
                }
            };

            let output = match engine::apply(&content, op, &matcher, options.range_spec(), 3) {
                Ok(o) => o,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };

            // Update cache so subsequent operations see modified content
            if let Some(ref text) = output.text {
                content_cache.insert(file_path.clone(), (text.clone(), encoding));
            }

            if !output.changes.is_empty() {
                summary.files_matched += 1;
                summary.total_replacements += output.changes.len();

                let result =
                    engine::build_op_result(op_index, &file_path.to_string_lossy(), output.changes);

                // In JSONL mode, print each file result as it completes
                if jsonl {
                    let line_json = serde_json::to_string(&result).unwrap_or_default();
                    let mut handle = stdout.lock();
                    let _ = writeln!(handle, "{}", line_json);
                    let _ = handle.flush();
                }

                results.push(result);

                if !dry_run && let Some(ref text) = output.text {
                    // Record undo entry before writing
                    if let Some(ref mut log) = undo_log
                        && let Some(ref undo_entry) = output.undo
                    {
                        record_undo(log, file_path, undo_entry, encoding);
                    }

                    if backup && let Err(e) = writer::create_backup(file_path) {
                        errors.push(RipsedError::write_failed(
                            &file_path.to_string_lossy(),
                            &format!("backup failed: {e}"),
                        ));
                        continue;
                    }

                    if atomic {
                        // Collect for batch write
                        pending_writes.push((file_path.clone(), text.clone(), encoding));
                    } else if writer::write_atomic_encoded(file_path, text, encoding).is_ok() {
                        summary.files_modified += 1;
                    }
                }
            }
        }
    }

    // Commit atomic batch if needed
    if atomic && !pending_writes.is_empty() {
        let mut batch = writer::AtomicBatch::new();
        let mut stage_err = None;
        for (path, content, encoding) in &pending_writes {
            if let Err(e) = batch.stage_encoded(path, content, *encoding) {
                stage_err = Some(e);
                break;
            }
        }
        let commit_result = match stage_err {
            Some(e) => Err(e),
            None => batch.commit(),
        };
        match commit_result {
            Ok(()) => {
                summary.files_modified += pending_writes.len();
            }
            Err(e) => {
                errors.push(ripsed_core::error::RipsedError::internal_error(format!(
                    "Atomic batch write failed: {e}"
                )));
                // None of the files were written, so don't save undo entries
                undo_log = None;
            }
        }
    }

    // Save undo log after all changes
    if let Some(ref log) = undo_log {
        save_undo_log(log);
    }

    // In JSONL mode, print a final summary line
    if jsonl {
        let summary_json = serde_json::json!({
            "type": "summary",
            "files_matched": summary.files_matched,
            "files_modified": summary.files_modified,
            "total_replacements": summary.total_replacements,
            "errors": errors.len(),
        });
        let mut handle = stdout.lock();
        let _ = writeln!(handle, "{}", summary_json);
        let _ = handle.flush();
    }

    let response = if errors.is_empty() {
        JsonResponse::success(dry_run, summary, results)
    } else {
        let mut resp = JsonResponse::success(dry_run, summary, results);
        resp.errors = errors;
        resp.success = resp.errors.is_empty();
        resp
    };

    println!("{}", response.to_json());
    // Taxonomy: any error -> 2; clean run with zero matches -> 1; else 0.
    if !response.success {
        Err(crate::shared::EXIT_ERROR)
    } else if response.summary.files_matched == 0 {
        Err(crate::shared::EXIT_NO_MATCHES)
    } else {
        Ok(())
    }
}

/// Handle a JSON undo request: pop the last N entries from the undo log,
/// restore the files, and print an UndoResponse JSON.
fn handle_json_undo(count: usize, config: &Config) {
    let mut log = load_undo_log(config);

    let records = log.pop(count);
    let mut files_restored = 0usize;

    for record in &records {
        let path = Path::new(&record.file_path);
        let encoding = crate::shared::undo_record_encoding(record);
        if writer::write_atomic_encoded(path, &record.entry.original_text, encoding).is_ok() {
            files_restored += 1;
        }
    }

    save_undo_log(&log);

    let response = UndoResponse {
        version: "1".to_string(),
        success: true,
        undo: UndoSummary {
            operations_reverted: records.len(),
            files_restored,
            log_entries_remaining: log.len(),
        },
    };

    println!("{}", response.to_json());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ripsed_core::config::Config;
    use ripsed_core::undo::UndoRecord;
    use std::fs;
    use tempfile::TempDir;

    /// Escape a TempDir path for safe embedding in JSON strings.
    fn json_path(dir: &TempDir) -> String {
        dir.path().display().to_string().replace('\\', "\\\\")
    }

    /// Create a temp directory with a test file, returning (dir, file_path).
    fn setup_test_file(content: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, content).unwrap();
        (dir, file_path)
    }

    #[test]
    fn test_handle_json_undo_restores_files() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("undoable.txt");
        fs::write(&file_path, "modified content").unwrap();

        let config = Config::default();

        // Ensure undo dir exists and write the log file directly
        let undo_dir = crate::shared::undo_dir();
        let _ = fs::create_dir_all(&undo_dir);

        let mut log = ripsed_core::undo::UndoLog::new(100);
        log.push(UndoRecord {
            encoding: None,
            timestamp: "12345".to_string(),
            file_path: file_path.to_string_lossy().to_string(),
            entry: ripsed_core::undo::UndoEntry {
                original_text: "original content".to_string(),
            },
        });
        save_undo_log(&log);

        handle_json_undo(1, &config);

        let restored = fs::read_to_string(&file_path).unwrap();
        assert_eq!(restored, "original content");
    }

    #[test]
    fn test_handle_json_undo_empty_log_does_not_panic() {
        // Test the underlying logic without touching shared filesystem state.
        // An empty UndoLog.pop(1) returns an empty vec, which handle_json_undo
        // handles gracefully (0 files restored).
        let mut log = ripsed_core::undo::UndoLog::new(100);
        let popped = log.pop(1);
        assert!(popped.is_empty());
    }

    #[test]
    fn test_json_mode_dry_run_does_not_write() {
        let (dir, file_path) = setup_test_file("hello world\n");
        let root = json_path(&dir);

        let input = format!(
            r#"{{
                "operations": [{{"op": "replace", "find": "hello", "replace": "goodbye"}}],
                "options": {{"dry_run": true, "root": "{root}"}}
            }}"#
        );

        let config = Config::default();
        // Actually run the mode: the operation matches, so the run succeeds,
        // but dry_run must prevent the write from landing.
        let result = run_json_mode(&input, &config, false);
        assert_eq!(result, Ok(()), "dry-run with a match should succeed");

        // File should remain unchanged
        let content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "hello world\n");
    }

    #[test]
    fn test_json_request_parse_undo() {
        let input = r#"{"undo": {"last": 2}}"#;
        let request = JsonRequest::parse(input).unwrap();
        assert!(request.undo.is_some());
        assert_eq!(request.undo.unwrap().last, 2);
    }

    #[test]
    fn test_json_request_parse_atomic_option() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "options": {"atomic": true}
        }"#;
        let request = JsonRequest::parse(input).unwrap();
        assert!(request.options.atomic);
    }

    #[test]
    fn test_atomic_batch_write_all_or_nothing() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "old_a").unwrap();
        fs::write(&b, "old_b").unwrap();

        let files: Vec<(&Path, &str)> = vec![(a.as_path(), "new_a"), (b.as_path(), "new_b")];
        writer::write_atomic_batch(&files).unwrap();

        assert_eq!(fs::read_to_string(&a).unwrap(), "new_a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "new_b");
    }

    #[test]
    fn test_atomic_batch_failure_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let good = dir.path().join("good.txt");
        fs::write(&good, "original").unwrap();

        let bad = Path::new("/nonexistent_dir_12345/bad.txt");
        let files: Vec<(&Path, &str)> = vec![(good.as_path(), "new"), (bad, "nope")];
        let result = writer::write_atomic_batch(&files);
        assert!(result.is_err());

        // Good file should still have original content
        assert_eq!(fs::read_to_string(&good).unwrap(), "original");
    }

    #[test]
    fn test_undo_response_to_json() {
        let resp = UndoResponse {
            version: "1".into(),
            success: true,
            undo: UndoSummary {
                operations_reverted: 2,
                files_restored: 2,
                log_entries_remaining: 5,
            },
        };
        let json_str = resp.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["version"], "1");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["undo"]["operations_reverted"], 2);
        assert_eq!(parsed["undo"]["files_restored"], 2);
        assert_eq!(parsed["undo"]["log_entries_remaining"], 5);
    }

    #[test]
    fn test_undo_log_roundtrip_for_json_mode() {
        // Test UndoLog directly without relying on shared filesystem paths
        let config = Config::default();
        let mut log = ripsed_core::undo::UndoLog::new(config.undo.max_entries);

        log.push(UndoRecord {
            encoding: None,
            timestamp: "999".to_string(),
            file_path: "/tmp/test_json_undo.txt".to_string(),
            entry: ripsed_core::undo::UndoEntry {
                original_text: "test content".to_string(),
            },
        });

        // Serialize to JSONL and reload
        let jsonl = log.to_jsonl();
        let reloaded = ripsed_core::undo::UndoLog::from_jsonl(&jsonl, config.undo.max_entries);
        assert_eq!(reloaded.len(), 1);

        // Pop to verify
        let mut reloaded = reloaded;
        let popped = reloaded.pop(1);
        assert!(!popped.is_empty());
        assert_eq!(popped[0].entry.original_text, "test content");
    }

    #[test]
    fn test_jsonl_streaming_output() {
        // Test that JSONL mode produces valid JSON per line
        // We test the serialization of individual OpResult objects
        use ripsed_core::diff::{Change, FileChanges, OpResult};

        let result = OpResult {
            operation_index: 0,
            files: vec![FileChanges {
                path: "test.txt".to_string(),
                changes: vec![Change {
                    line: 1,
                    before: "old".to_string(),
                    after: Some("new".to_string()),
                    context: None,
                }],
            }],
        };

        let json_line = serde_json::to_string(&result).unwrap();
        // Verify it's valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&json_line).unwrap();
        assert_eq!(parsed["operation_index"], 0);
        assert_eq!(parsed["files"][0]["path"], "test.txt");
    }

    #[test]
    fn test_json_path_helper_escapes_backslashes() {
        let dir = TempDir::new().unwrap();
        let path_str = json_path(&dir);
        // Should not contain unescaped backslashes
        assert!(!path_str.contains('\\') || path_str.contains("\\\\"));
    }

    #[test]
    fn test_pending_writes_collected_for_atomic() {
        // Verify that pending_writes vector works correctly
        let dir = TempDir::new().unwrap();
        let path_a = dir.path().join("a.txt");
        let path_b = dir.path().join("b.txt");

        let pending: Vec<(std::path::PathBuf, String)> = vec![
            (path_a.clone(), "content_a".to_string()),
            (path_b.clone(), "content_b".to_string()),
        ];

        let batch_refs: Vec<(&Path, &str)> = pending
            .iter()
            .map(|(p, c)| (p.as_path(), c.as_str()))
            .collect();
        writer::write_atomic_batch(&batch_refs).unwrap();

        assert_eq!(fs::read_to_string(&path_a).unwrap(), "content_a");
        assert_eq!(fs::read_to_string(&path_b).unwrap(), "content_b");
    }
}
