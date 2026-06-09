use serde::{Deserialize, Serialize};

/// An entry in the undo log, storing enough information to reverse an operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    /// The full original text before the operation.
    pub original_text: String,
}

/// A record in the persistent undo log file (.ripsed/undo.jsonl).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoRecord {
    /// Unix epoch seconds as a decimal string (e.g., `"1711550400"`).
    pub timestamp: String,
    pub file_path: String,
    pub entry: UndoEntry,
    /// Source-encoding tag of the file when it was read (e.g. `"utf-16le"`).
    /// `None` (and absent in pre-existing logs) means plain UTF-8. Restoring
    /// re-encodes `original_text` with this so undo is byte-exact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// Manages the undo log.
pub struct UndoLog {
    records: Vec<UndoRecord>,
    max_entries: usize,
}

impl UndoLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            records: Vec::new(),
            max_entries,
        }
    }

    /// Load undo log from JSONL content.
    pub fn from_jsonl(content: &str, max_entries: usize) -> Self {
        let records: Vec<UndoRecord> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        Self {
            records,
            max_entries,
        }
    }

    /// Serialize the log to JSONL format.
    pub fn to_jsonl(&self) -> String {
        self.records
            .iter()
            .filter_map(|r| serde_json::to_string(r).ok())
            .collect::<Vec<_>>()
            .join("\n")
            + if self.records.is_empty() { "" } else { "\n" }
    }

    /// Append a new undo record.
    pub fn push(&mut self, record: UndoRecord) {
        self.records.push(record);
        self.prune();
    }

    /// Remove the last N records and return them (for undo).
    pub fn pop(&mut self, count: usize) -> Vec<UndoRecord> {
        let drain_start = self.records.len().saturating_sub(count);
        self.records.drain(drain_start..).rev().collect()
    }

    /// Number of entries in the log.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Get recent entries for display.
    pub fn recent(&self, count: usize) -> &[UndoRecord] {
        let start = self.records.len().saturating_sub(count);
        &self.records[start..]
    }

    fn prune(&mut self) {
        if self.records.len() > self.max_entries {
            let excess = self.records.len() - self.max_entries;
            self.records.drain(..excess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_pop() {
        let mut log = UndoLog::new(100);
        log.push(UndoRecord {
            encoding: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            file_path: "test.txt".to_string(),
            entry: UndoEntry {
                original_text: "hello".to_string(),
            },
        });
        assert_eq!(log.len(), 1);
        let popped = log.pop(1);
        assert_eq!(popped.len(), 1);
        assert_eq!(popped[0].file_path, "test.txt");
        assert!(log.is_empty());
    }

    #[test]
    fn test_pruning() {
        let mut log = UndoLog::new(2);
        for i in 0..5 {
            log.push(UndoRecord {
                encoding: None,
                timestamp: format!("2026-01-0{i}T00:00:00Z"),
                file_path: format!("file{i}.txt"),
                entry: UndoEntry {
                    original_text: format!("content{i}"),
                },
            });
        }
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn test_jsonl_roundtrip() {
        let mut log = UndoLog::new(100);
        log.push(UndoRecord {
            encoding: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            file_path: "test.txt".to_string(),
            entry: UndoEntry {
                original_text: "original".to_string(),
            },
        });
        let jsonl = log.to_jsonl();
        let loaded = UndoLog::from_jsonl(&jsonl, 100);
        assert_eq!(loaded.len(), 1);
    }

    // ---- Adversarial edge-case tests ----

    fn record(path: &str, text: &str) -> UndoRecord {
        UndoRecord {
            encoding: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            file_path: path.to_string(),
            entry: UndoEntry {
                original_text: text.to_string(),
            },
        }
    }

    /// **Adversarial**: `pop(0)` must be a no-op — returning an empty Vec
    /// and leaving the log unchanged. Agents call this to roll back "the
    /// last 0 operations" when they compute a dynamic count, and a buggy
    /// implementation could e.g. drain the whole log.
    #[test]
    fn pop_zero_is_noop() {
        let mut log = UndoLog::new(10);
        log.push(record("a.txt", "A"));
        log.push(record("b.txt", "B"));
        let popped = log.pop(0);
        assert!(popped.is_empty(), "pop(0) should return empty");
        assert_eq!(log.len(), 2, "pop(0) should not drain the log");
    }

    /// **Adversarial**: `pop(n)` with n > log length must drain all
    /// records without panicking (saturating subtraction).
    #[test]
    fn pop_exceeds_length_drains_all() {
        let mut log = UndoLog::new(10);
        log.push(record("a.txt", "A"));
        log.push(record("b.txt", "B"));
        let popped = log.pop(100);
        assert_eq!(popped.len(), 2);
        assert!(log.is_empty());
    }

    /// **Adversarial**: `pop` must return records in REVERSE insertion
    /// order (newest first). Undo replays in this order to correctly
    /// reverse a sequence of operations — reversed ordering is critical
    /// when the same file was edited multiple times.
    #[test]
    fn pop_returns_newest_first() {
        let mut log = UndoLog::new(10);
        log.push(record("a.txt", "first"));
        log.push(record("a.txt", "second"));
        log.push(record("a.txt", "third"));

        let popped = log.pop(3);
        assert_eq!(popped[0].entry.original_text, "third");
        assert_eq!(popped[1].entry.original_text, "second");
        assert_eq!(popped[2].entry.original_text, "first");
    }

    /// **Adversarial**: When the log exceeds `max_entries`, pruning must
    /// remove the OLDEST entries, preserving the most recent N. A buggy
    /// implementation could drop the newest (which are the most likely to
    /// be undone) or a random subset.
    #[test]
    fn prune_drops_oldest_not_newest() {
        let mut log = UndoLog::new(3);
        for i in 0..10 {
            log.push(record(&format!("file_{i}.txt"), &format!("v{i}")));
        }
        assert_eq!(log.len(), 3);
        // The three remaining should be files 7, 8, 9.
        let latest_three = log.recent(3);
        assert_eq!(latest_three[0].file_path, "file_7.txt");
        assert_eq!(latest_three[1].file_path, "file_8.txt");
        assert_eq!(latest_three[2].file_path, "file_9.txt");
    }

    /// **Adversarial**: `from_jsonl` must silently skip malformed lines
    /// (corrupted log file recovery) but still load the good ones.
    /// Blowing up on a single bad line would brick all undo.
    #[test]
    fn from_jsonl_skips_malformed_lines() {
        let good = serde_json::to_string(&record("a.txt", "A")).unwrap();
        let bad = "{{{ not valid json";
        let also_good = serde_json::to_string(&record("b.txt", "B")).unwrap();
        let mixed = format!("{good}\n{bad}\n{also_good}\n");

        let log = UndoLog::from_jsonl(&mixed, 100);
        assert_eq!(
            log.len(),
            2,
            "malformed lines should be skipped, good ones kept"
        );
    }

    /// **Adversarial**: `from_jsonl` must tolerate blank lines, CRLF line
    /// endings, and leading/trailing whitespace without crashing or
    /// miscounting records.
    #[test]
    fn from_jsonl_tolerates_blank_and_crlf_lines() {
        let good = serde_json::to_string(&record("a.txt", "A")).unwrap();
        let mixed = format!("\n{good}\r\n\r\n");
        let log = UndoLog::from_jsonl(&mixed, 100);
        assert_eq!(log.len(), 1);
    }

    /// **Adversarial**: serializing an empty log produces an empty string
    /// (not a lone newline). Round-trip through `from_jsonl` preserves
    /// emptiness. This guards against a subtle bug where writing a lone
    /// `\n` would create a truncated-looking log file.
    #[test]
    fn empty_log_jsonl_is_empty_string() {
        let log = UndoLog::new(100);
        assert_eq!(log.to_jsonl(), "");
        let reparsed = UndoLog::from_jsonl("", 100);
        assert!(reparsed.is_empty());
    }

    /// **Adversarial**: after pushing and popping, the log should behave
    /// as if the popped records were never pushed for the purpose of
    /// subsequent `recent()` / `len()` calls. A bug could mark records
    /// as popped without actually removing them.
    #[test]
    fn pop_actually_removes_from_log() {
        let mut log = UndoLog::new(10);
        log.push(record("a.txt", "A"));
        log.push(record("b.txt", "B"));
        let _ = log.pop(1);
        assert_eq!(log.len(), 1);
        let remaining = log.recent(1);
        assert_eq!(remaining[0].file_path, "a.txt");
    }
}
