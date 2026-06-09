use crate::error::RipsedError;
use crate::operation::{Op, TransformMode};

/// A parsed .rip script file containing a sequence of operations.
#[derive(Debug, Clone)]
pub struct Script {
    pub operations: Vec<ScriptOp>,
}

/// A single operation from a script file, optionally scoped to a glob pattern.
#[derive(Debug, Clone)]
pub struct ScriptOp {
    pub op: Op,
    pub glob: Option<String>,
}

/// Parse a .rip script from its text content.
///
/// Each non-empty, non-comment line is parsed as an operation.
/// Comments start with `#` and blank lines are ignored.
/// Returns a `RipsedError` with line number context on failure.
pub fn parse_script(input: &str) -> Result<Script, RipsedError> {
    let mut operations = Vec::new();

    for (line_idx, raw_line) in input.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = raw_line.trim();

        // Skip blank lines and comment lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Strip inline comments (# not inside quotes)
        let effective = strip_inline_comment(trimmed);

        let script_op = parse_script_line(&effective, line_num)?;
        operations.push(script_op);
    }

    Ok(Script { operations })
}

/// Strip an inline comment from a line, respecting quoted strings.
fn strip_inline_comment(line: &str) -> String {
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut escaped = false;

    for (i, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
        } else if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
        } else if ch == '#' && !in_double_quote && !in_single_quote {
            return line[..i].trim_end().to_string();
        }
    }

    line.to_string()
}

/// Tokenize a script line, respecting quoted strings.
///
/// Handles double-quoted strings with `\"` escapes, single-quoted strings
/// (no escapes), and unquoted tokens separated by whitespace.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    // Track whether we entered a quoted context for this token,
    // so that empty quoted strings like "" produce an empty token.
    let mut had_quote = false;

    while let Some(ch) = chars.next() {
        if in_double_quote {
            if ch == '\\' {
                if let Some(&next) = chars.peek() {
                    match next {
                        '"' | '\\' => {
                            current.push(next);
                            chars.next();
                        }
                        'n' => {
                            current.push('\n');
                            chars.next();
                        }
                        't' => {
                            current.push('\t');
                            chars.next();
                        }
                        _ => {
                            // Preserve the backslash and next char literally.
                            // This keeps regex escapes like \s, \w, \d intact.
                            current.push('\\');
                            current.push(next);
                            chars.next();
                        }
                    }
                } else {
                    current.push('\\');
                }
            } else if ch == '"' {
                in_double_quote = false;
            } else {
                current.push(ch);
            }
        } else if in_single_quote {
            if ch == '\'' {
                in_single_quote = false;
            } else {
                current.push(ch);
            }
        } else if ch == '"' {
            in_double_quote = true;
            had_quote = true;
        } else if ch == '\'' {
            in_single_quote = true;
            had_quote = true;
        } else if ch.is_whitespace() {
            if !current.is_empty() || had_quote {
                tokens.push(std::mem::take(&mut current));
                had_quote = false;
            }
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() || had_quote {
        tokens.push(current);
    }

    tokens
}

/// Parse a single script line into a `ScriptOp`.
fn parse_script_line(line: &str, line_num: usize) -> Result<ScriptOp, RipsedError> {
    let tokens = tokenize(line);

    if tokens.is_empty() {
        return Err(script_error(line_num, "empty operation line"));
    }

    let op_name = tokens[0].to_lowercase();
    let args = &tokens[1..];

    // Extract shared flags from args
    let mut regex = false;
    let mut case_insensitive = false;
    let mut multiline = false;
    let mut count = crate::operation::ReplaceCount::All;
    let mut glob: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut named: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" | "--regex" => regex = true,
            "-i" | "--case-insensitive" => case_insensitive = true,
            "-U" | "--multiline" => multiline = true,
            "--first" => count = crate::operation::ReplaceCount::FirstPerLine,
            "--first-in-file" => count = crate::operation::ReplaceCount::FirstInFile,
            "--max-replacements" => {
                i += 1;
                if i >= args.len() {
                    return Err(script_error(
                        line_num,
                        "--max-replacements requires a value",
                    ));
                }
                let n: usize = args[i].parse().map_err(|_| {
                    script_error(
                        line_num,
                        &format!("--max-replacements: '{}' is not a valid number", args[i]),
                    )
                })?;
                if n == 0 {
                    return Err(script_error(
                        line_num,
                        "--max-replacements must be at least 1",
                    ));
                }
                count = crate::operation::ReplaceCount::Max(n);
            }
            "--glob" => {
                i += 1;
                if i >= args.len() {
                    return Err(script_error(line_num, "--glob requires a value"));
                }
                glob = Some(args[i].clone());
            }
            "--mode" => {
                i += 1;
                if i >= args.len() {
                    return Err(script_error(line_num, "--mode requires a value"));
                }
                named.insert("mode".to_string(), args[i].clone());
            }
            "--prefix" => {
                i += 1;
                if i >= args.len() {
                    return Err(script_error(line_num, "--prefix requires a value"));
                }
                named.insert("prefix".to_string(), args[i].clone());
            }
            "--suffix" => {
                i += 1;
                if i >= args.len() {
                    return Err(script_error(line_num, "--suffix requires a value"));
                }
                named.insert("suffix".to_string(), args[i].clone());
            }
            "--amount" => {
                i += 1;
                if i >= args.len() {
                    return Err(script_error(line_num, "--amount requires a value"));
                }
                named.insert("amount".to_string(), args[i].clone());
            }
            "--use-tabs" => {
                named.insert("use_tabs".to_string(), "true".to_string());
            }
            other => {
                if other.starts_with('-') {
                    return Err(script_error(line_num, &format!("unknown flag '{other}'")));
                }
                positional.push(other.to_string());
            }
        }
        i += 1;
    }

    // --multiline only applies to replace and delete
    if multiline && !matches!(op_name.as_str(), "replace" | "delete") {
        return Err(script_error(
            line_num,
            &format!("--multiline is not supported for '{op_name}' (replace and delete only)"),
        ));
    }

    // Count flags only apply to replace
    if count != crate::operation::ReplaceCount::All && op_name != "replace" {
        return Err(script_error(
            line_num,
            &format!(
                "--first/--first-in-file/--max-replacements are not supported for '{op_name}' (replace only)"
            ),
        ));
    }
    if multiline && count == crate::operation::ReplaceCount::FirstPerLine {
        return Err(script_error(
            line_num,
            "--first cannot be combined with --multiline (use --first-in-file or --max-replacements)",
        ));
    }

    let op = match op_name.as_str() {
        "replace" => {
            require_positional_count(&positional, 2, "replace", line_num)?;
            Op::Replace {
                count,
                multiline,
                find: positional[0].clone(),
                replace: positional[1].clone(),
                regex,
                case_insensitive,
            }
        }
        "delete" => {
            require_positional_count(&positional, 1, "delete", line_num)?;
            Op::Delete {
                multiline,
                find: positional[0].clone(),
                regex,
                case_insensitive,
            }
        }
        "insert_after" => {
            require_positional_count(&positional, 2, "insert_after", line_num)?;
            Op::InsertAfter {
                find: positional[0].clone(),
                content: positional[1].clone(),
                regex,
                case_insensitive,
            }
        }
        "insert_before" => {
            require_positional_count(&positional, 2, "insert_before", line_num)?;
            Op::InsertBefore {
                find: positional[0].clone(),
                content: positional[1].clone(),
                regex,
                case_insensitive,
            }
        }
        "replace_line" => {
            require_positional_count(&positional, 2, "replace_line", line_num)?;
            Op::ReplaceLine {
                find: positional[0].clone(),
                content: positional[1].clone(),
                regex,
                case_insensitive,
            }
        }
        "transform" => {
            require_positional_count(&positional, 1, "transform", line_num)?;
            let mode_str = named
                .get("mode")
                .ok_or_else(|| script_error(line_num, "transform requires --mode <mode>"))?;
            let mode: TransformMode = mode_str
                .parse()
                .map_err(|e: String| script_error(line_num, &e))?;
            Op::Transform {
                find: positional[0].clone(),
                mode,
                regex,
                case_insensitive,
            }
        }
        "surround" => {
            require_positional_count(&positional, 1, "surround", line_num)?;
            let prefix = named
                .get("prefix")
                .ok_or_else(|| script_error(line_num, "surround requires --prefix <value>"))?
                .clone();
            let suffix = named
                .get("suffix")
                .ok_or_else(|| script_error(line_num, "surround requires --suffix <value>"))?
                .clone();
            Op::Surround {
                find: positional[0].clone(),
                prefix,
                suffix,
                regex,
                case_insensitive,
            }
        }
        "indent" => {
            require_positional_count(&positional, 1, "indent", line_num)?;
            let amount = parse_amount(&named, line_num, 4)?;
            let use_tabs = named.contains_key("use_tabs");
            Op::Indent {
                find: positional[0].clone(),
                amount,
                use_tabs,
                regex,
                case_insensitive,
            }
        }
        "dedent" => {
            require_positional_count(&positional, 1, "dedent", line_num)?;
            let amount = parse_amount(&named, line_num, 4)?;
            let use_tabs = named.contains_key("use_tabs");
            Op::Dedent {
                find: positional[0].clone(),
                amount,
                use_tabs,
                regex,
                case_insensitive,
            }
        }
        other => {
            return Err(script_error(
                line_num,
                &format!(
                    "unknown operation '{other}'. Valid operations: replace, delete, \
                     insert_after, insert_before, replace_line, transform, surround, \
                     indent, dedent"
                ),
            ));
        }
    };

    Ok(ScriptOp { op, glob })
}

fn require_positional_count(
    positional: &[String],
    expected: usize,
    op_name: &str,
    line_num: usize,
) -> Result<(), RipsedError> {
    if positional.len() < expected {
        return Err(script_error(
            line_num,
            &format!(
                "'{op_name}' requires {expected} argument(s), got {}",
                positional.len()
            ),
        ));
    }
    Ok(())
}

fn parse_amount(
    named: &std::collections::HashMap<String, String>,
    line_num: usize,
    default: usize,
) -> Result<usize, RipsedError> {
    match named.get("amount") {
        Some(s) => s
            .parse::<usize>()
            .map_err(|_| script_error(line_num, &format!("invalid --amount value: '{s}'"))),
        None => Ok(default),
    }
}

fn script_error(line_num: usize, detail: &str) -> RipsedError {
    RipsedError::invalid_request(
        format!("Script parse error at line {line_num}: {detail}"),
        format!("Check the syntax at line {line_num} of your .rip script file."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::TransformMode;

    // ── Comment and blank line handling ──

    #[test]
    fn empty_script_produces_no_ops() {
        let script = parse_script("").unwrap();
        assert!(script.operations.is_empty());
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let input = r#"
# This is a comment
   # Indented comment

# Another comment
"#;
        let script = parse_script(input).unwrap();
        assert!(script.operations.is_empty());
    }

    #[test]
    fn inline_comments_are_stripped() {
        let input = r#"replace "old" "new" # this is a comment"#;
        let script = parse_script(input).unwrap();
        assert_eq!(script.operations.len(), 1);
        if let Op::Replace { find, replace, .. } = &script.operations[0].op {
            assert_eq!(find, "old");
            assert_eq!(replace, "new");
        } else {
            panic!("expected Replace op");
        }
    }

    #[test]
    fn hash_inside_quotes_is_not_a_comment() {
        let input = r#"replace "old#value" "new#value""#;
        let script = parse_script(input).unwrap();
        assert_eq!(script.operations.len(), 1);
        if let Op::Replace { find, replace, .. } = &script.operations[0].op {
            assert_eq!(find, "old#value");
            assert_eq!(replace, "new#value");
        } else {
            panic!("expected Replace op");
        }
    }

    // ── Quoted string handling ──

    #[test]
    fn double_quoted_strings_with_spaces() {
        let input = r#"replace "hello world" "goodbye world""#;
        let script = parse_script(input).unwrap();
        if let Op::Replace { find, replace, .. } = &script.operations[0].op {
            assert_eq!(find, "hello world");
            assert_eq!(replace, "goodbye world");
        } else {
            panic!("expected Replace op");
        }
    }

    #[test]
    fn single_quoted_strings() {
        let input = "replace 'hello world' 'goodbye world'";
        let script = parse_script(input).unwrap();
        if let Op::Replace { find, replace, .. } = &script.operations[0].op {
            assert_eq!(find, "hello world");
            assert_eq!(replace, "goodbye world");
        } else {
            panic!("expected Replace op");
        }
    }

    #[test]
    fn escaped_quotes_in_double_quotes() {
        let input = r#"replace "say \"hello\"" "say \"goodbye\"""#;
        let script = parse_script(input).unwrap();
        if let Op::Replace { find, replace, .. } = &script.operations[0].op {
            assert_eq!(find, r#"say "hello""#);
            assert_eq!(replace, r#"say "goodbye""#);
        } else {
            panic!("expected Replace op");
        }
    }

    #[test]
    fn unquoted_strings() {
        let input = "replace old_name new_name";
        let script = parse_script(input).unwrap();
        if let Op::Replace { find, replace, .. } = &script.operations[0].op {
            assert_eq!(find, "old_name");
            assert_eq!(replace, "new_name");
        } else {
            panic!("expected Replace op");
        }
    }

    // ── Replace operation ──

    #[test]
    fn parse_replace_basic() {
        let input = r#"replace "old" "new""#;
        let script = parse_script(input).unwrap();
        assert_eq!(script.operations.len(), 1);
        let op = &script.operations[0].op;
        assert_eq!(
            *op,
            Op::Replace {
                count: Default::default(),
                multiline: false,
                find: "old".to_string(),
                replace: "new".to_string(),
                regex: false,
                case_insensitive: false,
            }
        );
    }

    #[test]
    fn parse_replace_with_multiline_flag() {
        for flag in ["--multiline", "-U"] {
            let input = format!(r#"replace {flag} "old" "new""#);
            let script = parse_script(&input).unwrap();
            assert!(
                script.operations[0].op.is_multiline(),
                "{flag} should set multiline"
            );
        }
    }

    #[test]
    fn parse_delete_with_multiline_flag() {
        let input = r#"delete -U "start.*end" -e"#;
        let script = parse_script(input).unwrap();
        let op = &script.operations[0].op;
        assert!(op.is_multiline());
        assert!(op.is_regex());
    }

    #[test]
    fn parse_count_flags_on_replace() {
        use crate::operation::ReplaceCount;
        for (flag, expected) in [
            ("--first", ReplaceCount::FirstPerLine),
            ("--first-in-file", ReplaceCount::FirstInFile),
            ("--max-replacements 3", ReplaceCount::Max(3)),
        ] {
            let input = format!(r#"replace {flag} "old" "new""#);
            let script = parse_script(&input).unwrap();
            match &script.operations[0].op {
                Op::Replace { count, .. } => assert_eq!(*count, expected, "flag {flag}"),
                other => panic!("expected Replace, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_count_flags_rejected_for_non_replace() {
        let err = parse_script(r#"delete "x" --first"#).unwrap_err();
        assert!(err.message.contains("replace only"));
    }

    #[test]
    fn parse_max_replacements_zero_rejected() {
        let err = parse_script(r#"replace "a" "b" --max-replacements 0"#).unwrap_err();
        assert!(err.message.contains("at least 1"));
    }

    #[test]
    fn parse_first_with_multiline_rejected() {
        let err = parse_script(r#"replace "a" "b" --first -U"#).unwrap_err();
        assert!(err.message.contains("cannot be combined"));
    }

    #[test]
    fn parse_multiline_rejected_for_line_scoped_ops() {
        for line in [
            r#"transform "x" --mode upper --multiline"#,
            r#"insert_after "x" "y" -U"#,
            r#"indent "x" --amount 2 --multiline"#,
        ] {
            let err = parse_script(line).unwrap_err();
            assert!(
                err.message.contains("--multiline is not supported"),
                "expected multiline rejection for {line:?}, got: {}",
                err.message
            );
        }
    }

    #[test]
    fn parse_replace_with_regex() {
        let input = r#"replace -e "fn\s+old_(\w+)" "fn new_$1""#;
        let script = parse_script(input).unwrap();
        let op = &script.operations[0].op;
        assert_eq!(
            *op,
            Op::Replace {
                count: Default::default(),
                multiline: false,
                find: r"fn\s+old_(\w+)".to_string(),
                replace: "fn new_$1".to_string(),
                regex: true,
                case_insensitive: false,
            }
        );
    }

    #[test]
    fn parse_replace_with_case_insensitive() {
        let input = r#"replace -i "hello" "goodbye""#;
        let script = parse_script(input).unwrap();
        let op = &script.operations[0].op;
        assert!(op.is_case_insensitive());
    }

    #[test]
    fn parse_replace_with_glob() {
        let input = r#"replace "old" "new" --glob "*.rs""#;
        let script = parse_script(input).unwrap();
        assert_eq!(script.operations[0].glob, Some("*.rs".to_string()));
    }

    // ── Delete operation ──

    #[test]
    fn parse_delete_basic() {
        let input = r#"delete "console.log""#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::Delete {
                multiline: false,
                find: "console.log".to_string(),
                regex: false,
                case_insensitive: false,
            }
        );
    }

    #[test]
    fn parse_delete_with_regex() {
        let input = r#"delete -e "^\s*//\s*TODO:.*$""#;
        let script = parse_script(input).unwrap();
        let op = &script.operations[0].op;
        assert!(op.is_regex());
        assert_eq!(op.find_pattern(), r"^\s*//\s*TODO:.*$");
    }

    // ── InsertAfter operation ──

    #[test]
    fn parse_insert_after() {
        let input = r#"insert_after "use serde;" "use serde_json;""#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::InsertAfter {
                find: "use serde;".to_string(),
                content: "use serde_json;".to_string(),
                regex: false,
                case_insensitive: false,
            }
        );
    }

    // ── InsertBefore operation ──

    #[test]
    fn parse_insert_before() {
        let input = r#"insert_before "fn main" "// Entry point""#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::InsertBefore {
                find: "fn main".to_string(),
                content: "// Entry point".to_string(),
                regex: false,
                case_insensitive: false,
            }
        );
    }

    // ── ReplaceLine operation ──

    #[test]
    fn parse_replace_line() {
        let input = r#"replace_line "version = 1" "version = 2""#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::ReplaceLine {
                find: "version = 1".to_string(),
                content: "version = 2".to_string(),
                regex: false,
                case_insensitive: false,
            }
        );
    }

    // ── Transform operation ──

    #[test]
    fn parse_transform() {
        let input = r#"transform "functionName" --mode snake_case"#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::Transform {
                find: "functionName".to_string(),
                mode: TransformMode::SnakeCase,
                regex: false,
                case_insensitive: false,
            }
        );
    }

    #[test]
    fn parse_transform_upper() {
        let input = r#"transform "hello" --mode upper"#;
        let script = parse_script(input).unwrap();
        if let Op::Transform { mode, .. } = &script.operations[0].op {
            assert_eq!(*mode, TransformMode::Upper);
        } else {
            panic!("expected Transform op");
        }
    }

    // ── Surround operation ──

    #[test]
    fn parse_surround() {
        let input = r#"surround "word" --prefix "(" --suffix ")""#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::Surround {
                find: "word".to_string(),
                prefix: "(".to_string(),
                suffix: ")".to_string(),
                regex: false,
                case_insensitive: false,
            }
        );
    }

    // ── Indent operation ──

    #[test]
    fn parse_indent_with_amount() {
        let input = r#"indent "nested" --amount 4"#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::Indent {
                find: "nested".to_string(),
                amount: 4,
                use_tabs: false,
                regex: false,
                case_insensitive: false,
            }
        );
    }

    #[test]
    fn parse_indent_default_amount() {
        let input = r#"indent "nested""#;
        let script = parse_script(input).unwrap();
        if let Op::Indent { amount, .. } = &script.operations[0].op {
            assert_eq!(*amount, 4);
        } else {
            panic!("expected Indent op");
        }
    }

    // ── Dedent operation ──

    #[test]
    fn parse_dedent() {
        let input = r#"dedent "over_indented" --amount 2"#;
        let script = parse_script(input).unwrap();
        assert_eq!(
            script.operations[0].op,
            Op::Dedent {
                find: "over_indented".to_string(),
                amount: 2,
                use_tabs: false,
                regex: false,
                case_insensitive: false,
            }
        );
    }

    // ── Multi-operation script ──

    #[test]
    fn parse_multi_operation_script() {
        let input = r#"
# Rename old_name to new_name
replace "old_name" "new_name"

# Remove debug lines
delete -e "^\s*console\.log"

# Add import
insert_after "use serde;" "use serde_json;"
"#;
        let script = parse_script(input).unwrap();
        assert_eq!(script.operations.len(), 3);

        assert!(matches!(script.operations[0].op, Op::Replace { .. }));
        assert!(matches!(script.operations[1].op, Op::Delete { .. }));
        assert!(matches!(script.operations[2].op, Op::InsertAfter { .. }));
    }

    // ── Error cases ──

    #[test]
    fn error_unknown_operation() {
        let input = r#"frobnicate "hello" "world""#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("unknown operation"));
        assert!(err.message.contains("line 1"));
    }

    #[test]
    fn error_missing_args_for_replace() {
        let input = r#"replace "only_one_arg""#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("requires 2 argument"));
        assert!(err.message.contains("line 1"));
    }

    #[test]
    fn error_missing_args_for_delete() {
        let input = "delete";
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("requires 1 argument"));
    }

    #[test]
    fn error_transform_missing_mode() {
        let input = r#"transform "hello""#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("--mode"));
    }

    #[test]
    fn error_transform_invalid_mode() {
        let input = r#"transform "hello" --mode invalid_mode"#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("unknown transform mode"));
    }

    #[test]
    fn error_surround_missing_prefix() {
        let input = r#"surround "word" --suffix ")""#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("--prefix"));
    }

    #[test]
    fn error_surround_missing_suffix() {
        let input = r#"surround "word" --prefix "("")"#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("--suffix"));
    }

    #[test]
    fn error_unknown_flag() {
        let input = r#"replace --unknown "hello" "world""#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("unknown flag"));
    }

    #[test]
    fn error_glob_missing_value() {
        let input = r#"replace "a" "b" --glob"#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("--glob requires a value"));
    }

    #[test]
    fn error_invalid_amount() {
        let input = r#"indent "hello" --amount abc"#;
        let err = parse_script(input).unwrap_err();
        assert!(err.message.contains("invalid --amount"));
    }

    #[test]
    fn error_line_number_is_correct() {
        let input = "# comment\n\nreplace \"a\" \"b\"\nbad_op \"x\"";
        let err = parse_script(input).unwrap_err();
        assert!(
            err.message.contains("line 4"),
            "Error should reference line 4, got: {}",
            err.message
        );
    }

    // ── Tokenizer tests ──

    #[test]
    fn tokenize_simple() {
        let tokens = tokenize("replace old new");
        assert_eq!(tokens, vec!["replace", "old", "new"]);
    }

    #[test]
    fn tokenize_double_quoted() {
        let tokens = tokenize(r#"replace "hello world" "goodbye world""#);
        assert_eq!(tokens, vec!["replace", "hello world", "goodbye world"]);
    }

    #[test]
    fn tokenize_single_quoted() {
        let tokens = tokenize("replace 'hello world' 'goodbye world'");
        assert_eq!(tokens, vec!["replace", "hello world", "goodbye world"]);
    }

    #[test]
    fn tokenize_mixed_quotes() {
        let tokens = tokenize(r#"replace 'find this' "replace that""#);
        assert_eq!(tokens, vec!["replace", "find this", "replace that"]);
    }

    #[test]
    fn tokenize_escaped_double_quote() {
        let tokens = tokenize(r#"replace "say \"hi\"" new"#);
        assert_eq!(tokens, vec!["replace", r#"say "hi""#, "new"]);
    }

    #[test]
    fn tokenize_flags() {
        let tokens = tokenize(r#"replace -e "pattern" "replacement" --glob "*.rs""#);
        assert_eq!(
            tokens,
            vec!["replace", "-e", "pattern", "replacement", "--glob", "*.rs"]
        );
    }

    #[test]
    fn tokenize_empty_string() {
        let tokens = tokenize(r#"replace "" "new""#);
        assert_eq!(tokens, vec!["replace", "", "new"]);
    }

    // ── Combination flags ──

    #[test]
    fn parse_replace_regex_case_insensitive_glob() {
        let input = r#"replace -e -i "pattern" "replacement" --glob "*.txt""#;
        let script = parse_script(input).unwrap();
        let sop = &script.operations[0];
        assert!(sop.op.is_regex());
        assert!(sop.op.is_case_insensitive());
        assert_eq!(sop.glob, Some("*.txt".to_string()));
    }

    #[test]
    fn parse_long_form_flags() {
        let input = r#"replace --regex --case-insensitive "a" "b""#;
        let script = parse_script(input).unwrap();
        let op = &script.operations[0].op;
        assert!(op.is_regex());
        assert!(op.is_case_insensitive());
    }

    #[test]
    fn parse_indent_with_use_tabs() {
        let input = r#"indent "nested" --amount 1 --use-tabs"#;
        let script = parse_script(input).unwrap();
        if let Op::Indent {
            amount, use_tabs, ..
        } = &script.operations[0].op
        {
            assert_eq!(*amount, 1);
            assert!(*use_tabs);
        } else {
            panic!("expected Indent op");
        }
    }
}
