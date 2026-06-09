use crate::error::RipsedError;
use crate::operation::Op;
use regex::Regex;

/// One match found by [`Matcher::find_replacements`]: the byte span of the
/// match in the original text and the fully-expanded replacement for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchSpan {
    /// Byte offset of the match start in the original text.
    pub start: usize,
    /// Byte offset one past the match end in the original text.
    pub end: usize,
    /// The replacement text with any capture references (`$1`) expanded.
    pub replacement: String,
}

/// Abstraction over literal and regex matching.
#[derive(Debug)]
pub enum Matcher {
    Literal {
        pattern: String,
    },
    /// A regex matcher — used for both explicit `--regex` patterns and as the
    /// implementation backing case-insensitive literal matching (via
    /// `regex::escape` + `(?i)`), which avoids byte-offset mismatches from
    /// `str::to_lowercase()` on multi-byte Unicode characters.
    Regex(Regex),
}

impl Matcher {
    /// Create a new matcher from an operation.
    pub fn new(op: &Op) -> Result<Self, RipsedError> {
        let pattern = op.find_pattern();
        let is_regex = op.is_regex();
        let case_insensitive = op.is_case_insensitive();

        if is_regex || case_insensitive {
            // For case-insensitive literals, escape the pattern and delegate to
            // the regex engine which handles Unicode casing correctly.
            let re_src = if is_regex {
                pattern.to_string()
            } else {
                regex::escape(pattern)
            };
            let re_pattern = if case_insensitive {
                format!("(?i){re_src}")
            } else {
                re_src
            };
            Regex::new(&re_pattern).map(Matcher::Regex).map_err(|e| {
                let mut err = RipsedError::invalid_regex(0, pattern, &e.to_string());
                err.operation_index = None;
                err
            })
        } else {
            Ok(Matcher::Literal {
                pattern: pattern.to_string(),
            })
        }
    }

    /// Check if the given text matches.
    pub fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Literal { pattern, .. } => text.contains(pattern.as_str()),
            Matcher::Regex(re) => re.is_match(text),
        }
    }

    /// Replace all matches in the given text. Returns None if no matches.
    pub fn replace(&self, text: &str, replacement: &str) -> Option<String> {
        match self {
            Matcher::Literal { pattern, .. } => {
                if text.contains(pattern.as_str()) {
                    Some(text.replace(pattern.as_str(), replacement))
                } else {
                    None
                }
            }
            Matcher::Regex(re) => {
                if re.is_match(text) {
                    Some(re.replace_all(text, replacement).into_owned())
                } else {
                    None
                }
            }
        }
    }

    /// Find every non-overlapping match in `text` and compute its expanded
    /// replacement, left to right.
    ///
    /// Spans are returned in ascending order and never overlap, with the
    /// same semantics as [`Matcher::replace`] (`str::replace` for literals,
    /// `Regex::replace_all` for regexes) — splicing each span's replacement
    /// into the original text reproduces `replace`'s output exactly.
    pub fn find_replacements(&self, text: &str, replacement: &str) -> Vec<MatchSpan> {
        match self {
            Matcher::Literal { pattern } => text
                .match_indices(pattern.as_str())
                .map(|(start, matched)| MatchSpan {
                    start,
                    end: start + matched.len(),
                    replacement: replacement.to_string(),
                })
                .collect(),
            Matcher::Regex(re) => re
                .captures_iter(text)
                .map(|caps| {
                    let m = caps.get(0).expect("capture group 0 always exists");
                    let mut expanded = String::new();
                    caps.expand(replacement, &mut expanded);
                    MatchSpan {
                        start: m.start(),
                        end: m.end(),
                        replacement: expanded,
                    }
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_literal_match() {
        let op = Op::Replace {
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("say hello world"));
        assert!(!m.is_match("say Hi world"));
    }

    #[test]
    fn test_literal_case_insensitive() {
        let op = Op::Replace {
            multiline: false,
            find: "hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: true,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("say HELLO world"));
        assert!(m.is_match("say Hello world"));
    }

    #[test]
    fn test_regex_match() {
        let op = Op::Replace {
            multiline: false,
            find: r"fn\s+(\w+)".to_string(),
            replace: "fn new_$1".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("fn old_func() {"));
        assert!(!m.is_match("let x = 5;"));
    }

    #[test]
    fn test_regex_replace_with_captures() {
        let op = Op::Replace {
            multiline: false,
            find: r"fn\s+old_(\w+)".to_string(),
            replace: "fn new_$1".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("fn old_function() {", "fn new_$1");
        assert_eq!(result, Some("fn new_function() {".to_string()));
    }

    #[test]
    fn test_invalid_regex() {
        let op = Op::Replace {
            multiline: false,
            find: "fn (foo".to_string(),
            replace: "bar".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let err = Matcher::new(&op).unwrap_err();
        assert_eq!(err.code, crate::error::ErrorCode::InvalidRegex);
    }

    // ---------------------------------------------------------------
    // Empty pattern behavior
    // ---------------------------------------------------------------

    #[test]
    fn test_empty_pattern_literal_matches_everything() {
        let op = Op::Replace {
            multiline: false,
            find: "".to_string(),
            replace: "x".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        // An empty string is contained in every string
        assert!(m.is_match("anything"));
        assert!(m.is_match(""));
    }

    #[test]
    fn test_empty_pattern_literal_replace() {
        let op = Op::Replace {
            multiline: false,
            find: "".to_string(),
            replace: "x".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        // Rust's str::replace("", "x") inserts "x" between every char and at start/end
        let result = m.replace("ab", "x");
        assert_eq!(result, Some("xaxbx".to_string()));
    }

    #[test]
    fn test_empty_pattern_regex_matches_everything() {
        let op = Op::Replace {
            multiline: false,
            find: "".to_string(),
            replace: "x".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("anything"));
        assert!(m.is_match(""));
    }

    // ---------------------------------------------------------------
    // Pattern that matches entire line
    // ---------------------------------------------------------------

    #[test]
    fn test_pattern_matches_entire_line_literal() {
        let op = Op::Replace {
            multiline: false,
            find: "hello world".to_string(),
            replace: "goodbye".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("hello world", "goodbye");
        assert_eq!(result, Some("goodbye".to_string()));
    }

    #[test]
    fn test_pattern_matches_entire_line_regex() {
        let op = Op::Replace {
            multiline: false,
            find: r"^.*$".to_string(),
            replace: "replaced".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("anything here", "replaced");
        assert_eq!(result, Some("replaced".to_string()));
    }

    #[test]
    fn test_regex_anchored_full_line() {
        let op = Op::Replace {
            multiline: false,
            find: r"^fn main\(\)$".to_string(),
            replace: "fn start()".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("fn main()"));
        assert!(!m.is_match("  fn main()")); // leading whitespace
        assert!(!m.is_match("fn main() {")); // trailing content
    }

    // ---------------------------------------------------------------
    // Case-insensitive with unicode (Turkish I problem, etc.)
    // ---------------------------------------------------------------

    #[test]
    fn test_case_insensitive_ascii() {
        let op = Op::Replace {
            multiline: false,
            find: "Hello".to_string(),
            replace: "hi".to_string(),
            regex: false,
            case_insensitive: true,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("HELLO"));
        assert!(m.is_match("hello"));
        assert!(m.is_match("HeLLo"));
        let result = m.replace("say HELLO there", "hi");
        assert_eq!(result, Some("say hi there".to_string()));
    }

    #[test]
    fn test_case_insensitive_german_eszett() {
        // German sharp-s: lowercase to_lowercase() of "SS" is "ss",
        // and to_lowercase() of "\u{00DF}" (sharp-s) is "\u{00DF}"
        // This tests that the engine handles non-trivial unicode casing
        let op = Op::Replace {
            multiline: false,
            find: "stra\u{00DF}e".to_string(), // "strasse" with sharp-s
            replace: "street".to_string(),
            regex: false,
            case_insensitive: true,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("STRA\u{00DF}E"));
    }

    #[test]
    fn test_case_insensitive_turkish_i_lowercase() {
        // Turkish dotted I: \u{0130} (capital I with dot above)
        // This is a known edge case. We test that the matcher doesn't panic
        // and behaves consistently with Unicode simple case folding.
        let op = Op::Replace {
            multiline: false,
            find: "i".to_string(),
            replace: "x".to_string(),
            regex: false,
            case_insensitive: true,
        };
        let m = Matcher::new(&op).unwrap();
        // Standard ASCII: "I" simple-folds to "i", so this matches
        assert!(m.is_match("I"));
        // \u{0130} (İ) has no simple case fold to "i" in Unicode — the full
        // fold is "i\u{0307}" but the regex engine only uses simple folds.
        // This correctly does NOT match, avoiding false positives from the
        // old to_lowercase()-based byte-offset approach.
        assert!(!m.is_match("\u{0130}"));
    }

    // ---------------------------------------------------------------
    // Regex special characters in literal mode
    // ---------------------------------------------------------------

    #[test]
    fn test_literal_mode_regex_metacharacters() {
        // All these are regex metacharacters but should be treated literally
        let patterns = vec![
            (".", "dot"),
            ("*", "star"),
            ("+", "plus"),
            ("?", "question"),
            ("(", "paren"),
            ("[", "bracket"),
            ("{", "brace"),
            ("^", "caret"),
            ("$", "dollar"),
            ("|", "pipe"),
            ("\\", "backslash"),
        ];
        for (pat, name) in patterns {
            let op = Op::Replace {
                multiline: false,
                find: pat.to_string(),
                replace: "X".to_string(),
                regex: false,
                case_insensitive: false,
            };
            let m = Matcher::new(&op).unwrap();
            let text = format!("before {pat} after");
            assert!(
                m.is_match(&text),
                "Literal mode should match '{name}' ({pat}) as a literal character"
            );
            let result = m.replace(&text, "X");
            assert_eq!(
                result,
                Some("before X after".to_string()),
                "Literal mode should replace '{name}' ({pat}) as a literal"
            );
        }
    }

    // ---------------------------------------------------------------
    // Multiple matches on same line
    // ---------------------------------------------------------------

    #[test]
    fn test_multiple_matches_same_line() {
        let op = Op::Replace {
            multiline: false,
            find: "ab".to_string(),
            replace: "X".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("ab cd ab ef ab", "X");
        assert_eq!(result, Some("X cd X ef X".to_string()));
    }

    #[test]
    fn test_replace_with_empty_string() {
        let op = Op::Replace {
            multiline: false,
            find: "remove".to_string(),
            replace: "".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("please remove this", "");
        assert_eq!(result, Some("please  this".to_string()));
    }

    #[test]
    fn test_no_match_returns_none() {
        let op = Op::Replace {
            multiline: false,
            find: "xyz".to_string(),
            replace: "abc".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.replace("nothing here", "abc").is_none());
    }

    // ---------------------------------------------------------------
    // Pathological / adversarial pattern tests
    //
    // These lock in behavior for patterns that look like they ought to
    // break something: regex metacharacters misused in literal mode,
    // empty inputs, patterns with backreference-like replacement strings,
    // and regex that would blow up a backtracking engine.
    // ---------------------------------------------------------------

    /// A literal pattern of `$1` (which would be a capture backreference in
    /// a regex replacement context) must match the literal two-character
    /// sequence in text and be replaceable without invoking capture-group
    /// semantics. Regression guard against anyone accidentally swapping
    /// `str::replace` for `Regex::replace_all` in the literal path.
    #[test]
    fn test_literal_dollar_one_pattern() {
        let op = Op::Replace {
            multiline: false,
            find: "$1".to_string(),
            replace: "REPLACED".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.is_match("value is $1 here"));
        let result = m.replace("value is $1 here", "REPLACED");
        assert_eq!(result, Some("value is REPLACED here".to_string()));
    }

    /// A regex pattern whose replacement string contains `$0`, `$1`, etc.
    /// should be interpreted as a capture-backreference in regex mode.
    /// This is intended behavior; locking it in so nobody accidentally
    /// escapes it.
    #[test]
    fn test_regex_backreferences_work_in_replace() {
        let op = Op::Replace {
            multiline: false,
            find: r"hello (\w+)".to_string(),
            replace: "greetings, $1!".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("hello world", "greetings, $1!");
        assert_eq!(result, Some("greetings, world!".to_string()));
    }

    /// **Adversarial**: the classic "catastrophic backtracking" pattern
    /// `(a+)+$` on a long non-matching input is O(2^n) in a naive NFA.
    /// The `regex` crate uses a DFA/bounded-time engine so this should
    /// complete effectively instantly. Lock in that we've picked a safe
    /// engine — switching to a backtracking regex crate would hang here.
    #[test]
    fn test_regex_no_catastrophic_backtracking() {
        let op = Op::Replace {
            multiline: false,
            find: r"(a+)+$".to_string(),
            replace: "X".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        // 30 'a's followed by 'b' — classic ReDoS trigger for backtracking engines.
        let mut input = "a".repeat(30);
        input.push('b');
        let start = std::time::Instant::now();
        let result = m.is_match(&input);
        let elapsed = start.elapsed();
        assert!(!result, "pattern should not match 'aaaa...b'");
        // Generous bound — should actually complete in microseconds.
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "regex took too long ({elapsed:?}) — possible ReDoS"
        );
    }

    /// **Adversarial**: the replacement string is NUL-separated or contains
    /// control characters. Must pass through unchanged (no shell-like
    /// interpretation).
    #[test]
    fn test_replacement_with_control_chars() {
        let op = Op::Replace {
            multiline: false,
            find: "placeholder".to_string(),
            replace: "\x07bell\x1bescape\x00nul".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let result = m.replace("use placeholder here", "\x07bell\x1bescape\x00nul");
        assert_eq!(
            result,
            Some("use \x07bell\x1bescape\x00nul here".to_string())
        );
    }

    /// **Adversarial**: a regex that is a valid-but-empty-matching pattern
    /// (like `(?:)`) produces an empty match at every position. This is a
    /// weird edge case that can blow up naive replace loops. Lock in that
    /// we produce *some* deterministic output without panicking.
    #[test]
    fn test_empty_regex_match_does_not_panic() {
        let op = Op::Replace {
            multiline: false,
            find: r"(?:)".to_string(),
            replace: "X".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        // Must not panic — actual content of the result is implementation-defined.
        let _ = m.replace("abc", "X");
    }
}

// ---------------------------------------------------------------
// Property-based tests (proptest)
// ---------------------------------------------------------------
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Invariant: in literal mode, `Matcher::is_match(text)` ⟺
        /// `text.contains(pattern)`. This guards against a future optimization
        /// accidentally changing the semantics of literal matching.
        #[test]
        fn prop_literal_matches_iff_contains(
            pattern in "[a-zA-Z0-9 ]{1,10}",
            text in "[a-zA-Z0-9 ]{0,60}",
        ) {
            let op = Op::Replace {
                multiline: false,
                find: pattern.clone(),
                replace: "".into(),
                regex: false,
                case_insensitive: false,
            };
            let m = Matcher::new(&op).unwrap();
            prop_assert_eq!(m.is_match(&text), text.contains(&pattern));
        }

        /// Invariant: `replace(text, pat)` returns `None` iff `is_match(text)`
        /// is `false`. A mismatch here means we'd record a spurious "change"
        /// with no actual edit.
        #[test]
        fn prop_replace_none_iff_not_match(
            pattern in "[a-zA-Z0-9]{1,6}",
            text in "[a-zA-Z0-9]{0,40}",
            replacement in "[a-zA-Z0-9]{0,6}",
        ) {
            let op = Op::Replace {
                multiline: false,
                find: pattern.clone(),
                replace: replacement.clone(),
                regex: false,
                case_insensitive: false,
            };
            let m = Matcher::new(&op).unwrap();
            let is_match = m.is_match(&text);
            let replaced = m.replace(&text, &replacement);
            prop_assert_eq!(replaced.is_some(), is_match);
        }

        /// Invariant: replacing pattern with itself is a no-op on content
        /// (the returned String equals the input). This is a fixed-point
        /// test that catches mis-implementations of the literal replace path.
        #[test]
        fn prop_replace_with_self_is_identity(
            pattern in "[a-zA-Z0-9]{1,6}",
            text in "[a-zA-Z0-9 ]{0,50}",
        ) {
            let op = Op::Replace {
                multiline: false,
                find: pattern.clone(),
                replace: pattern.clone(),
                regex: false,
                case_insensitive: false,
            };
            let m = Matcher::new(&op).unwrap();
            if let Some(replaced) = m.replace(&text, &pattern) {
                prop_assert_eq!(replaced, text);
            }
        }

        /// Invariant: case-insensitive literal matching is symmetric —
        /// `Matcher(p, ci=true).is_match(t)` equals
        /// `Matcher(t.to_lowercase(), ci=false).is_match(p.to_lowercase())`
        /// for ASCII patterns. (Restricts to ASCII because Unicode case folding
        /// is famously asymmetric; our ASCII invariant is what callers rely on.)
        #[test]
        fn prop_case_insensitive_ascii_symmetric(
            pattern in "[a-zA-Z]{1,6}",
            text in "[a-zA-Z]{0,30}",
        ) {
            let op = Op::Replace {
                multiline: false,
                find: pattern.clone(),
                replace: String::new(),
                regex: false,
                case_insensitive: true,
            };
            let m = Matcher::new(&op).unwrap();
            let matches = m.is_match(&text);
            prop_assert_eq!(
                matches,
                text.to_ascii_lowercase().contains(&pattern.to_ascii_lowercase())
            );
        }

        /// Invariant: splicing `find_replacements` spans into the original
        /// text reproduces `replace`'s output exactly — the two APIs must
        /// never drift apart.
        #[test]
        fn prop_find_replacements_splice_equals_replace(
            text in ".{0,60}",
            pattern in ".{1,5}",
            replacement in ".{0,8}",
        ) {
            let op = Op::Replace {
                multiline: false,
                find: pattern.clone(),
                replace: replacement.clone(),
                regex: false,
                case_insensitive: false,
            };
            let m = Matcher::new(&op).unwrap();
            let spans = m.find_replacements(&text, &replacement);
            let mut spliced = String::new();
            let mut last = 0;
            for s in &spans {
                spliced.push_str(&text[last..s.start]);
                spliced.push_str(&s.replacement);
                last = s.end;
            }
            spliced.push_str(&text[last..]);
            let expected = m.replace(&text, &replacement).unwrap_or_else(|| text.clone());
            prop_assert_eq!(spliced, expected);
        }
    }

    #[test]
    fn test_find_replacements_literal_spans() {
        let op = Op::Replace {
            multiline: false,
            find: "ab".to_string(),
            replace: "X".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let spans = m.find_replacements("ab--ab", "X");
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (0, 2));
        assert_eq!((spans[1].start, spans[1].end), (4, 6));
        assert_eq!(spans[0].replacement, "X");
    }

    #[test]
    fn test_find_replacements_regex_capture_expansion() {
        let op = Op::Replace {
            multiline: false,
            find: r"(\d+)-(\d+)".to_string(),
            replace: "$2-$1".to_string(),
            regex: true,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let spans = m.find_replacements("1-2 and 3-4", "$2-$1");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].replacement, "2-1");
        assert_eq!(spans[1].replacement, "4-3");
    }

    #[test]
    fn test_find_replacements_across_newlines() {
        let op = Op::Replace {
            multiline: true,
            find: "a\nb".to_string(),
            replace: "ab".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        let spans = m.find_replacements("x\na\nb\ny", "ab");
        assert_eq!(spans.len(), 1);
        assert_eq!((spans[0].start, spans[0].end), (2, 5));
    }

    #[test]
    fn test_find_replacements_no_match_is_empty() {
        let op = Op::Replace {
            multiline: false,
            find: "zzz".to_string(),
            replace: "x".to_string(),
            regex: false,
            case_insensitive: false,
        };
        let m = Matcher::new(&op).unwrap();
        assert!(m.find_replacements("abc", "x").is_empty());
    }
}
