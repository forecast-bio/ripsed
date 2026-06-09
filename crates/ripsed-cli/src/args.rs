use clap::Parser;
use ripsed_core::operation::{LineRange, PatternRange, TransformMode};

/// ripsed — a fast, modern stream editor. Like sed, but better.
#[derive(Parser, Debug)]
#[command(name = "ripsed", version, about)]
pub struct Cli {
    /// Pattern to search for
    pub find: Option<String>,

    /// Replacement string
    pub replace: Option<String>,

    /// Treat FIND as a regex pattern
    #[arg(short = 'e', long)]
    pub regex: bool,

    /// Match across line boundaries (replace and delete only)
    #[arg(
        short = 'U',
        long,
        conflicts_with_all = [
            "after", "before", "replace_line", "transform",
            "surround", "indent", "dedent", "line_range",
        ]
    )]
    pub multiline: bool,

    /// Replace only the first occurrence on each matching line (sed s///)
    #[arg(
        long,
        conflicts_with_all = [
            "first_in_file", "max_replacements", "multiline", "delete",
            "after", "before", "replace_line", "transform",
            "surround", "indent", "dedent",
        ]
    )]
    pub first: bool,

    /// Replace only the first occurrence in each file
    #[arg(
        long,
        conflicts_with_all = [
            "max_replacements", "delete", "after", "before",
            "replace_line", "transform", "surround", "indent", "dedent",
        ]
    )]
    pub first_in_file: bool,

    /// Replace at most N occurrences per file
    #[arg(
        long,
        value_name = "N",
        value_parser = parse_max_replacements,
        conflicts_with_all = [
            "delete", "after", "before", "replace_line",
            "transform", "surround", "indent", "dedent",
        ]
    )]
    pub max_replacements: Option<usize>,

    /// Delete matching lines
    #[arg(short = 'd', long)]
    pub delete: bool,

    /// Read from stdin, write to stdout
    #[arg(short = 'p', long)]
    pub pipe: bool,

    /// Preview changes without writing
    #[arg(long)]
    pub dry_run: bool,

    /// Create .bak files before modifying
    #[arg(long)]
    pub backup: bool,

    /// Only process files matching glob
    #[arg(long)]
    pub glob: Option<String>,

    /// Skip files matching glob
    #[arg(long = "ignore")]
    pub ignore_pattern: Option<String>,

    /// Include hidden files
    #[arg(long)]
    pub hidden: bool,

    /// Don't respect .gitignore
    #[arg(long)]
    pub no_gitignore: bool,

    /// Enable agent/JSON mode
    #[arg(short = 'j', long)]
    pub json: bool,

    /// JSON input as argument (for --json mode)
    #[arg(long, hide = true)]
    pub json_input: Option<String>,

    /// Force human mode even if stdin looks like JSON
    #[arg(long)]
    pub no_json: bool,

    /// Stream results as JSON Lines
    #[arg(long)]
    pub jsonl: bool,

    /// Insert text after matching lines
    #[arg(long)]
    pub after: Option<String>,

    /// Insert text before matching lines
    #[arg(long)]
    pub before: Option<String>,

    /// Replace entire matching line with new content
    #[arg(long)]
    pub replace_line: Option<String>,

    /// Only operate on lines N through M (format: N:M)
    #[arg(short = 'n', long, value_parser = parse_line_range)]
    pub line_range: Option<LineRange>,

    /// Only operate between pattern-matched lines (format: /start/,/end/)
    #[arg(
        long,
        value_name = "/START/,/END/",
        value_parser = parse_pattern_range,
        conflicts_with_all = ["line_range", "multiline"]
    )]
    pub range: Option<PatternRange>,

    /// Maximum directory recursion depth
    #[arg(long, value_parser = parse_max_depth)]
    pub max_depth: Option<usize>,

    /// Case-insensitive matching
    #[arg(long)]
    pub case_insensitive: bool,

    /// Print count of matches/replacements only
    #[arg(short = 'c', long)]
    pub count: bool,

    /// Suppress all non-error output
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Interactive confirmation before each change
    #[arg(long)]
    pub confirm: bool,

    /// Undo last N operations (default: 1)
    #[arg(long, num_args = 0..=1, default_missing_value = "1")]
    pub undo: Option<usize>,

    /// Show recent undo log
    #[arg(long)]
    pub undo_list: bool,

    /// Follow symbolic links during file discovery
    #[arg(long)]
    pub follow: bool,

    /// Path to .ripsed.toml config file
    #[arg(long)]
    pub config: Option<String>,

    /// Transform matched text (modes: upper, lower, title, snake_case, camel_case)
    #[arg(long, value_parser = parse_transform_mode)]
    pub transform: Option<TransformMode>,

    /// Surround matching lines with prefix and suffix
    #[arg(long, num_args = 2, value_names = ["PREFIX", "SUFFIX"])]
    pub surround: Option<Vec<String>>,

    /// Indent matching lines by N spaces
    #[arg(long)]
    pub indent: Option<usize>,

    /// Remove up to N leading spaces from matching lines
    #[arg(long)]
    pub dedent: Option<usize>,

    /// Run operations from a .rip script file
    #[arg(long)]
    pub script: Option<String>,
}

fn parse_transform_mode(s: &str) -> Result<TransformMode, String> {
    s.parse()
}

/// Parse a sed-style pattern range: `/start/,/end/`.
///
/// Both patterns are regexes and are compiled here so a typo fails at
/// argument parsing rather than per-file. Patterns containing the literal
/// sequence `/,/` are not expressible in this syntax.
fn parse_pattern_range(s: &str) -> Result<PatternRange, String> {
    const FORMAT: &str = "range must look like /start/,/end/";
    let inner = s
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
        .ok_or(FORMAT)?;
    let parts: Vec<&str> = inner.splitn(2, "/,/").collect();
    let [start, end] = parts.as_slice() else {
        return Err(FORMAT.to_string());
    };
    for (which, pattern) in [("start", start), ("end", end)] {
        regex::Regex::new(pattern)
            .map_err(|e| format!("invalid {which} pattern '{pattern}': {e}"))?;
    }
    Ok(PatternRange {
        start_pattern: start.to_string(),
        end_pattern: end.to_string(),
    })
}

/// Parse --max-replacements: a positive occurrence cap.
fn parse_max_replacements(s: &str) -> Result<usize, String> {
    match s.parse::<usize>() {
        Ok(0) => Err("max replacements must be at least 1".to_string()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("'{s}' is not a valid number")),
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;

    #[test]
    fn parse_pattern_range_valid() {
        let r = parse_pattern_range("/begin/,/end/").unwrap();
        assert_eq!(r.start_pattern, "begin");
        assert_eq!(r.end_pattern, "end");
    }

    #[test]
    fn parse_pattern_range_regex_metachars_pass_through() {
        let r = parse_pattern_range(r"/\[deps\]/,/^$/").unwrap();
        assert_eq!(r.start_pattern, r"\[deps\]");
        assert_eq!(r.end_pattern, "^$");
    }

    #[test]
    fn parse_pattern_range_malformed_rejected() {
        for bad in ["start,end", "/start/", "/a/,/b", "a/,/b/", ""] {
            assert!(
                parse_pattern_range(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn parse_pattern_range_invalid_regex_rejected() {
        let err = parse_pattern_range("/(unclosed/,/end/").unwrap_err();
        assert!(err.contains("invalid start pattern"));
        let err = parse_pattern_range("/start/,/(unclosed/").unwrap_err();
        assert!(err.contains("invalid end pattern"));
    }
}

/// Parse a line range string in "N:M" format into a `LineRange`.
///
/// Accepted formats:
///   - "N:M" — lines N through M (inclusive, 1-indexed)
///   - "N:"  — line N through end of file
///   - "N"   — shorthand for "N:" (line N through end)
///
/// Both N and M must be >= 1, and if both are present, N <= M.
fn parse_line_range(s: &str) -> Result<LineRange, String> {
    if let Some((start_str, end_str)) = s.split_once(':') {
        let start: usize = start_str
            .parse()
            .map_err(|_| format!("invalid line range start: '{start_str}'"))?;
        if start == 0 {
            return Err("line range start must be >= 1".to_string());
        }
        if end_str.is_empty() {
            Ok(LineRange { start, end: None })
        } else {
            let end: usize = end_str
                .parse()
                .map_err(|_| format!("invalid line range end: '{end_str}'"))?;
            if end == 0 {
                return Err("line range end must be >= 1".to_string());
            }
            if start > end {
                return Err(format!("line range start ({start}) must be <= end ({end})"));
            }
            Ok(LineRange {
                start,
                end: Some(end),
            })
        }
    } else {
        // Single number: treat as "N:" (N through end)
        let start: usize = s
            .parse()
            .map_err(|_| format!("invalid line range: '{s}'"))?;
        if start == 0 {
            return Err("line range start must be >= 1".to_string());
        }
        Ok(LineRange { start, end: None })
    }
}

/// Value parser for `--max-depth` that rejects 0.
fn parse_max_depth(s: &str) -> Result<usize, String> {
    let val: usize = s.parse().map_err(|_| format!("invalid max-depth: '{s}'"))?;
    if val == 0 {
        return Err("max-depth must be >= 1".to_string());
    }
    Ok(val)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_line_range tests ----

    #[test]
    fn test_parse_line_range_full() {
        let r = parse_line_range("5:10").unwrap();
        assert_eq!(r.start, 5);
        assert_eq!(r.end, Some(10));
    }

    #[test]
    fn test_parse_line_range_open_end() {
        let r = parse_line_range("3:").unwrap();
        assert_eq!(r.start, 3);
        assert_eq!(r.end, None);
    }

    #[test]
    fn test_parse_line_range_single_number() {
        let r = parse_line_range("7").unwrap();
        assert_eq!(r.start, 7);
        assert_eq!(r.end, None);
    }

    #[test]
    fn test_parse_line_range_same_start_end() {
        let r = parse_line_range("4:4").unwrap();
        assert_eq!(r.start, 4);
        assert_eq!(r.end, Some(4));
    }

    #[test]
    fn test_parse_line_range_rejects_zero_start() {
        let err = parse_line_range("0:5").unwrap_err();
        assert!(err.contains("must be >= 1"), "got: {err}");
    }

    #[test]
    fn test_parse_line_range_rejects_zero_end() {
        let err = parse_line_range("1:0").unwrap_err();
        assert!(err.contains("must be >= 1"), "got: {err}");
    }

    #[test]
    fn test_parse_line_range_rejects_start_gt_end() {
        let err = parse_line_range("10:5").unwrap_err();
        assert!(err.contains("must be <= end"), "got: {err}");
    }

    #[test]
    fn test_parse_line_range_rejects_non_numeric() {
        assert!(parse_line_range("abc").is_err());
        assert!(parse_line_range("1:xyz").is_err());
        assert!(parse_line_range("abc:5").is_err());
    }

    #[test]
    fn test_parse_line_range_rejects_zero_single() {
        let err = parse_line_range("0").unwrap_err();
        assert!(err.contains("must be >= 1"), "got: {err}");
    }

    // ---- parse_max_depth tests ----

    #[test]
    fn test_parse_max_depth_valid() {
        assert_eq!(parse_max_depth("1").unwrap(), 1);
        assert_eq!(parse_max_depth("5").unwrap(), 5);
        assert_eq!(parse_max_depth("100").unwrap(), 100);
    }

    #[test]
    fn test_parse_max_depth_rejects_zero() {
        let err = parse_max_depth("0").unwrap_err();
        assert!(err.contains("must be >= 1"), "got: {err}");
    }

    #[test]
    fn test_parse_max_depth_rejects_non_numeric() {
        assert!(parse_max_depth("abc").is_err());
        assert!(parse_max_depth("-1").is_err());
    }
}
