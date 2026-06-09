use crate::diff::{Change, ChangeContext, FileChanges, OpResult};
use crate::error::RipsedError;
use crate::matcher::{MatchSpan, Matcher};
use crate::operation::{LineRange, Op, ReplaceCount, TransformMode};
use crate::undo::UndoEntry;

/// The result of applying operations to a text buffer.
#[derive(Debug)]
pub struct EngineOutput {
    /// The modified text (None if unchanged).
    pub text: Option<String>,
    /// Structured diff of changes made.
    pub changes: Vec<Change>,
    /// Undo entry to reverse this operation.
    pub undo: Option<UndoEntry>,
}

/// The result of applying a single operation to one line.
///
/// Each helper function returns this to tell the main loop what to do with
/// the line and whether a change was recorded.
enum LineAction {
    /// Line is unchanged; push the original.
    Unchanged,
    /// Line was replaced by a new value; push `new_line` and record the change.
    Replaced { new_line: String, change: Change },
    /// Line was deleted; do NOT push anything, but record the change.
    Deleted { change: Change },
    /// A new line was inserted after the original; push both and record the change.
    InsertedAfter { content: String, change: Change },
    /// A new line was inserted before the original; push both and record the change.
    InsertedBefore { content: String, change: Change },
}

/// Detect whether the text predominantly uses CRLF line endings.
///
/// Uses a majority-vote heuristic: counts CRLF (`\r\n`) vs bare LF (`\n`)
/// occurrences and returns `true` only when CRLF is strictly more common.
/// When counts are equal (including zero), prefers LF as the more portable
/// default.  This prevents a file with mixed line endings from being
/// silently normalized to all-CRLF.
fn uses_crlf(text: &str) -> bool {
    let crlf_count = text.matches("\r\n").count();
    let lf_count = text.matches('\n').count() - crlf_count;
    crlf_count > lf_count
}

// ---------------------------------------------------------------------------
// Per-operation helper functions
//
// Each helper inspects one line and returns a `LineAction` indicating what the
// main `apply()` loop should do.  Keeping the logic in dedicated functions
// makes it easy to add new `Op` variants without bloating `apply()`.
// ---------------------------------------------------------------------------

/// Shared context passed to each per-line operation helper.
struct LineCtx<'a> {
    line: &'a str,
    line_num: usize,
    matcher: &'a Matcher,
    lines: &'a [&'a str],
    idx: usize,
    context_lines: usize,
    /// The file's detected line separator — multi-line `Change` metadata
    /// must use this so it matches the bytes actually written.
    line_sep: &'a str,
}

impl LineCtx<'_> {
    fn build_context(&self) -> ChangeContext {
        build_context(self.lines, self.idx, self.context_lines)
    }
}

/// Handle `Op::Replace` — substitute matched text within the line,
/// honoring the operation's [`ReplaceCount`] via the shared `budget`
/// (remaining file-wide occurrences for `FirstInFile`/`Max`, `None`
/// when unlimited).
fn apply_replace(
    cx: &LineCtx,
    replace: &str,
    count: ReplaceCount,
    budget: &mut Option<usize>,
) -> LineAction {
    let line_limit = match (count, budget.as_ref()) {
        (ReplaceCount::FirstPerLine, _) => 1,
        (_, Some(0)) => return LineAction::Unchanged, // budget exhausted
        (_, Some(remaining)) => *remaining,
        (_, None) => 0, // unlimited
    };
    if let Some((replaced, occurrences)) = cx.matcher.replace_n(cx.line, replace, line_limit) {
        if let Some(remaining) = budget {
            *remaining = remaining.saturating_sub(occurrences);
        }
        LineAction::Replaced {
            new_line: replaced.clone(),
            change: Change {
                line: cx.line_num,
                before: cx.line.to_string(),
                after: Some(replaced),
                context: Some(cx.build_context()),
            },
        }
    } else {
        LineAction::Unchanged
    }
}

/// Initial file-wide occurrence budget for a Replace's [`ReplaceCount`]
/// (`None` = unlimited; per-line caps are handled in [`apply_replace`]).
fn replace_budget(op: &Op) -> Option<usize> {
    match op {
        Op::Replace { count, .. } => match count {
            ReplaceCount::All | ReplaceCount::FirstPerLine => None,
            ReplaceCount::FirstInFile => Some(1),
            ReplaceCount::Max(n) => Some(*n),
        },
        _ => None,
    }
}

/// Handle `Op::Delete` — remove the line entirely if matched.
fn apply_delete(cx: &LineCtx) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        LineAction::Deleted {
            change: Change {
                line: cx.line_num,
                before: cx.line.to_string(),
                after: None,
                context: Some(cx.build_context()),
            },
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::InsertAfter` — insert new content after a matched line.
fn apply_insert_after(cx: &LineCtx, content: &str) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        LineAction::InsertedAfter {
            content: content.to_string(),
            change: Change {
                line: cx.line_num,
                before: cx.line.to_string(),
                after: Some(format!("{}{}{content}", cx.line, cx.line_sep)),
                context: Some(cx.build_context()),
            },
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::InsertBefore` — insert new content before a matched line.
fn apply_insert_before(cx: &LineCtx, content: &str) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        LineAction::InsertedBefore {
            content: content.to_string(),
            change: Change {
                line: cx.line_num,
                before: cx.line.to_string(),
                after: Some(format!("{content}{}{}", cx.line_sep, cx.line)),
                context: Some(cx.build_context()),
            },
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::ReplaceLine` — replace the entire line with new content.
fn apply_replace_line(cx: &LineCtx, content: &str) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        LineAction::Replaced {
            new_line: content.to_string(),
            change: Change {
                line: cx.line_num,
                before: cx.line.to_string(),
                after: Some(content.to_string()),
                context: Some(cx.build_context()),
            },
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::Transform` — apply a case/naming transformation to matched text.
fn apply_transform_op(cx: &LineCtx, mode: TransformMode) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        let new_line = apply_transform(cx.line, cx.matcher, mode);
        if new_line != cx.line {
            LineAction::Replaced {
                new_line: new_line.clone(),
                change: Change {
                    line: cx.line_num,
                    before: cx.line.to_string(),
                    after: Some(new_line),
                    context: Some(cx.build_context()),
                },
            }
        } else {
            LineAction::Unchanged
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::Surround` — wrap matched lines with a prefix and suffix.
fn apply_surround(cx: &LineCtx, prefix: &str, suffix: &str) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        let new_line = format!("{prefix}{}{suffix}", cx.line);
        if new_line != cx.line {
            LineAction::Replaced {
                new_line: new_line.clone(),
                change: Change {
                    line: cx.line_num,
                    before: cx.line.to_string(),
                    after: Some(new_line),
                    context: Some(cx.build_context()),
                },
            }
        } else {
            LineAction::Unchanged
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::Indent` — prepend whitespace to matched lines.
fn apply_indent(cx: &LineCtx, amount: usize, use_tabs: bool) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        let indent = if use_tabs {
            "\t".repeat(amount)
        } else {
            " ".repeat(amount)
        };
        let new_line = format!("{indent}{}", cx.line);
        if new_line != cx.line {
            LineAction::Replaced {
                new_line: new_line.clone(),
                change: Change {
                    line: cx.line_num,
                    before: cx.line.to_string(),
                    after: Some(new_line),
                    context: Some(cx.build_context()),
                },
            }
        } else {
            LineAction::Unchanged
        }
    } else {
        LineAction::Unchanged
    }
}

/// Handle `Op::Dedent` — remove leading whitespace from matched lines.
fn apply_dedent(cx: &LineCtx, amount: usize, use_tabs: bool) -> LineAction {
    if cx.matcher.is_match(cx.line) {
        let new_line = dedent_line(cx.line, amount, use_tabs);
        if new_line != cx.line {
            LineAction::Replaced {
                new_line: new_line.clone(),
                change: Change {
                    line: cx.line_num,
                    before: cx.line.to_string(),
                    after: Some(new_line),
                    context: Some(cx.build_context()),
                },
            }
        } else {
            LineAction::Unchanged
        }
    } else {
        LineAction::Unchanged
    }
}

/// Apply a single operation to a text buffer.
///
/// Returns the modified text and a structured diff.
/// If `dry_run` is true, the text is computed but flagged as preview-only.
pub fn apply(
    text: &str,
    op: &Op,
    matcher: &Matcher,
    line_range: Option<LineRange>,
    context_lines: usize,
) -> Result<EngineOutput, RipsedError> {
    if op.is_multiline() {
        if line_range.is_some() {
            return Err(RipsedError::invalid_request(
                "line ranges are not supported in multiline mode",
                "multiline patterns match against the whole buffer; remove the line range or the multiline flag",
            ));
        }
        if matches!(
            op,
            Op::Replace {
                count: ReplaceCount::FirstPerLine,
                ..
            }
        ) {
            return Err(RipsedError::invalid_request(
                "first_per_line count is not supported in multiline mode",
                "per-line counting has no meaning when matching the whole buffer; use first_in_file or {\"max\": n} instead",
            ));
        }
        return Ok(apply_multiline(text, op, matcher, context_lines));
    }

    let crlf = uses_crlf(text);
    let line_sep = if crlf { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.lines().collect();
    let mut result_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut changes: Vec<Change> = Vec::new();
    let mut budget = replace_budget(op);

    for (idx, &line) in lines.iter().enumerate() {
        let line_num = idx + 1; // 1-indexed

        // Skip lines outside the line range
        if let Some(range) = line_range
            && !range.contains(line_num)
        {
            result_lines.push(line.to_string());
            continue;
        }

        let cx = LineCtx {
            line,
            line_num,
            matcher,
            lines: &lines,
            idx,
            context_lines,
            line_sep,
        };

        let action = match op {
            Op::Replace { replace, count, .. } => apply_replace(&cx, replace, *count, &mut budget),
            Op::Delete { .. } => apply_delete(&cx),
            Op::InsertAfter { content, .. } => apply_insert_after(&cx, content),
            Op::InsertBefore { content, .. } => apply_insert_before(&cx, content),
            Op::ReplaceLine { content, .. } => apply_replace_line(&cx, content),
            Op::Transform { mode, .. } => apply_transform_op(&cx, *mode),
            Op::Surround { prefix, suffix, .. } => apply_surround(&cx, prefix, suffix),
            Op::Indent {
                amount, use_tabs, ..
            } => apply_indent(&cx, *amount, *use_tabs),
            Op::Dedent {
                amount, use_tabs, ..
            } => apply_dedent(&cx, *amount, *use_tabs),
        };

        match action {
            LineAction::Unchanged => {
                result_lines.push(line.to_string());
            }
            LineAction::Replaced { new_line, change } => {
                changes.push(change);
                result_lines.push(new_line);
            }
            LineAction::Deleted { change } => {
                changes.push(change);
                // Don't push — line is deleted
            }
            LineAction::InsertedAfter { content, change } => {
                result_lines.push(line.to_string());
                changes.push(change);
                result_lines.push(content);
            }
            LineAction::InsertedBefore { content, change } => {
                changes.push(change);
                result_lines.push(content);
                result_lines.push(line.to_string());
            }
        }
    }

    let modified_text = if changes.is_empty() {
        None
    } else {
        // Preserve line ending style and trailing newline — but only when
        // the result has content. If every line was deleted, the output is
        // a genuinely empty file, not a file containing a single empty line.
        let mut joined = result_lines.join(line_sep);
        if !result_lines.is_empty() && (text.ends_with('\n') || text.ends_with("\r\n")) {
            joined.push_str(line_sep);
        }
        Some(joined)
    };

    let undo = if !changes.is_empty() {
        Some(UndoEntry {
            original_text: text.to_string(),
        })
    } else {
        None
    };

    Ok(EngineOutput {
        text: modified_text,
        changes,
        undo,
    })
}

/// Apply a text transformation to matched portions of a line.
fn apply_transform(line: &str, matcher: &Matcher, mode: TransformMode) -> String {
    match matcher {
        Matcher::Literal { pattern, .. } => {
            line.replace(pattern.as_str(), &transform_text(pattern, mode))
        }
        Matcher::Regex(re) => {
            let result = re.replace_all(line, |caps: &regex::Captures| {
                transform_text(&caps[0], mode)
            });
            result.into_owned()
        }
    }
}

/// Transform a text string according to the given mode.
fn transform_text(text: &str, mode: TransformMode) -> String {
    match mode {
        TransformMode::Upper => text.to_uppercase(),
        TransformMode::Lower => text.to_lowercase(),
        TransformMode::Title => {
            let mut result = String::with_capacity(text.len());
            let mut capitalize_next = true;
            for ch in text.chars() {
                if ch.is_whitespace() || ch == '_' || ch == '-' {
                    result.push(ch);
                    capitalize_next = true;
                } else if capitalize_next {
                    for upper in ch.to_uppercase() {
                        result.push(upper);
                    }
                    capitalize_next = false;
                } else {
                    result.push(ch);
                }
            }
            result
        }
        TransformMode::SnakeCase => {
            let mut result = String::with_capacity(text.len() + 4);
            let mut prev_was_lower = false;
            for ch in text.chars() {
                if ch.is_uppercase() {
                    if prev_was_lower {
                        result.push('_');
                    }
                    for lower in ch.to_lowercase() {
                        result.push(lower);
                    }
                    prev_was_lower = false;
                } else if ch == '-' || ch == ' ' {
                    result.push('_');
                    prev_was_lower = false;
                } else {
                    result.push(ch);
                    prev_was_lower = ch.is_lowercase();
                }
            }
            result
        }
        TransformMode::CamelCase => {
            let mut result = String::with_capacity(text.len());
            let mut capitalize_next = false;
            let mut first = true;
            for ch in text.chars() {
                if ch == '_' || ch == '-' || ch == ' ' {
                    capitalize_next = true;
                } else if capitalize_next {
                    for upper in ch.to_uppercase() {
                        result.push(upper);
                    }
                    capitalize_next = false;
                } else if first {
                    for lower in ch.to_lowercase() {
                        result.push(lower);
                    }
                    first = false;
                } else {
                    result.push(ch);
                    first = false;
                }
            }
            result
        }
    }
}

/// Remove up to `amount` leading whitespace characters from a line.
/// When `use_tabs` is true, strips leading tabs; otherwise strips leading spaces.
fn dedent_line(line: &str, amount: usize, use_tabs: bool) -> String {
    let ch = if use_tabs { '\t' } else { ' ' };
    let leading = line.len() - line.trim_start_matches(ch).len();
    let remove = leading.min(amount);
    line[remove..].to_string()
}

/// Apply a multiline (whole-buffer) Replace or Delete operation.
///
/// Unlike the line-by-line path, the buffer is never split and rejoined:
/// each match span's replacement is spliced into the original text, so line
/// separators outside the matched spans are untouched byte-for-byte and the
/// trailing-newline state is preserved naturally.
///
/// `Change` metadata for a span: `line` is the 1-indexed line where the span
/// starts, `before`/`after` carry the raw span bytes (including any `\r\n`
/// inside it), and `context` holds the lines around the span — so the
/// metadata always matches the bytes actually written.
fn apply_multiline(text: &str, op: &Op, matcher: &Matcher, context_lines: usize) -> EngineOutput {
    // `is_multiline()` is true only for Replace and Delete; Delete removes
    // the matched span (replacement = "").
    let (replacement, is_delete) = match op {
        Op::Replace { replace, .. } => (replace.as_str(), false),
        _ => ("", true),
    };

    let mut spans = matcher.find_replacements(text, replacement);
    // FirstPerLine is rejected before dispatch; FirstInFile and Max
    // simply truncate the span list (occurrence-counted).
    if let Some(limit) = replace_budget(op) {
        spans.truncate(limit);
    }
    if spans.is_empty() {
        return EngineOutput {
            text: None,
            changes: Vec::new(),
            undo: None,
        };
    }

    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    let mut changes = Vec::with_capacity(spans.len());
    let mut last_end = 0usize;

    for MatchSpan {
        start,
        end,
        replacement,
    } in spans
    {
        out.push_str(&text[last_end..start]);
        let before = &text[start..end];
        let start_line_idx = text[..start].matches('\n').count();
        let end_line_idx = start_line_idx + before.matches('\n').count();
        changes.push(Change {
            line: start_line_idx + 1,
            before: before.to_string(),
            after: if is_delete {
                None
            } else {
                Some(replacement.clone())
            },
            context: Some(build_span_context(
                &lines,
                start_line_idx,
                end_line_idx,
                context_lines,
            )),
        });
        out.push_str(&replacement);
        last_end = end;
    }
    out.push_str(&text[last_end..]);

    EngineOutput {
        text: Some(out),
        changes,
        undo: Some(UndoEntry {
            original_text: text.to_string(),
        }),
    }
}

/// Build display context around a span covering `start_idx..=end_idx`
/// (0-indexed line indices): up to `context_lines` lines before the span's
/// first line and after its last line.
fn build_span_context(
    lines: &[&str],
    start_idx: usize,
    end_idx: usize,
    context_lines: usize,
) -> ChangeContext {
    let before_start = start_idx.saturating_sub(context_lines);
    let before = lines[before_start..start_idx.min(lines.len())]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let after_start = (end_idx + 1).min(lines.len());
    let after_end = (end_idx + 1 + context_lines).min(lines.len());
    let after = lines[after_start..after_end]
        .iter()
        .map(|s| s.to_string())
        .collect();
    ChangeContext { before, after }
}

fn build_context(lines: &[&str], idx: usize, context_lines: usize) -> ChangeContext {
    let start = idx.saturating_sub(context_lines);
    let end = (idx + context_lines + 1).min(lines.len());

    let before = lines[start..idx].iter().map(|s| s.to_string()).collect();
    let after = if idx + 1 < end {
        lines[idx + 1..end].iter().map(|s| s.to_string()).collect()
    } else {
        vec![]
    };

    ChangeContext { before, after }
}

/// Build an OpResult from file-level changes.
pub fn build_op_result(operation_index: usize, path: &str, changes: Vec<Change>) -> OpResult {
    OpResult {
        operation_index,
        files: if changes.is_empty() {
            vec![]
        } else {
            vec![FileChanges {
                path: path.to_string(),
                changes,
            }]
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::Matcher;

    #[test]
    fn test_simple_replace() {
        let text = "hello world\nfoo bar\nhello again\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 2).unwrap();
        assert_eq!(result.text.unwrap(), "hi world\nfoo bar\nhi again\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_delete_lines() {
        let text = "keep\ndelete me\nkeep too\n";
        let op = Op::Delete {
            multiline: false,
            find: "delete".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "keep\nkeep too\n");
    }

    #[test]
    fn test_no_changes() {
        let text = "nothing matches here\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "zzz".to_string(),
            replace: "aaa".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_line_range() {
        let text = "line1\nline2\nline3\nline4\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "line".to_string(),
            replace: "row".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let range = Some(LineRange {
            start: 2,
            end: Some(3),
        });
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, range, 0).unwrap();
        assert_eq!(result.text.unwrap(), "line1\nrow2\nrow3\nline4\n");
    }

    // ---------------------------------------------------------------
    // CRLF handling tests
    // ---------------------------------------------------------------

    #[test]
    fn test_crlf_replace_preserves_crlf() {
        let text = "hello world\r\nfoo bar\r\nhello again\r\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hi world\r\nfoo bar\r\nhi again\r\n");
    }

    #[test]
    fn test_crlf_delete_preserves_crlf() {
        let text = "keep\r\ndelete me\r\nkeep too\r\n";
        let op = Op::Delete {
            multiline: false,
            find: "delete".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "keep\r\nkeep too\r\n");
    }

    #[test]
    fn test_crlf_no_trailing_newline() {
        let text = "hello world\r\nfoo bar";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(output, "hi world\r\nfoo bar");
        // No trailing CRLF since original didn't have one
        assert!(!output.ends_with("\r\n"));
    }

    #[test]
    fn test_crlf_insert_after_metadata_uses_crlf() {
        let text = "alpha\r\nbeta\r\n";
        let op = Op::InsertAfter {
            find: "alpha".to_string(),
            content: "inserted".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();

        let output = result.text.unwrap();
        assert_eq!(output, "alpha\r\ninserted\r\nbeta\r\n");

        // The Change.after metadata must show the same bytes the file gets:
        // CRLF between the matched line and the inserted content, not LF.
        let after = result.changes[0].after.as_deref().unwrap();
        assert_eq!(after, "alpha\r\ninserted");
        assert!(
            output.contains(after),
            "metadata {after:?} must appear verbatim in output {output:?}"
        );
    }

    #[test]
    fn test_crlf_insert_before_metadata_uses_crlf() {
        let text = "alpha\r\nbeta\r\n";
        let op = Op::InsertBefore {
            find: "beta".to_string(),
            content: "inserted".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();

        let output = result.text.unwrap();
        assert_eq!(output, "alpha\r\ninserted\r\nbeta\r\n");

        let after = result.changes[0].after.as_deref().unwrap();
        assert_eq!(after, "inserted\r\nbeta");
        assert!(
            output.contains(after),
            "metadata {after:?} must appear verbatim in output {output:?}"
        );
    }

    #[test]
    fn test_lf_insert_after_metadata_uses_lf() {
        // Guard the default: LF files keep LF in the metadata.
        let text = "alpha\nbeta\n";
        let op = Op::InsertAfter {
            find: "alpha".to_string(),
            content: "inserted".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let after = result.changes[0].after.as_deref().unwrap();
        assert_eq!(after, "alpha\ninserted");
    }

    // ── Multiline (whole-buffer) mode ──

    fn multiline_replace_op(find: &str, replace: &str, regex: bool) -> Op {
        Op::Replace {
            count: Default::default(),
            find: find.to_string(),
            replace: replace.to_string(),
            regex,
            case_insensitive: false,
            multiline: true,
        }
    }

    fn multiline_delete_op(find: &str, regex: bool) -> Op {
        Op::Delete {
            find: find.to_string(),
            regex,
            case_insensitive: false,
            multiline: true,
        }
    }

    #[test]
    fn test_multiline_literal_replace_across_lines() {
        let text = "fn old(\n    x: u32,\n) {}\n";
        let op = multiline_replace_op("old(\n    x: u32,\n)", "new(x: u32)", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "fn new(x: u32) {}\n");
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].line, 1);
        assert_eq!(result.changes[0].before, "old(\n    x: u32,\n)");
        assert_eq!(result.changes[0].after.as_deref(), Some("new(x: u32)"));
    }

    #[test]
    fn test_multiline_regex_captures_across_lines() {
        let text = "alpha\nbeta\ngamma\n";
        // Swap the first two lines using a cross-line capture.
        let op = multiline_replace_op(r"(\w+)\n(\w+)\n", "$2\n$1\n", true);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "beta\nalpha\ngamma\n");
        assert_eq!(result.changes[0].after.as_deref(), Some("beta\nalpha\n"));
    }

    #[test]
    fn test_multiline_delete_removes_span_not_lines() {
        let text = "keep [START]\ndoomed\n[END] keep\n";
        let op = multiline_delete_op("[START]\ndoomed\n[END]", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // Only the span is removed; surrounding text on the boundary lines stays.
        assert_eq!(result.text.unwrap(), "keep  keep\n");
        assert_eq!(result.changes[0].after, None);
        assert_eq!(result.changes[0].before, "[START]\ndoomed\n[END]");
    }

    #[test]
    fn test_multiline_crlf_metadata_matches_output_bytes() {
        let text = "alpha\r\nbeta\r\ngamma\r\n";
        let op = multiline_replace_op("alpha\r\nbeta", "one\r\ntwo", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(output, "one\r\ntwo\r\ngamma\r\n");
        let change = &result.changes[0];
        assert_eq!(change.before, "alpha\r\nbeta");
        let after = change.after.as_deref().unwrap();
        assert_eq!(after, "one\r\ntwo");
        assert!(
            output.contains(after),
            "metadata {after:?} must appear verbatim in output {output:?}"
        );
    }

    #[test]
    fn test_multiline_match_at_eof_without_trailing_newline() {
        let text = "head\ntail";
        let op = multiline_replace_op("head\ntail", "joined", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(output, "joined");
        assert!(!output.ends_with('\n'));
    }

    #[test]
    fn test_multiline_preserves_untouched_separators() {
        // Mixed line endings outside the match must pass through untouched —
        // buffer mode never rejoins lines, so no majority-vote normalization.
        let text = "a\r\nMARK\nb\n";
        let op = multiline_replace_op("MARK", "X", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "a\r\nX\nb\n");
    }

    #[test]
    fn test_multiline_with_line_range_is_rejected() {
        let op = multiline_replace_op("a", "b", false);
        let matcher = Matcher::new(&op).unwrap();
        let range = LineRange {
            start: 1,
            end: Some(2),
        };
        let err = apply("a\n", &op, &matcher, Some(range), 0).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::InvalidRequest);
    }

    #[test]
    fn test_multiline_change_line_numbers_ascending_and_correct() {
        let text = "x\nfoo\nx\nfoo\nx\nfoo\n";
        let op = multiline_replace_op("foo\nx", "bar\nx", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // Non-overlapping left-to-right: matches start on lines 2 and 4.
        let line_numbers: Vec<usize> = result.changes.iter().map(|c| c.line).collect();
        assert_eq!(line_numbers, vec![2, 4]);
        assert_eq!(result.text.unwrap(), "x\nbar\nx\nbar\nx\nfoo\n");
    }

    #[test]
    fn test_multiline_no_match_returns_none() {
        let op = multiline_replace_op("absent\npattern", "x", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply("some\ntext\n", &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
        assert!(result.undo.is_none());
    }

    #[test]
    fn test_multiline_delete_everything_yields_empty_string() {
        let text = "all\ngone\n";
        let op = multiline_delete_op("all\ngone\n", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "");
    }

    #[test]
    fn test_multiline_undo_roundtrip() {
        let text = "one\ntwo\nthree\n";
        let op = multiline_replace_op("one\ntwo", "1\n2", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "1\n2\nthree\n");
        assert_eq!(result.undo.unwrap().original_text, text);
    }

    #[test]
    fn test_multiline_output_equals_matcher_replace() {
        // Buffer-mode splicing must reproduce Matcher::replace on the whole
        // text exactly — they share semantics by contract.
        let text = "aaa\nbbb aaa\nccc\n";
        let op = multiline_replace_op("aa", "Z", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(
            result.text.unwrap(),
            matcher.replace(text, "Z").unwrap(),
            "span splicing must match replace_all output"
        );
    }

    #[test]
    fn test_multiline_span_context() {
        let text = "ctx1\nctx2\nA\nB\nctx3\nctx4\n";
        let op = multiline_replace_op("A\nB", "AB", false);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 1).unwrap();
        let ctx = result.changes[0].context.as_ref().unwrap();
        assert_eq!(ctx.before, vec!["ctx2".to_string()]);
        assert_eq!(ctx.after, vec!["ctx3".to_string()]);
    }

    // ── Replacement count control ──

    fn counted_replace_op(find: &str, replace: &str, count: ReplaceCount) -> Op {
        Op::Replace {
            find: find.to_string(),
            replace: replace.to_string(),
            regex: false,
            case_insensitive: false,
            multiline: false,
            count,
        }
    }

    #[test]
    fn test_count_first_per_line_replaces_one_per_line() {
        let text = "a a a\nx\na a\n";
        let op = counted_replace_op("a", "B", ReplaceCount::FirstPerLine);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B a a\nx\nB a\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_count_first_in_file_replaces_only_first_occurrence() {
        let text = "a a\na\na\n";
        let op = counted_replace_op("a", "B", ReplaceCount::FirstInFile);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B a\na\na\n");
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].line, 1);
    }

    #[test]
    fn test_count_max_spans_lines_and_counts_occurrences() {
        // Budget of 3 occurrences: line 1 consumes 2, line 2 consumes the
        // last 1 (partial line), line 3 is untouched.
        let text = "a a\na a\na\n";
        let op = counted_replace_op("a", "B", ReplaceCount::Max(3));
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B B\nB a\na\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_count_all_is_default_behavior() {
        let text = "a a\na\n";
        let op = counted_replace_op("a", "B", ReplaceCount::All);
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B B\nB\n");
    }

    #[test]
    fn test_count_first_per_line_with_regex_captures() {
        let text = "x1 x2 x3\n";
        let op = Op::Replace {
            find: r"x(\d)".to_string(),
            replace: "y$1".to_string(),
            regex: true,
            case_insensitive: false,
            multiline: false,
            count: ReplaceCount::FirstPerLine,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "y1 x2 x3\n");
    }

    #[test]
    fn test_count_max_in_multiline_mode_truncates_spans() {
        let text = "a\na\na\n";
        let op = Op::Replace {
            find: "a\n".to_string(),
            replace: "B\n".to_string(),
            regex: false,
            case_insensitive: false,
            multiline: true,
            count: ReplaceCount::Max(2),
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B\nB\na\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_count_first_in_file_in_multiline_mode() {
        let text = "a\na\n";
        let op = Op::Replace {
            find: "a".to_string(),
            replace: "B".to_string(),
            regex: false,
            case_insensitive: false,
            multiline: true,
            count: ReplaceCount::FirstInFile,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B\na\n");
    }

    #[test]
    fn test_count_first_per_line_rejected_in_multiline_mode() {
        let op = Op::Replace {
            find: "a".to_string(),
            replace: "B".to_string(),
            regex: false,
            case_insensitive: false,
            multiline: true,
            count: ReplaceCount::FirstPerLine,
        };
        let matcher = Matcher::new(&op).unwrap();
        let err = apply("a\n", &op, &matcher, None, 0).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::InvalidRequest);
    }

    #[test]
    fn test_count_budget_exhausted_skips_remaining_lines() {
        // Once the budget hits zero, later matching lines are untouched and
        // produce no Change entries.
        let text = "a\na\na\na\n";
        let op = counted_replace_op("a", "B", ReplaceCount::Max(1));
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "B\na\na\na\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_uses_crlf_detection() {
        assert!(uses_crlf("a\r\nb\r\n"));
        assert!(uses_crlf("a\r\n"));
        assert!(!uses_crlf("a\nb\n"));
        assert!(!uses_crlf("no newline at all"));
        assert!(!uses_crlf(""));
    }

    // ---------------------------------------------------------------
    // Edge-case tests
    // ---------------------------------------------------------------

    #[test]
    fn test_empty_input_text() {
        let text = "";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "anything".to_string(),
            replace: "something".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_single_line_no_trailing_newline() {
        let text = "hello world";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(output, "hi world");
        // Should NOT add a trailing newline that wasn't there
        assert!(!output.ends_with('\n'));
    }

    #[test]
    fn test_whitespace_only_lines() {
        let text = "  \n\t\n   \t  \n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "\t".to_string(),
            replace: "TAB".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert!(output.contains("TAB"));
        assert_eq!(result.changes.len(), 2); // lines 2 and 3 have tabs
    }

    #[test]
    fn test_very_long_line() {
        let long_word = "x".repeat(100_000);
        let text = format!("before\n{long_word}\nafter\n");
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "x".to_string(),
            replace: "y".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(&text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        let expected_long = "y".repeat(100_000);
        assert!(output.contains(&expected_long));
    }

    #[test]
    fn test_unicode_emoji() {
        let text = "hello world\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "world".to_string(),
            replace: "\u{1F30D}".to_string(), // earth globe emoji
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hello \u{1F30D}\n");
    }

    #[test]
    fn test_unicode_cjk() {
        let text = "\u{4F60}\u{597D}\u{4E16}\u{754C}\n"; // "hello world" in Chinese
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "\u{4E16}\u{754C}".to_string(),    // "world"
            replace: "\u{5730}\u{7403}".to_string(), // "earth"
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "\u{4F60}\u{597D}\u{5730}\u{7403}\n");
    }

    #[test]
    fn test_unicode_combining_characters() {
        // e + combining acute accent = e-acute
        let text = "caf\u{0065}\u{0301}\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "caf\u{0065}\u{0301}".to_string(),
            replace: "coffee".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "coffee\n");
    }

    #[test]
    fn test_regex_special_chars_in_literal_mode() {
        // In literal mode, regex metacharacters should be treated as literals
        let text = "price is $10.00 (USD)\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "$10.00".to_string(),
            replace: "$20.00".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "price is $20.00 (USD)\n");
    }

    #[test]
    fn test_overlapping_matches_in_single_line() {
        // "aaa" with pattern "aa" — standard str::replace does non-overlapping left-to-right
        let text = "aaa\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "aa".to_string(),
            replace: "b".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // Rust's str::replace: "aaa".replace("aa", "b") == "ba"
        assert_eq!(result.text.unwrap(), "ba\n");
    }

    #[test]
    fn test_replace_line_count_preserved() {
        let text = "line1\nline2\nline3\nline4\nline5\n";
        let input_line_count = text.lines().count();
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "line".to_string(),
            replace: "row".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        let output_line_count = output.lines().count();
        assert_eq!(input_line_count, output_line_count);
    }

    #[test]
    fn test_replace_preserves_empty_result_on_non_match() {
        // Pattern that exists nowhere in text
        let text = "alpha\nbeta\ngamma\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "zzzzzz".to_string(),
            replace: "y".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.undo.is_none());
    }

    #[test]
    fn test_undo_entry_stores_original() {
        let text = "hello\nworld\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let undo = result.undo.unwrap();
        assert_eq!(undo.original_text, text);
    }

    #[test]
    fn test_determinism_same_input_same_output() {
        let text = "foo bar baz\nhello world\nfoo again\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "foo".to_string(),
            replace: "qux".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let r1 = apply(text, &op, &matcher, None, 0).unwrap();
        let r2 = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(r1.text, r2.text);
        assert_eq!(r1.changes.len(), r2.changes.len());
        for (c1, c2) in r1.changes.iter().zip(r2.changes.iter()) {
            assert_eq!(c1, c2);
        }
    }

    // ---------------------------------------------------------------
    // Transform operation tests
    // ---------------------------------------------------------------

    #[test]
    fn test_transform_upper() {
        let text = "hello world\nfoo bar\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "HELLO world\nfoo bar\n");
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].line, 1);
    }

    #[test]
    fn test_transform_lower() {
        let text = "HELLO WORLD\nFOO BAR\n";
        let op = Op::Transform {
            find: "HELLO".to_string(),
            mode: TransformMode::Lower,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hello WORLD\nFOO BAR\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_transform_noop_when_already_target_case() {
        // Transforming already-lowercase text to Lower should produce no changes
        let text = "hello world\nfoo bar\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Lower,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none(), "No text modification expected");
        assert!(result.changes.is_empty(), "No changes expected");
    }

    #[test]
    fn test_transform_title() {
        let text = "hello world\nfoo bar\n";
        let op = Op::Transform {
            find: "hello world".to_string(),
            mode: TransformMode::Title,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "Hello World\nfoo bar\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_transform_snake_case() {
        let text = "let myVariable = 1;\nother line\n";
        let op = Op::Transform {
            find: "myVariable".to_string(),
            mode: TransformMode::SnakeCase,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "let my_variable = 1;\nother line\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_transform_camel_case() {
        let text = "let my_variable = 1;\nother line\n";
        let op = Op::Transform {
            find: "my_variable".to_string(),
            mode: TransformMode::CamelCase,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "let myVariable = 1;\nother line\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_transform_upper_multiple_matches_on_line() {
        let text = "hello and hello again\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "HELLO and HELLO again\n");
    }

    #[test]
    fn test_transform_no_match() {
        let text = "hello world\n";
        let op = Op::Transform {
            find: "zzz".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_transform_empty_text() {
        let text = "";
        let op = Op::Transform {
            find: "anything".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_transform_with_regex() {
        let text = "let fooBar = 1;\nlet bazQux = 2;\n";
        let op = Op::Transform {
            find: r"[a-z]+[A-Z]\w*".to_string(),
            mode: TransformMode::SnakeCase,
            regex: true,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert!(output.contains("foo_bar"));
        assert!(output.contains("baz_qux"));
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_transform_case_insensitive() {
        let text = "Hello HELLO hello\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: true,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "HELLO HELLO HELLO\n");
    }

    #[test]
    fn test_transform_crlf_preserved() {
        let text = "hello world\r\nfoo bar\r\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "HELLO world\r\nfoo bar\r\n");
    }

    #[test]
    fn test_transform_with_line_range() {
        let text = "hello\nhello\nhello\nhello\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let range = Some(LineRange {
            start: 2,
            end: Some(3),
        });
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, range, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hello\nHELLO\nHELLO\nhello\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_transform_title_with_underscores() {
        let text = "my_func_name\n";
        let op = Op::Transform {
            find: "my_func_name".to_string(),
            mode: TransformMode::Title,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // Title case capitalizes after underscores
        assert_eq!(result.text.unwrap(), "My_Func_Name\n");
    }

    #[test]
    fn test_transform_snake_case_from_multi_word() {
        let text = "my-kebab-case\n";
        let op = Op::Transform {
            find: "my-kebab-case".to_string(),
            mode: TransformMode::SnakeCase,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "my_kebab_case\n");
    }

    #[test]
    fn test_transform_camel_case_from_snake() {
        let text = "my_var_name\n";
        let op = Op::Transform {
            find: "my_var_name".to_string(),
            mode: TransformMode::CamelCase,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "myVarName\n");
    }

    #[test]
    fn test_transform_camel_case_from_kebab() {
        let text = "my-var-name\n";
        let op = Op::Transform {
            find: "my-var-name".to_string(),
            mode: TransformMode::CamelCase,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "myVarName\n");
    }

    // ---------------------------------------------------------------
    // Surround operation tests
    // ---------------------------------------------------------------

    #[test]
    fn test_surround_basic() {
        let text = "hello world\nfoo bar\n";
        let op = Op::Surround {
            find: "hello".to_string(),
            prefix: "<<".to_string(),
            suffix: ">>".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "<<hello world>>\nfoo bar\n");
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].line, 1);
    }

    #[test]
    fn test_surround_multiple_lines() {
        let text = "foo line 1\nbar line 2\nfoo line 3\n";
        let op = Op::Surround {
            find: "foo".to_string(),
            prefix: "[".to_string(),
            suffix: "]".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(
            result.text.unwrap(),
            "[foo line 1]\nbar line 2\n[foo line 3]\n"
        );
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_surround_no_match() {
        let text = "hello world\n";
        let op = Op::Surround {
            find: "zzz".to_string(),
            prefix: "<".to_string(),
            suffix: ">".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_surround_empty_text() {
        let text = "";
        let op = Op::Surround {
            find: "anything".to_string(),
            prefix: "<".to_string(),
            suffix: ">".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_surround_with_regex() {
        let text = "fn main() {\n    let x = 1;\n}\n";
        let op = Op::Surround {
            find: r"fn\s+\w+".to_string(),
            prefix: "/* ".to_string(),
            suffix: " */".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(
            result.text.unwrap(),
            "/* fn main() { */\n    let x = 1;\n}\n"
        );
    }

    #[test]
    fn test_surround_case_insensitive() {
        let text = "Hello world\nhello world\nHELLO world\n";
        let op = Op::Surround {
            find: "hello".to_string(),
            prefix: "(".to_string(),
            suffix: ")".to_string(),
            regex: false,
            case_insensitive: true,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(output, "(Hello world)\n(hello world)\n(HELLO world)\n");
        assert_eq!(result.changes.len(), 3);
    }

    #[test]
    fn test_surround_crlf_preserved() {
        let text = "hello world\r\nfoo bar\r\n";
        let op = Op::Surround {
            find: "hello".to_string(),
            prefix: "[".to_string(),
            suffix: "]".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "[hello world]\r\nfoo bar\r\n");
    }

    #[test]
    fn test_surround_with_line_range() {
        let text = "foo\nfoo\nfoo\nfoo\n";
        let op = Op::Surround {
            find: "foo".to_string(),
            prefix: "<".to_string(),
            suffix: ">".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let range = Some(LineRange {
            start: 2,
            end: Some(3),
        });
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, range, 0).unwrap();
        assert_eq!(result.text.unwrap(), "foo\n<foo>\n<foo>\nfoo\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_surround_with_empty_prefix_and_suffix() {
        let text = "hello world\n";
        let op = Op::Surround {
            find: "hello".to_string(),
            prefix: String::new(),
            suffix: String::new(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // Surround with empty prefix and suffix is a no-op — no change recorded.
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    // ---------------------------------------------------------------
    // Indent operation tests
    // ---------------------------------------------------------------

    #[test]
    fn test_indent_basic() {
        let text = "hello\nworld\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "    hello\nworld\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_indent_multiple_lines() {
        let text = "foo line 1\nbar line 2\nfoo line 3\n";
        let op = Op::Indent {
            find: "foo".to_string(),
            amount: 2,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(
            result.text.unwrap(),
            "  foo line 1\nbar line 2\n  foo line 3\n"
        );
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_indent_with_tabs() {
        let text = "hello\nworld\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 2,
            use_tabs: true,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "\t\thello\nworld\n");
    }

    #[test]
    fn test_indent_no_match() {
        let text = "hello world\n";
        let op = Op::Indent {
            find: "zzz".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_indent_empty_text() {
        let text = "";
        let op = Op::Indent {
            find: "anything".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_indent_zero_amount() {
        let text = "hello\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 0,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // Indent by 0 is a no-op — no change recorded.
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_indent_with_regex() {
        let text = "fn main() {\nlet x = 1;\n}\n";
        let op = Op::Indent {
            find: r"let\s+".to_string(),
            amount: 4,
            use_tabs: false,
            regex: true,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "fn main() {\n    let x = 1;\n}\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_indent_case_insensitive() {
        let text = "Hello\nhello\nHELLO\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 2,
            use_tabs: false,
            regex: false,
            case_insensitive: true,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "  Hello\n  hello\n  HELLO\n");
        assert_eq!(result.changes.len(), 3);
    }

    #[test]
    fn test_indent_crlf_preserved() {
        let text = "hello\r\nworld\r\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "    hello\r\nworld\r\n");
    }

    #[test]
    fn test_indent_with_line_range() {
        let text = "foo\nfoo\nfoo\nfoo\n";
        let op = Op::Indent {
            find: "foo".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let range = Some(LineRange {
            start: 2,
            end: Some(3),
        });
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, range, 0).unwrap();
        assert_eq!(result.text.unwrap(), "foo\n    foo\n    foo\nfoo\n");
        assert_eq!(result.changes.len(), 2);
    }

    // ---------------------------------------------------------------
    // Dedent operation tests
    // ---------------------------------------------------------------

    #[test]
    fn test_dedent_basic() {
        let text = "    hello\nworld\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hello\nworld\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_dedent_partial() {
        // Only 2 spaces of leading whitespace, dedent by 4 should remove only 2
        let text = "  hello\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hello\n");
    }

    #[test]
    fn test_dedent_no_leading_spaces() {
        // Line matches but has no leading spaces -- nothing to remove
        let text = "hello\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // No actual change because line has no leading spaces
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_dedent_multiple_lines() {
        let text = "    foo line 1\n    bar line 2\n    foo line 3\n";
        let op = Op::Dedent {
            find: "foo".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(
            result.text.unwrap(),
            "foo line 1\n    bar line 2\nfoo line 3\n"
        );
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_dedent_no_match() {
        let text = "    hello world\n";
        let op = Op::Dedent {
            find: "zzz".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_dedent_empty_text() {
        let text = "";
        let op = Op::Dedent {
            find: "anything".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert!(result.text.is_none());
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_dedent_with_regex() {
        let text = "    let x = 1;\n    fn main() {\n";
        let op = Op::Dedent {
            find: r"let\s+".to_string(),
            amount: 4,
            use_tabs: false,
            regex: true,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "let x = 1;\n    fn main() {\n");
        assert_eq!(result.changes.len(), 1);
    }

    #[test]
    fn test_dedent_case_insensitive() {
        let text = "    Hello\n    hello\n    HELLO\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 2,
            use_tabs: false,
            regex: false,
            case_insensitive: true,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "  Hello\n  hello\n  HELLO\n");
        assert_eq!(result.changes.len(), 3);
    }

    #[test]
    fn test_dedent_crlf_preserved() {
        let text = "    hello\r\nworld\r\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.text.unwrap(), "hello\r\nworld\r\n");
    }

    #[test]
    fn test_dedent_with_line_range() {
        let text = "    foo\n    foo\n    foo\n    foo\n";
        let op = Op::Dedent {
            find: "foo".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let range = Some(LineRange {
            start: 2,
            end: Some(3),
        });
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, range, 0).unwrap();
        assert_eq!(result.text.unwrap(), "    foo\nfoo\nfoo\n    foo\n");
        assert_eq!(result.changes.len(), 2);
    }

    #[test]
    fn test_dedent_only_removes_spaces_not_tabs() {
        // Dedent only strips leading spaces, not tabs
        let text = "\t\thello\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        // The dedent_line function only strips spaces (trim_start_matches(' ')),
        // tabs are not removed.
        assert!(result.text.is_none());
    }

    // ---------------------------------------------------------------
    // Indent then Dedent roundtrip
    // ---------------------------------------------------------------

    #[test]
    fn test_indent_then_dedent_roundtrip() {
        let original = "hello world\nfoo bar\n";

        // Step 1: Indent by 4
        let indent_op = Op::Indent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let indent_matcher = Matcher::new(&indent_op).unwrap();
        let indented = apply(original, &indent_op, &indent_matcher, None, 0).unwrap();
        let indented_text = indented.text.unwrap();
        assert_eq!(indented_text, "    hello world\nfoo bar\n");

        // Step 2: Dedent by 4 (the find still matches because "hello" is in the line)
        let dedent_op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let dedent_matcher = Matcher::new(&dedent_op).unwrap();
        let dedented = apply(&indented_text, &dedent_op, &dedent_matcher, None, 0).unwrap();
        assert_eq!(dedented.text.unwrap(), original);
    }

    // ---------------------------------------------------------------
    // Undo entry tests for new ops
    // ---------------------------------------------------------------

    #[test]
    fn test_transform_undo_stores_original() {
        let text = "hello world\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.undo.unwrap().original_text, text);
    }

    #[test]
    fn test_surround_undo_stores_original() {
        let text = "hello world\n";
        let op = Op::Surround {
            find: "hello".to_string(),
            prefix: "<".to_string(),
            suffix: ">".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.undo.unwrap().original_text, text);
    }

    #[test]
    fn test_indent_undo_stores_original() {
        let text = "hello world\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.undo.unwrap().original_text, text);
    }

    #[test]
    fn test_dedent_undo_stores_original() {
        let text = "    hello world\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.undo.unwrap().original_text, text);
    }

    // ---------------------------------------------------------------
    // Line preservation tests for new ops
    // ---------------------------------------------------------------

    #[test]
    fn test_transform_preserves_line_count() {
        let text = "hello\nworld\nfoo\n";
        let op = Op::Transform {
            find: "hello".to_string(),
            mode: TransformMode::Upper,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(text.lines().count(), output.lines().count());
    }

    #[test]
    fn test_surround_preserves_line_count() {
        let text = "hello\nworld\nfoo\n";
        let op = Op::Surround {
            find: "hello".to_string(),
            prefix: "<".to_string(),
            suffix: ">".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(text.lines().count(), output.lines().count());
    }

    #[test]
    fn test_indent_preserves_line_count() {
        let text = "hello\nworld\nfoo\n";
        let op = Op::Indent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(text.lines().count(), output.lines().count());
    }

    #[test]
    fn test_dedent_preserves_line_count() {
        let text = "    hello\n    world\n    foo\n";
        let op = Op::Dedent {
            find: "hello".to_string(),
            amount: 4,
            use_tabs: false,
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(text.lines().count(), output.lines().count());
    }

    // ---------------------------------------------------------------
    // Adversarial: line number correctness under odd line endings
    // ---------------------------------------------------------------

    /// **Adversarial**: in a file with mixed line endings, the change
    /// reported for line 3 should correspond to the THIRD *logical line*
    /// (1-indexed), not a byte offset or a line under some alternative
    /// counting scheme. Agents use this line number to navigate.
    #[test]
    fn test_line_numbers_with_mixed_line_endings() {
        // Line 1 = "alpha" (LF)
        // Line 2 = "beta" (CRLF)
        // Line 3 = "gamma match" (LF) — this is where the change should be reported
        // Line 4 = "delta" (CRLF)
        let text = "alpha\nbeta\r\ngamma match\ndelta\r\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "match".to_string(),
            replace: "HIT".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(
            result.changes[0].line, 3,
            "Line number must be 3 regardless of the mixed CR/LF / CRLF endings above it; got {}",
            result.changes[0].line
        );
        assert_eq!(result.changes[0].before, "gamma match");
    }

    /// **Adversarial**: a file consisting entirely of a single line without
    /// any trailing newline still has "line 1" — if that line matches, the
    /// reported change must be on line 1.
    #[test]
    fn test_line_number_for_single_line_no_newline() {
        let text = "only line matches here";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "matches".to_string(),
            replace: "OK".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].line, 1);
    }

    /// **Adversarial**: the first line of a file, when matched, must be
    /// reported as line 1, not line 0 (off-by-one regression guard).
    #[test]
    fn test_first_line_is_one_not_zero() {
        let text = "match first\nother\nother\n";
        let op = Op::Replace {
            count: Default::default(),
            multiline: false,
            find: "match".to_string(),
            replace: "X".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(
            result.changes[0].line, 1,
            "First-line change must be reported as line 1, not 0"
        );
    }

    /// **Adversarial**: when a Delete operation removes the middle line of
    /// a three-line file, the single reported change must reference the
    /// DELETED line's original line number (2), not the new position of
    /// the following line. Undo needs this invariant to restore correctly.
    #[test]
    fn test_delete_reports_original_line_number() {
        let text = "keep1\ndelete_me\nkeep2\n";
        let op = Op::Delete {
            multiline: false,
            find: "delete_me".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        assert_eq!(result.changes.len(), 1);
        assert_eq!(
            result.changes[0].line, 2,
            "Deleted line's reported line must be its original position (2)"
        );
        assert_eq!(result.changes[0].before, "delete_me");
        assert_eq!(result.changes[0].after, None);
    }

    /// **Adversarial regression**: deleting the single line of a one-line
    /// file must produce an empty file (""), not a file containing one
    /// empty line ("\n"). The old engine preserved the trailing newline
    /// unconditionally, which inverted the POSIX "non-existent file vs.
    /// file with empty line" distinction — `wc -l` would say "1" for a
    /// file the user thought they had emptied.
    #[test]
    fn test_delete_all_lines_produces_empty_file() {
        let text = "only line\n";
        let op = Op::Delete {
            multiline: false,
            find: "only".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(
            output, "",
            "Deleting every line must yield an empty file, got {output:?}"
        );
    }

    /// Same invariant for CRLF input.
    #[test]
    fn test_delete_all_lines_crlf_produces_empty_file() {
        let text = "only line\r\n";
        let op = Op::Delete {
            multiline: false,
            find: "only".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let matcher = Matcher::new(&op).unwrap();
        let result = apply(text, &op, &matcher, None, 0).unwrap();
        let output = result.text.unwrap();
        assert_eq!(
            output, "",
            "Deleting every CRLF line must yield an empty file"
        );
    }
}

// ---------------------------------------------------------------
// Property-based tests (proptest)
// ---------------------------------------------------------------
#[cfg(test)]
mod proptests {
    use super::*;
    use crate::matcher::Matcher;
    use crate::operation::Op;
    use proptest::prelude::*;

    /// Strategy for generating text that is multiple lines with a trailing newline.
    fn arb_multiline_text() -> impl Strategy<Value = String> {
        prop::collection::vec("[^\n\r]{0,80}", 1..10).prop_map(|lines| lines.join("\n") + "\n")
    }

    /// Strategy for generating a non-empty find pattern (plain literal).
    fn arb_find_pattern() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9]{1,8}"
    }

    proptest! {
        /// Round-trip: applying a Replace then undoing (restoring original_text)
        /// should give back the original text.
        #[test]
        fn prop_roundtrip_undo(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
            replace in "[a-zA-Z0-9]{0,8}",
        ) {
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find: find.clone(),
                replace: replace.clone(),
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();

            if let Some(undo) = &result.undo {
                // Undo should restore original text
                prop_assert_eq!(&undo.original_text, &text);
            }
            // If no changes, text should be None
            if result.text.is_none() {
                prop_assert!(result.changes.is_empty());
            }
        }

        /// No-op: applying with a pattern that cannot match leaves text unchanged.
        #[test]
        fn prop_noop_nonmatching_pattern(text in arb_multiline_text()) {
            // Use a pattern with a NUL byte which will never appear in text generated
            // by arb_multiline_text
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find: "\x00\x00NOMATCH\x00\x00".to_string(),
                replace: "replacement".to_string(),
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            prop_assert!(result.text.is_none(), "Non-matching pattern should not modify text");
            prop_assert!(result.changes.is_empty());
            prop_assert!(result.undo.is_none());
        }

        /// Determinism: same input always produces same output.
        #[test]
        fn prop_deterministic(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
            replace in "[a-zA-Z0-9]{0,8}",
        ) {
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find,
                replace,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let r1 = apply(&text, &op, &matcher, None, 0).unwrap();
            let r2 = apply(&text, &op, &matcher, None, 0).unwrap();
            prop_assert_eq!(&r1.text, &r2.text);
            prop_assert_eq!(r1.changes.len(), r2.changes.len());
        }

        /// Line count: for Replace ops, output line count == input line count.
        #[test]
        fn prop_replace_preserves_line_count(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
            replace in "[a-zA-Z0-9]{0,8}",
        ) {
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find,
                replace,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            if let Some(ref output) = result.text {
                let input_lines = text.lines().count();
                let output_lines = output.lines().count();
                prop_assert_eq!(
                    input_lines,
                    output_lines,
                    "Replace should preserve line count: input={} output={}",
                    input_lines,
                    output_lines
                );
            }
        }

        /// Indent then Dedent by the same amount should restore the original text
        /// when every line contains the find pattern and starts with enough spaces.
        #[test]
        fn prop_indent_dedent_roundtrip(
            amount in 1usize..=16,
        ) {
            // Use a known find pattern that appears on every line
            let find = "marker".to_string();
            let text = "marker line one\nmarker line two\nmarker line three\n";

            let indent_op = Op::Indent {
                find: find.clone(),
                amount,
                use_tabs: false,
                regex: false,
                case_insensitive: false,
            };
            let indent_matcher = Matcher::new(&indent_op).unwrap();
            let indented = apply(text, &indent_op, &indent_matcher, None, 0).unwrap();
            let indented_text = indented.text.unwrap();

            // Every line should now start with `amount` spaces
            for line in indented_text.lines() {
                let leading = line.len() - line.trim_start_matches(' ').len();
                prop_assert!(leading >= amount, "Expected at least {} leading spaces, got {}", amount, leading);
            }

            let dedent_op = Op::Dedent {
                find: find.clone(),
                amount,
                use_tabs: false,
                regex: false,
                case_insensitive: false,
            };
            let dedent_matcher = Matcher::new(&dedent_op).unwrap();
            let dedented = apply(&indented_text, &dedent_op, &dedent_matcher, None, 0).unwrap();
            prop_assert_eq!(dedented.text.unwrap(), text);
        }

        /// Transform Upper then Lower should restore the original when
        /// the text is already all lowercase ASCII.
        #[test]
        fn prop_transform_upper_lower_roundtrip(
            find in "[a-z]{1,8}",
        ) {
            let text = format!("prefix {find} suffix\n");

            let upper_op = Op::Transform {
                find: find.clone(),
                mode: crate::operation::TransformMode::Upper,
                regex: false,
                case_insensitive: false,
            };
            let upper_matcher = Matcher::new(&upper_op).unwrap();
            let uppered = apply(&text, &upper_op, &upper_matcher, None, 0).unwrap();

            if let Some(ref upper_text) = uppered.text {
                let upper_find = find.to_uppercase();
                let lower_op = Op::Transform {
                    find: upper_find,
                    mode: crate::operation::TransformMode::Lower,
                    regex: false,
                    case_insensitive: false,
                };
                let lower_matcher = Matcher::new(&lower_op).unwrap();
                let lowered = apply(upper_text, &lower_op, &lower_matcher, None, 0).unwrap();
                prop_assert_eq!(lowered.text.unwrap(), text);
            }
        }

        /// Surround preserves line count.
        #[test]
        fn prop_surround_preserves_line_count(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
        ) {
            let op = Op::Surround {
                find,
                prefix: "<<".to_string(),
                suffix: ">>".to_string(),
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            if let Some(ref output) = result.text {
                let input_lines = text.lines().count();
                let output_lines = output.lines().count();
                prop_assert_eq!(
                    input_lines,
                    output_lines,
                    "Surround should preserve line count: input={} output={}",
                    input_lines,
                    output_lines
                );
            }
        }

        /// Transform preserves line count.
        #[test]
        fn prop_transform_preserves_line_count(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
        ) {
            let op = Op::Transform {
                find,
                mode: crate::operation::TransformMode::Upper,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            if let Some(ref output) = result.text {
                let input_lines = text.lines().count();
                let output_lines = output.lines().count();
                prop_assert_eq!(
                    input_lines,
                    output_lines,
                    "Transform should preserve line count: input={} output={}",
                    input_lines,
                    output_lines
                );
            }
        }

        /// Indent preserves line count.
        #[test]
        fn prop_indent_preserves_line_count(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
            amount in 1usize..=16,
        ) {
            let op = Op::Indent {
                find,
                amount,
                use_tabs: false,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            if let Some(ref output) = result.text {
                let input_lines = text.lines().count();
                let output_lines = output.lines().count();
                prop_assert_eq!(
                    input_lines,
                    output_lines,
                    "Indent should preserve line count: input={} output={}",
                    input_lines,
                    output_lines
                );
            }
        }

        /// **Adversarial**: Line numbers recorded in `changes` must be
        /// 1-indexed, strictly within the input's line bounds, and in
        /// ascending order. A violation would break agent tooling that
        /// depends on line numbers to navigate output.
        #[test]
        fn prop_change_lines_ascending_and_in_bounds(
            text in arb_multiline_text(),
            find in arb_find_pattern(),
            replace in "[a-zA-Z0-9]{0,8}",
        ) {
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find,
                replace,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            let max_line = text.lines().count();
            let mut prev: usize = 0;
            for change in &result.changes {
                prop_assert!(change.line >= 1, "Line numbers must be 1-indexed, got {}", change.line);
                prop_assert!(
                    change.line <= max_line,
                    "Line {} out of input range (max {})",
                    change.line,
                    max_line
                );
                prop_assert!(
                    change.line > prev,
                    "Changes must be strictly ascending by line: prev={} current={}",
                    prev,
                    change.line
                );
                prev = change.line;
            }
        }

        /// **Adversarial**: Delete should reduce line count by exactly
        /// the number of matching lines. If the engine ever deletes one
        /// line too many or too few, this will catch it.
        #[test]
        fn prop_delete_exact_line_count(
            // Find must be alphanumeric; we use pure-punctuation filler lines
            // (guaranteed disjoint from the find character class) to avoid
            // accidental matches in "non-matching" lines.
            find in arb_find_pattern(),
            match_count in 0usize..=8,
            nonmatch_count in 0usize..=8,
        ) {
            let match_lines: Vec<String> = (0..match_count)
                .map(|_| format!("-- {} --", find))
                .collect();
            // Filler chars are punctuation only — cannot contain any char
            // from [a-zA-Z0-9], so they cannot contain the find pattern.
            let nonmatch_lines: Vec<String> = (0..nonmatch_count)
                .map(|_| "--- filler ---".to_string())
                .collect();
            // Assert our filler hypothesis before running the engine.
            for l in &nonmatch_lines {
                prop_assume!(!l.contains(&find));
            }
            // Interleave deterministically.
            let mut merged = Vec::with_capacity(match_count + nonmatch_count);
            let mut mi = match_lines.iter();
            let mut ni = nonmatch_lines.iter();
            loop {
                let m = mi.next();
                let n = ni.next();
                if m.is_none() && n.is_none() { break; }
                if let Some(m) = m { merged.push(m.clone()); }
                if let Some(n) = n { merged.push(n.clone()); }
            }
            if merged.is_empty() {
                // Degenerate: no lines at all — nothing interesting to test.
                return Ok(());
            }
            let text = merged.join("\n") + "\n";

            let op = Op::Delete {
                multiline: false,
                find: find.clone(),
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();

            let expected_deletions: usize = text.lines().filter(|l| l.contains(&find)).count();
            prop_assert_eq!(
                expected_deletions,
                match_count,
                "test construction bug: matcher-count must equal match_count"
            );

            if expected_deletions == 0 {
                prop_assert!(result.text.is_none());
                prop_assert!(result.changes.is_empty());
            } else {
                let input_lines = text.lines().count();
                let output_lines = result.text.as_ref().unwrap().lines().count();
                prop_assert_eq!(
                    input_lines - expected_deletions,
                    output_lines,
                    "Delete removed wrong number of lines: expected {} - {} = {}, got {}",
                    input_lines,
                    expected_deletions,
                    input_lines - expected_deletions,
                    output_lines
                );
                prop_assert_eq!(result.changes.len(), expected_deletions);
            }
        }

        /// **Adversarial**: CRLF-majority input produces CRLF-majority output
        /// under any Replace that does not embed newlines. Regression guard
        /// for the uses_crlf majority-vote logic.
        #[test]
        fn prop_crlf_majority_preserved(
            find in "[a-zA-Z]{1,6}",
            replace in "[a-zA-Z]{0,6}",
        ) {
            // Build heavily-CRLF text with enough matches that the replace
            // doesn't turn into a no-op.
            let text = format!(
                "line {find} one\r\n{find} middle\r\nanother {find} here\r\nending\r\n"
            );
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find: find.clone(),
                replace,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            if let Some(ref output) = result.text {
                let crlf = output.matches("\r\n").count();
                let bare_lf = output.matches('\n').count() - crlf;
                prop_assert!(
                    crlf > bare_lf,
                    "CRLF majority lost: {crlf} CRLF vs {bare_lf} bare LF in {output:?}"
                );
            }
        }

        /// **Adversarial**: The count of recorded changes under Replace must
        /// equal the number of input lines that contain the find pattern.
        /// (Replace changes at most one line per match-line, because replacements
        /// are intra-line.) This catches double-counting and missed matches.
        #[test]
        fn prop_replace_change_count_matches_containing_lines(
            find in arb_find_pattern(),
            replace in "[a-zA-Z]{0,8}",
        ) {
            // Build a text with a known, nonzero number of lines containing the pattern.
            let text = format!(
                "head\n{find} one\nmiddle\n{find} two {find}\ntail\n"
            );
            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find: find.clone(),
                replace,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();

            let expected: usize = text.lines().filter(|l| l.contains(&find)).count();
            prop_assert_eq!(result.changes.len(), expected);
        }

        /// **Adversarial**: Text with no trailing newline must remain without
        /// a trailing newline after any Replace operation, even if the
        /// final line is modified. Regression guard for file-end handling.
        #[test]
        fn prop_no_trailing_newline_preserved(
            find in "[a-zA-Z]{1,6}",
            replace in "[a-zA-Z]{1,6}",
        ) {
            // Build text WITHOUT a trailing newline, with the pattern on the
            // last line so replacement happens there.
            let text = format!("first line\nlast line with {find}");
            prop_assume!(!text.ends_with('\n'));

            let op = Op::Replace {
                count: Default::default(),
                multiline: false,
                find,
                replace,
                regex: false,
                case_insensitive: false,
            };
            let matcher = Matcher::new(&op).unwrap();
            let result = apply(&text, &op, &matcher, None, 0).unwrap();
            if let Some(ref output) = result.text {
                prop_assert!(
                    !output.ends_with('\n'),
                    "Spurious trailing newline added to {output:?} (input had none)"
                );
            }
        }
    }
}
