use ripsed_core::error::RipsedError;
use ripsed_core::operation::{Op, OpOptions};
use serde::{Deserialize, Serialize};

/// A structured JSON request from an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRequest {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub operations: Vec<JsonOp>,
    #[serde(default)]
    pub options: OpOptions,
    /// Undo request (mutually exclusive with operations).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo: Option<UndoRequest>,
    /// Forward-compatible: capture unknown top-level fields.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A single operation in a JSON request, with per-operation glob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonOp {
    #[serde(flatten)]
    pub op: Op,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<String>,
    /// Every op field as raw JSON (serde's flatten buffering copies ALL
    /// keys here, including ones `Op` consumed — not just leftovers).
    /// Used by validation to detect fields that an op variant silently
    /// dropped, e.g. `multiline` on a line-scoped operation.
    #[serde(flatten, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// An undo request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoRequest {
    pub last: usize,
}

fn default_version() -> String {
    crate::schema::CURRENT_VERSION.to_string()
}

impl JsonRequest {
    /// Parse and validate a JSON request from a string.
    pub fn parse(input: &str) -> Result<Self, RipsedError> {
        let request: JsonRequest = serde_json::from_str(input).map_err(|e| {
            RipsedError::invalid_request(
                format!("Failed to parse JSON request: {e}"),
                "Check that the JSON is well-formed and matches the ripsed request schema.",
            )
        })?;

        request.validate()?;
        Ok(request)
    }

    /// Validate the request after parsing.
    fn validate(&self) -> Result<(), RipsedError> {
        if !crate::schema::is_supported_version(&self.version) {
            return Err(RipsedError::invalid_request(
                format!(
                    "Unknown version '{}'. Supported versions: {}",
                    self.version,
                    crate::schema::SUPPORTED_VERSIONS.join(", ")
                ),
                format!(
                    "Set \"version\": \"{}\" in your request.",
                    crate::schema::CURRENT_VERSION
                ),
            ));
        }

        if self.undo.is_some() && !self.operations.is_empty() {
            return Err(RipsedError::invalid_request(
                "Request cannot contain both 'operations' and 'undo'.",
                "Send undo and operations as separate requests.",
            ));
        }

        if self.undo.is_none() && self.operations.is_empty() {
            return Err(RipsedError::invalid_request(
                "Request must contain 'operations' or 'undo'.",
                "Add at least one operation or an undo request.",
            ));
        }

        // Validate undo request
        if let Some(undo) = &self.undo
            && undo.last == 0
        {
            return Err(RipsedError::invalid_request(
                "Undo 'last' must be at least 1.",
                "Set \"last\" to the number of operations to undo (minimum 1).",
            ));
        }

        // Validate each operation
        for (i, json_op) in self.operations.iter().enumerate() {
            validate_op(i, &json_op.op)?;

            // `multiline` only exists on replace/delete. Serde's flatten
            // buffering copies every key into `extra` (even ones `Op`
            // consumed), so the field's presence there is only meaningful
            // for ops that can't consume it — reject those rather than
            // silently ignore, since an agent that sets it expects it to
            // work. An explicit `false` is a harmless no-op and allowed.
            if !matches!(json_op.op, Op::Replace { .. } | Op::Delete { .. })
                && let Some(value) = json_op.extra.get("multiline")
                && value.as_bool() != Some(false)
            {
                let mut err = RipsedError::invalid_request(
                    format!("Operation {i}: 'multiline' is not supported for this operation type."),
                    "Multiline matching is only available for 'replace' and 'delete' operations.",
                );
                err.operation_index = Some(i);
                return Err(err);
            }

            // Same detection pattern for 'count', which only replace consumes.
            // An explicit "all" is the default and tolerated as a no-op.
            if !matches!(json_op.op, Op::Replace { .. })
                && let Some(value) = json_op.extra.get("count")
                && value.as_str() != Some("all")
            {
                let mut err = RipsedError::invalid_request(
                    format!("Operation {i}: 'count' is not supported for this operation type."),
                    "Replacement counts are only available for 'replace' operations.",
                );
                err.operation_index = Some(i);
                return Err(err);
            }

            // Validate per-operation glob if present
            if let Some(glob) = &json_op.glob {
                validate_glob_pattern(glob).map_err(|msg| {
                    RipsedError::invalid_request(
                        format!("Invalid glob in operation {i}: {msg}"),
                        format!("Fix the glob pattern '{}' in operation {i}. {}", glob, msg),
                    )
                })?;
            }
        }

        // Validate global glob in options
        if let Some(glob) = &self.options.glob {
            validate_glob_pattern(glob).map_err(|msg| {
                RipsedError::invalid_request(
                    format!("Invalid glob in options: {msg}"),
                    format!("Fix the glob pattern '{}' in options. {}", glob, msg),
                )
            })?;
        }

        // Validate ignore glob in options
        if let Some(ignore) = &self.options.ignore {
            validate_glob_pattern(ignore).map_err(|msg| {
                RipsedError::invalid_request(
                    format!("Invalid ignore glob in options: {msg}"),
                    format!("Fix the ignore pattern '{}' in options. {}", ignore, msg),
                )
            })?;
        }

        Ok(())
    }

    /// Extract the list of operations with their effective globs.
    /// Per-operation globs take precedence over the global options glob.
    pub fn into_ops(self) -> (Vec<(Op, Option<String>)>, OpOptions) {
        let global_glob = self.options.glob.clone();
        let ops = self
            .operations
            .into_iter()
            .map(|json_op| {
                let glob = json_op.glob.or_else(|| global_glob.clone());
                (json_op.op, glob)
            })
            .collect();
        (ops, self.options)
    }
}

/// Validate a single operation's fields.
fn validate_op(index: usize, op: &Op) -> Result<(), RipsedError> {
    match op {
        // Note: an empty replacement is valid (it deletes the matched text)
        Op::Replace {
            find,
            regex,
            multiline,
            count,
            ..
        } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for replace."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
            if let ripsed_core::operation::ReplaceCount::Max(0) = count {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'count' max must be at least 1."),
                    format!("Set {{\"max\": n}} with n >= 1 in operation {index}."),
                ));
            }
            if *multiline && matches!(count, ripsed_core::operation::ReplaceCount::FirstPerLine) {
                return Err(RipsedError::invalid_request(
                    format!(
                        "Operation {index}: 'first_per_line' count is not supported with multiline."
                    ),
                    "Per-line counting has no meaning when matching the whole buffer; use 'first_in_file' or {\"max\": n}.",
                ));
            }
        }
        Op::Delete { find, regex, .. } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for delete."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::InsertAfter {
            find,
            content,
            regex,
            ..
        } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for insert_after."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if content.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'content' must not be empty for insert_after."),
                    format!("Set a non-empty 'content' in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::InsertBefore {
            find,
            content,
            regex,
            ..
        } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for insert_before."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if content.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'content' must not be empty for insert_before."),
                    format!("Set a non-empty 'content' in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::ReplaceLine {
            find,
            content,
            regex,
            ..
        } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for replace_line."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if content.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'content' must not be empty for replace_line."),
                    format!("Set a non-empty 'content' in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::Transform { find, regex, .. } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for transform."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::Surround {
            find,
            prefix,
            suffix,
            regex,
            ..
        } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for surround."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if prefix.is_empty() && suffix.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!(
                        "Operation {index}: 'prefix' or 'suffix' must not both be empty for surround."
                    ),
                    format!("Set a non-empty 'prefix' or 'suffix' in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::Indent { find, regex, .. } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for indent."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        Op::Dedent { find, regex, .. } => {
            if find.is_empty() {
                return Err(RipsedError::invalid_request(
                    format!("Operation {index}: 'find' must not be empty for dedent."),
                    format!("Set a non-empty 'find' pattern in operation {index}."),
                ));
            }
            if *regex {
                validate_regex(index, find)?;
            }
        }
        _ => {}
    }

    Ok(())
}

/// Validate that a string compiles as a valid regex.
fn validate_regex(index: usize, pattern: &str) -> Result<(), RipsedError> {
    regex::Regex::new(pattern)
        .map_err(|e| RipsedError::invalid_regex(index, pattern, &e.to_string()))?;
    Ok(())
}

/// Validate a glob pattern for common malformations.
fn validate_glob_pattern(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("Glob pattern must not be empty.".to_string());
    }

    // Check for unmatched brackets
    let mut in_bracket = false;
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                // Skip escaped character
                let _ = chars.next();
            }
            '[' if !in_bracket => {
                in_bracket = true;
            }
            ']' if in_bracket => {
                in_bracket = false;
            }
            '{' => {
                // Check for unmatched braces
                let mut brace_depth = 1;
                let mut found_close = false;
                for next_ch in chars.by_ref() {
                    match next_ch {
                        '{' => brace_depth += 1,
                        '}' => {
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                found_close = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if !found_close {
                    return Err("Unmatched '{' in glob pattern. Add a closing '}'.".to_string());
                }
            }
            '}' => {
                return Err(
                    "Unmatched '}' in glob pattern. Remove the extra '}' or add an opening '{'."
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    if in_bracket {
        return Err("Unmatched '[' in glob pattern. Add a closing ']'.".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic parsing ──

    #[test]
    fn test_parse_simple_replace() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "foo", "replace": "bar"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations.len(), 1);
        assert!(req.options.dry_run); // default
    }

    #[test]
    fn test_parse_invalid_json() {
        let result = JsonRequest::parse("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_operations() {
        let input = r#"{"operations": []}"#;
        let result = JsonRequest::parse(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_unknown_version() {
        let input =
            r#"{"version": "99", "operations": [{"op": "replace", "find": "a", "replace": "b"}]}"#;
        let result = JsonRequest::parse(input);
        assert!(result.is_err());
    }

    // ── Every operation type ──

    #[test]
    fn test_parse_delete() {
        let input = r#"{
            "operations": [{"op": "delete", "find": "TODO", "regex": false}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations.len(), 1);
        match &req.operations[0].op {
            Op::Delete { find, regex, .. } => {
                assert_eq!(find, "TODO");
                assert!(!regex);
            }
            _ => panic!("Expected Delete operation"),
        }
    }

    #[test]
    fn test_parse_delete_with_regex() {
        let input = r#"{
            "operations": [{"op": "delete", "find": "^\\s*//\\s*TODO:.*$", "regex": true}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Delete { find, regex, .. } => {
                assert_eq!(find, r"^\s*//\s*TODO:.*$");
                assert!(regex);
            }
            _ => panic!("Expected Delete operation"),
        }
    }

    #[test]
    fn test_parse_insert_after() {
        let input = r#"{
            "operations": [{
                "op": "insert_after",
                "find": "use serde::Deserialize;",
                "content": "use serde::Serialize;",
                "glob": "src/models/*.rs"
            }]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations.len(), 1);
        match &req.operations[0].op {
            Op::InsertAfter { find, content, .. } => {
                assert_eq!(find, "use serde::Deserialize;");
                assert_eq!(content, "use serde::Serialize;");
            }
            _ => panic!("Expected InsertAfter operation"),
        }
        assert_eq!(req.operations[0].glob.as_deref(), Some("src/models/*.rs"));
    }

    #[test]
    fn test_parse_insert_before() {
        let input = r#"{
            "operations": [{
                "op": "insert_before",
                "find": "fn main()",
                "content": "// Entry point"
            }]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::InsertBefore { find, content, .. } => {
                assert_eq!(find, "fn main()");
                assert_eq!(content, "// Entry point");
            }
            _ => panic!("Expected InsertBefore operation"),
        }
    }

    #[test]
    fn test_parse_replace_line() {
        let input = r#"{
            "operations": [{
                "op": "replace_line",
                "find": "old_version = 1",
                "content": "new_version = 2"
            }]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::ReplaceLine { find, content, .. } => {
                assert_eq!(find, "old_version = 1");
                assert_eq!(content, "new_version = 2");
            }
            _ => panic!("Expected ReplaceLine operation"),
        }
    }

    // ── Validation: empty find ──

    #[test]
    fn test_reject_empty_find_replace() {
        let input = r#"{"operations": [{"op": "replace", "find": "", "replace": "bar"}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'find' must not be empty"));
    }

    #[test]
    fn test_reject_empty_find_delete() {
        let input = r#"{"operations": [{"op": "delete", "find": ""}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'find' must not be empty"));
    }

    #[test]
    fn test_reject_empty_find_insert_after() {
        let input = r#"{"operations": [{"op": "insert_after", "find": "", "content": "x"}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'find' must not be empty"));
    }

    #[test]
    fn test_reject_empty_find_insert_before() {
        let input = r#"{"operations": [{"op": "insert_before", "find": "", "content": "x"}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'find' must not be empty"));
    }

    #[test]
    fn test_reject_empty_find_replace_line() {
        let input = r#"{"operations": [{"op": "replace_line", "find": "", "content": "x"}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'find' must not be empty"));
    }

    // ── Validation: empty content ──

    #[test]
    fn test_reject_empty_content_insert_after() {
        let input = r#"{"operations": [{"op": "insert_after", "find": "x", "content": ""}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'content' must not be empty"));
    }

    #[test]
    fn test_reject_empty_content_insert_before() {
        let input = r#"{"operations": [{"op": "insert_before", "find": "x", "content": ""}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'content' must not be empty"));
    }

    #[test]
    fn test_reject_empty_content_replace_line() {
        let input = r#"{"operations": [{"op": "replace_line", "find": "x", "content": ""}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'content' must not be empty"));
    }

    // ── Replace with empty replacement is valid (acts as deletion) ──

    #[test]
    fn test_allow_empty_replacement_in_replace() {
        let input = r#"{"operations": [{"op": "replace", "find": "remove_me", "replace": ""}]}"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Replace { find, replace, .. } => {
                assert_eq!(find, "remove_me");
                assert_eq!(replace, "");
            }
            _ => panic!("Expected Replace operation"),
        }
    }

    // ── Regex validation ──

    #[test]
    fn test_reject_invalid_regex_in_replace() {
        let input = r#"{"operations": [{"op": "replace", "find": "fn (foo", "replace": "bar", "regex": true}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert_eq!(err.code, ripsed_core::error::ErrorCode::InvalidRegex);
    }

    #[test]
    fn test_reject_invalid_regex_in_delete() {
        let input = r#"{"operations": [{"op": "delete", "find": "[unclosed", "regex": true}]}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert_eq!(err.code, ripsed_core::error::ErrorCode::InvalidRegex);
    }

    #[test]
    fn test_accept_valid_regex_in_delete() {
        let input = r#"{"operations": [{"op": "delete", "find": "^\\s*//.*$", "regex": true}]}"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations.len(), 1);
    }

    // ── Glob validation ──

    #[test]
    fn test_accept_valid_glob() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "glob": "**/*.rs"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations[0].glob.as_deref(), Some("**/*.rs"));
    }

    #[test]
    fn test_reject_empty_glob() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "glob": ""}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Invalid glob"));
    }

    #[test]
    fn test_reject_unmatched_open_bracket() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "glob": "[unclosed"}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Unmatched '['"));
    }

    #[test]
    fn test_reject_unmatched_open_brace() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "glob": "{a,b"}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Unmatched '{'"));
    }

    #[test]
    fn test_reject_unmatched_close_brace() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "glob": "a,b}"}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Unmatched '}'"));
    }

    #[test]
    fn test_accept_valid_alternation_glob() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "glob": "*.{rs,toml}"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations[0].glob.as_deref(), Some("*.{rs,toml}"));
    }

    #[test]
    fn test_reject_empty_options_glob() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "options": {"glob": ""}
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Invalid glob in options"));
    }

    #[test]
    fn test_reject_malformed_options_ignore() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "options": {"ignore": "[bad"}
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Invalid ignore glob"));
    }

    // ── Per-operation glob extraction ──

    #[test]
    fn test_per_op_glob_overrides_global() {
        let input = r#"{
            "operations": [
                {"op": "replace", "find": "a", "replace": "b", "glob": "*.rs"},
                {"op": "delete", "find": "c"}
            ],
            "options": {"glob": "*.py"}
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        let (ops, _options) = req.into_ops();
        // First op has per-op glob, should override global
        assert_eq!(ops[0].1.as_deref(), Some("*.rs"));
        // Second op has no per-op glob, should inherit global
        assert_eq!(ops[1].1.as_deref(), Some("*.py"));
    }

    #[test]
    fn test_no_glob_yields_none() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        let (ops, _) = req.into_ops();
        assert_eq!(ops[0].1, None);
    }

    // ── Undo requests ──

    #[test]
    fn test_parse_undo_request() {
        let input = r#"{"undo": {"last": 3}}"#;
        let req = JsonRequest::parse(input).unwrap();
        assert!(req.operations.is_empty());
        assert_eq!(req.undo.as_ref().unwrap().last, 3);
    }

    #[test]
    fn test_reject_undo_with_operations() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "undo": {"last": 1}
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("both 'operations' and 'undo'"));
    }

    #[test]
    fn test_reject_undo_zero() {
        let input = r#"{"undo": {"last": 0}}"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("'last' must be at least 1"));
    }

    // ── Forward compatibility: extra fields preserved ──

    #[test]
    fn test_extra_top_level_fields_preserved() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "metadata": {"agent": "test-agent", "request_id": "abc123"}
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert!(req.extra.contains_key("metadata"));
        let metadata = req.extra.get("metadata").unwrap();
        assert_eq!(
            metadata.get("agent").and_then(|v| v.as_str()),
            Some("test-agent")
        );
    }

    #[test]
    fn test_unknown_top_level_fields_do_not_cause_error() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "future_field": true,
            "another_thing": [1, 2, 3]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.extra.len(), 2);
    }

    // ── Unknown operation type ──

    #[test]
    fn test_unknown_op_type_rejected() {
        let input = r#"{
            "operations": [{"op": "explode", "find": "a"}]
        }"#;
        let err = JsonRequest::parse(input);
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_transform() {
        let input = r#"{
            "operations": [{"op": "transform", "find": "hello", "mode": "upper"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Transform { find, mode, .. } => {
                assert_eq!(find, "hello");
                assert_eq!(*mode, ripsed_core::operation::TransformMode::Upper);
            }
            _ => panic!("Expected Transform operation"),
        }
    }

    #[test]
    fn test_parse_surround() {
        let input = r#"{
            "operations": [{"op": "surround", "find": "word", "prefix": "(", "suffix": ")"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Surround {
                find,
                prefix,
                suffix,
                ..
            } => {
                assert_eq!(find, "word");
                assert_eq!(prefix, "(");
                assert_eq!(suffix, ")");
            }
            _ => panic!("Expected Surround operation"),
        }
    }

    #[test]
    fn test_parse_indent() {
        let input = r#"{
            "operations": [{"op": "indent", "find": "fn main", "amount": 2}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Indent { find, amount, .. } => {
                assert_eq!(find, "fn main");
                assert_eq!(*amount, 2);
            }
            _ => panic!("Expected Indent operation"),
        }
    }

    #[test]
    fn test_parse_dedent() {
        let input = r#"{
            "operations": [{"op": "dedent", "find": "nested", "amount": 4}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Dedent { find, amount, .. } => {
                assert_eq!(find, "nested");
                assert_eq!(*amount, 4);
            }
            _ => panic!("Expected Dedent operation"),
        }
    }

    // ── Unicode patterns ──

    #[test]
    fn test_unicode_find_replace() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "\u00e9l\u00e8ve", "replace": "\u00e9tudiant"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Replace { find, replace, .. } => {
                assert_eq!(find, "\u{00e9}l\u{00e8}ve");
                assert_eq!(replace, "\u{00e9}tudiant");
            }
            _ => panic!("Expected Replace"),
        }
    }

    #[test]
    fn test_cjk_find_pattern() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "\u4f60\u597d", "replace": "\u5168\u7403"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Replace { find, .. } => {
                assert_eq!(find, "\u{4f60}\u{597d}");
            }
            _ => panic!("Expected Replace"),
        }
    }

    #[test]
    fn test_emoji_in_content() {
        let input = r#"{
            "operations": [{
                "op": "insert_after",
                "find": "// header",
                "content": "// \u2764\ufe0f love this code"
            }]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::InsertAfter { content, .. } => {
                assert!(content.contains('\u{2764}'));
            }
            _ => panic!("Expected InsertAfter"),
        }
    }

    // ── Options parsing ──

    #[test]
    fn test_parse_options() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}],
            "options": {
                "dry_run": false,
                "root": "./my-project",
                "gitignore": true,
                "backup": true,
                "atomic": true,
                "glob": "**/*.rs",
                "hidden": true,
                "max_depth": 5
            }
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert!(!req.options.dry_run);
        assert_eq!(req.options.root.as_deref(), Some("./my-project"));
        assert!(req.options.gitignore);
        assert!(req.options.backup);
        assert!(req.options.atomic);
        assert_eq!(req.options.glob.as_deref(), Some("**/*.rs"));
        assert!(req.options.hidden);
        assert_eq!(req.options.max_depth, Some(5));
    }

    #[test]
    fn test_default_options() {
        let input = r#"{"operations": [{"op": "replace", "find": "a", "replace": "b"}]}"#;
        let req = JsonRequest::parse(input).unwrap();
        assert!(req.options.dry_run);
        assert!(req.options.gitignore);
        assert!(!req.options.backup);
        assert!(!req.options.atomic);
        assert!(!req.options.hidden);
        assert!(req.options.glob.is_none());
        assert!(req.options.root.is_none());
    }

    // ── Multiline flag ──

    #[test]
    fn test_multiline_flag_on_replace_and_delete() {
        let input = r#"{
            "operations": [
                {"op": "replace", "find": "a\nb", "replace": "ab", "multiline": true},
                {"op": "delete", "find": "x\ny", "multiline": true}
            ]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert!(req.operations[0].op.is_multiline());
        assert!(req.operations[1].op.is_multiline());
    }

    #[test]
    fn test_multiline_false_on_line_scoped_op_is_tolerated() {
        // Explicit `"multiline": false` is a no-op everywhere — rejecting it
        // would break agents that emit defaults for every field.
        let input = r#"{
            "operations": [{"op": "insert_after", "find": "a", "content": "b", "multiline": false}]
        }"#;
        assert!(JsonRequest::parse(input).is_ok());
    }

    #[test]
    fn test_multiline_defaults_to_false_when_omitted() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b"}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert!(!req.operations[0].op.is_multiline());
    }

    #[test]
    fn test_multiline_rejected_on_line_scoped_ops() {
        for op_json in [
            r#"{"op": "insert_after", "find": "a", "content": "b", "multiline": true}"#,
            r#"{"op": "transform", "find": "a", "mode": "upper", "multiline": true}"#,
            r#"{"op": "indent", "find": "a", "amount": 2, "multiline": true}"#,
        ] {
            let input = format!(r#"{{"operations": [{op_json}]}}"#);
            let err = JsonRequest::parse(&input).unwrap_err();
            assert_eq!(
                err.code,
                ripsed_core::error::ErrorCode::InvalidRequest,
                "expected rejection for {op_json}"
            );
            assert_eq!(err.operation_index, Some(0));
            assert!(err.message.contains("multiline"));
        }
    }

    // ── Replacement count ──

    #[test]
    fn test_count_accepted_on_replace() {
        let input = r#"{
            "operations": [
                {"op": "replace", "find": "a", "replace": "b", "count": "first_per_line"},
                {"op": "replace", "find": "a", "replace": "b", "count": {"max": 2}}
            ]
        }"#;
        assert!(JsonRequest::parse(input).is_ok());
    }

    #[test]
    fn test_count_max_zero_rejected() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "count": {"max": 0}}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert_eq!(err.code, ripsed_core::error::ErrorCode::InvalidRequest);
        assert!(err.message.contains("max"));
    }

    #[test]
    fn test_count_rejected_on_non_replace_ops() {
        let input = r#"{
            "operations": [{"op": "delete", "find": "a", "count": "first_per_line"}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert_eq!(err.code, ripsed_core::error::ErrorCode::InvalidRequest);
        assert_eq!(err.operation_index, Some(0));
        assert!(err.message.contains("count"));
    }

    #[test]
    fn test_count_all_tolerated_on_non_replace_ops() {
        let input = r#"{
            "operations": [{"op": "delete", "find": "a", "count": "all"}]
        }"#;
        assert!(JsonRequest::parse(input).is_ok());
    }

    #[test]
    fn test_count_first_per_line_with_multiline_rejected() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "a", "replace": "b", "multiline": true, "count": "first_per_line"}]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert_eq!(err.code, ripsed_core::error::ErrorCode::InvalidRequest);
        assert!(err.message.contains("first_per_line"));
    }

    // ── Case insensitive flag ──

    #[test]
    fn test_case_insensitive_flag() {
        let input = r#"{
            "operations": [{"op": "replace", "find": "hello", "replace": "world", "case_insensitive": true}]
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        match &req.operations[0].op {
            Op::Replace {
                case_insensitive, ..
            } => {
                assert!(case_insensitive);
            }
            _ => panic!("Expected Replace"),
        }
    }

    // ── Batch operations ──

    #[test]
    fn test_multiple_operations() {
        let input = r#"{
            "operations": [
                {"op": "replace", "find": "old_fn", "replace": "new_fn", "glob": "src/**/*.rs"},
                {"op": "delete", "find": "^\\s*//\\s*TODO:.*$", "regex": true, "glob": "**/*.rs"},
                {"op": "insert_after", "find": "use serde::Deserialize;", "content": "use serde::Serialize;", "glob": "src/models/*.rs"}
            ],
            "options": {"dry_run": true}
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations.len(), 3);
    }

    // ── Nested validation errors ──

    #[test]
    fn test_first_bad_op_reports_index() {
        let input = r#"{
            "operations": [
                {"op": "replace", "find": "good", "replace": "fine"},
                {"op": "replace", "find": "", "replace": "bad"}
            ]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert!(err.message.contains("Operation 1"));
    }

    #[test]
    fn test_bad_regex_reports_index() {
        let input = r#"{
            "operations": [
                {"op": "replace", "find": "ok", "replace": "fine"},
                {"op": "delete", "find": "[bad", "regex": true}
            ]
        }"#;
        let err = JsonRequest::parse(input).unwrap_err();
        assert_eq!(err.code, ripsed_core::error::ErrorCode::InvalidRegex);
        assert_eq!(err.operation_index, Some(1));
    }

    // ── Design doc example: full agent workflow request ──

    #[test]
    fn test_design_doc_rename_struct_request() {
        let input = r#"{
            "operations": [
                {
                    "op": "replace",
                    "find": "UserConfig",
                    "replace": "AppConfig",
                    "glob": "**/*.rs"
                }
            ],
            "options": { "dry_run": true, "root": "/home/dev/my-project" }
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.operations.len(), 1);
        assert!(req.options.dry_run);
        assert_eq!(req.options.root.as_deref(), Some("/home/dev/my-project"));
        let (ops, _) = req.into_ops();
        assert_eq!(ops[0].1.as_deref(), Some("**/*.rs"));
    }

    #[test]
    fn test_design_doc_full_request_example() {
        let input = r#"{
            "version": "1",
            "operations": [
                {
                    "op": "replace",
                    "find": "old_function_name",
                    "replace": "new_function_name",
                    "regex": false,
                    "glob": "src/**/*.rs",
                    "case_insensitive": false
                },
                {
                    "op": "delete",
                    "find": "^\\s*//\\s*TODO:.*$",
                    "regex": true,
                    "glob": "**/*.rs"
                },
                {
                    "op": "insert_after",
                    "find": "use serde::Deserialize;",
                    "content": "use serde::Serialize;",
                    "glob": "src/models/*.rs"
                }
            ],
            "options": {
                "dry_run": true,
                "root": "./my-project",
                "gitignore": true,
                "backup": false,
                "atomic": true
            }
        }"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.version, "1");
        assert_eq!(req.operations.len(), 3);
        assert!(req.options.dry_run);
        assert!(req.options.atomic);
        assert!(!req.options.backup);
    }

    #[test]
    fn test_design_doc_undo_request() {
        let input = r#"{"undo": {"last": 1}}"#;
        let req = JsonRequest::parse(input).unwrap();
        assert_eq!(req.undo.unwrap().last, 1);
    }
}
