use serde::{Deserialize, Serialize};

/// Text transformation modes for the Transform operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransformMode {
    Upper,
    Lower,
    Title,
    SnakeCase,
    CamelCase,
}

impl std::fmt::Display for TransformMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformMode::Upper => write!(f, "upper"),
            TransformMode::Lower => write!(f, "lower"),
            TransformMode::Title => write!(f, "title"),
            TransformMode::SnakeCase => write!(f, "snake_case"),
            TransformMode::CamelCase => write!(f, "camel_case"),
        }
    }
}

impl std::str::FromStr for TransformMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "upper" => Ok(TransformMode::Upper),
            "lower" => Ok(TransformMode::Lower),
            "title" => Ok(TransformMode::Title),
            "snake_case" | "snake" => Ok(TransformMode::SnakeCase),
            "camel_case" | "camel" => Ok(TransformMode::CamelCase),
            _ => Err(format!(
                "unknown transform mode '{s}'. Valid modes: upper, lower, title, snake_case, camel_case"
            )),
        }
    }
}

/// The intermediate representation for all ripsed operations.
/// Both CLI args and JSON requests are normalized into this form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Op {
    Replace {
        find: String,
        replace: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
        /// Match against the whole buffer instead of line-by-line, allowing
        /// patterns to span line boundaries (like ripgrep's `-U`).
        #[serde(default)]
        multiline: bool,
    },
    Delete {
        find: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
        /// Match against the whole buffer; deletes the matched span rather
        /// than whole lines (like ripgrep's `-U`).
        #[serde(default)]
        multiline: bool,
    },
    InsertAfter {
        find: String,
        content: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
    InsertBefore {
        find: String,
        content: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
    ReplaceLine {
        find: String,
        content: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
    Transform {
        find: String,
        mode: TransformMode,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
    Surround {
        find: String,
        prefix: String,
        suffix: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
    Indent {
        find: String,
        #[serde(default = "default_indent_amount")]
        amount: usize,
        #[serde(default)]
        use_tabs: bool,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
    Dedent {
        find: String,
        #[serde(default = "default_indent_amount")]
        amount: usize,
        #[serde(default)]
        use_tabs: bool,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_insensitive: bool,
    },
}

fn default_indent_amount() -> usize {
    4
}

/// Options that control how operations are applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpOptions {
    #[serde(default = "default_true")]
    pub dry_run: bool,
    pub root: Option<String>,
    #[serde(default = "default_true")]
    pub gitignore: bool,
    #[serde(default)]
    pub backup: bool,
    #[serde(default)]
    pub atomic: bool,
    pub glob: Option<String>,
    pub ignore: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    pub max_depth: Option<usize>,
    pub line_range: Option<LineRange>,
}

impl Default for OpOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            root: None,
            gitignore: true,
            backup: false,
            atomic: false,
            glob: None,
            ignore: None,
            hidden: false,
            max_depth: None,
            line_range: None,
        }
    }
}

/// A range of lines to operate on (1-indexed, inclusive).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: Option<usize>,
}

impl LineRange {
    pub fn contains(&self, line: usize) -> bool {
        line >= self.start && self.end.is_none_or(|end| line <= end)
    }
}

use crate::default_true;

impl Op {
    /// Extract the find pattern from the operation.
    pub fn find_pattern(&self) -> &str {
        match self {
            Op::Replace { find, .. }
            | Op::Delete { find, .. }
            | Op::InsertAfter { find, .. }
            | Op::InsertBefore { find, .. }
            | Op::ReplaceLine { find, .. }
            | Op::Transform { find, .. }
            | Op::Surround { find, .. }
            | Op::Indent { find, .. }
            | Op::Dedent { find, .. } => find,
        }
    }

    /// Whether this operation matches against the whole buffer (allowing
    /// patterns to span line boundaries) instead of line-by-line.
    ///
    /// Only `Replace` and `Delete` support multiline matching; every other
    /// operation is inherently line-scoped and always returns `false`.
    pub fn is_multiline(&self) -> bool {
        match self {
            Op::Replace { multiline, .. } | Op::Delete { multiline, .. } => *multiline,
            _ => false,
        }
    }

    pub fn is_regex(&self) -> bool {
        match self {
            Op::Replace { regex, .. }
            | Op::Delete { regex, .. }
            | Op::InsertAfter { regex, .. }
            | Op::InsertBefore { regex, .. }
            | Op::ReplaceLine { regex, .. }
            | Op::Transform { regex, .. }
            | Op::Surround { regex, .. }
            | Op::Indent { regex, .. }
            | Op::Dedent { regex, .. } => *regex,
        }
    }

    pub fn is_case_insensitive(&self) -> bool {
        match self {
            Op::Replace {
                case_insensitive, ..
            }
            | Op::Delete {
                case_insensitive, ..
            }
            | Op::InsertAfter {
                case_insensitive, ..
            }
            | Op::InsertBefore {
                case_insensitive, ..
            }
            | Op::ReplaceLine {
                case_insensitive, ..
            }
            | Op::Transform {
                case_insensitive, ..
            }
            | Op::Surround {
                case_insensitive, ..
            }
            | Op::Indent {
                case_insensitive, ..
            }
            | Op::Dedent {
                case_insensitive, ..
            } => *case_insensitive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Op serde roundtrip ──

    /// Locks in the protocol wire-format: the `op` tag MUST serialize as
    /// `"replace"` (snake_case), not `"Replace"`. Agents that depend on
    /// this wire format would silently break if the tag rename were
    /// removed. Only one such test per variant class — the rest are
    /// pure serde-framework roundtrips with no added value.
    #[test]
    fn replace_op_tag_wire_format() {
        let op = Op::Replace {
            multiline: false,
            find: "foo".into(),
            replace: "bar".into(),
            regex: false,
            case_insensitive: false,
        };
        let json = serde_json::to_value(&op).unwrap();
        assert_eq!(json["op"], "replace");
        assert_eq!(json["find"], "foo");
        assert_eq!(json["replace"], "bar");
    }

    #[test]
    fn multiline_field_defaults_to_false_and_roundtrips() {
        // Wire format: omitted -> false (back-compat with pre-multiline requests).
        let op: Op =
            serde_json::from_str(r#"{"op": "replace", "find": "a", "replace": "b"}"#).unwrap();
        assert!(!op.is_multiline());

        let op: Op = serde_json::from_str(
            r#"{"op": "replace", "find": "a", "replace": "b", "multiline": true}"#,
        )
        .unwrap();
        assert!(op.is_multiline());

        let op: Op =
            serde_json::from_str(r#"{"op": "delete", "find": "a", "multiline": true}"#).unwrap();
        assert!(op.is_multiline());
    }

    #[test]
    fn is_multiline_false_for_line_scoped_ops() {
        // Line-scoped ops can't express multiline at the type level.
        let op = Op::InsertAfter {
            find: "a".into(),
            content: "b".into(),
            regex: false,
            case_insensitive: false,
        };
        assert!(!op.is_multiline());
    }

    #[test]
    fn deserialize_with_default_booleans() {
        let json = r#"{"op": "replace", "find": "a", "replace": "b"}"#;
        let op: Op = serde_json::from_str(json).unwrap();
        assert!(!op.is_regex());
        assert!(!op.is_case_insensitive());
    }

    #[test]
    fn unknown_op_tag_fails_deserialization() {
        let json = r#"{"op": "transform", "find": "a"}"#;
        let result = serde_json::from_str::<Op>(json);
        assert!(result.is_err());
    }

    // ── LineRange ──

    #[test]
    fn line_range_contains_bounded() {
        let range = LineRange {
            start: 5,
            end: Some(10),
        };
        assert!(!range.contains(4));
        assert!(range.contains(5));
        assert!(range.contains(7));
        assert!(range.contains(10));
        assert!(!range.contains(11));
    }

    #[test]
    fn line_range_contains_unbounded_end() {
        let range = LineRange {
            start: 3,
            end: None,
        };
        assert!(!range.contains(2));
        assert!(range.contains(3));
        assert!(range.contains(1000));
    }

    #[test]
    fn line_range_single_line() {
        let range = LineRange {
            start: 7,
            end: Some(7),
        };
        assert!(!range.contains(6));
        assert!(range.contains(7));
        assert!(!range.contains(8));
    }

    // ── OpOptions ──

    #[test]
    fn op_options_default_values() {
        let opts = OpOptions::default();
        assert!(opts.dry_run);
        assert!(opts.gitignore);
        assert!(!opts.backup);
        assert!(!opts.atomic);
        assert!(!opts.hidden);
        assert!(opts.root.is_none());
        assert!(opts.glob.is_none());
        assert!(opts.ignore.is_none());
        assert!(opts.max_depth.is_none());
        assert!(opts.line_range.is_none());
    }

    #[test]
    fn op_options_deserializes_with_defaults() {
        let json = "{}";
        let opts: OpOptions = serde_json::from_str(json).unwrap();
        assert!(opts.dry_run);
        assert!(opts.gitignore);
    }

    #[test]
    fn op_options_overrides_defaults() {
        let json = r#"{"dry_run": false, "gitignore": false, "backup": true}"#;
        let opts: OpOptions = serde_json::from_str(json).unwrap();
        assert!(!opts.dry_run);
        assert!(!opts.gitignore);
        assert!(opts.backup);
    }

    // ── Serde defaults and unknown-variant behavior ──

    /// Protocol: `transform` with no `mode` must be rejected, not defaulted.
    #[test]
    fn transform_missing_mode_fails() {
        let json = r#"{"op": "transform", "find": "a"}"#;
        let result = serde_json::from_str::<Op>(json);
        assert!(result.is_err());
    }

    /// Protocol: `indent` amount defaults to 4 when omitted.
    #[test]
    fn indent_amount_defaults_to_four() {
        let json = r#"{"op": "indent", "find": "x"}"#;
        let op: Op = serde_json::from_str(json).unwrap();
        match op {
            Op::Indent {
                amount, use_tabs, ..
            } => {
                assert_eq!(amount, 4);
                assert!(!use_tabs);
            }
            _ => panic!("Expected Indent variant"),
        }
    }

    /// Protocol: `dedent` amount defaults to 4 when omitted.
    #[test]
    fn dedent_amount_defaults_to_four() {
        let json = r#"{"op": "dedent", "find": "x"}"#;
        let op: Op = serde_json::from_str(json).unwrap();
        match op {
            Op::Dedent { amount, .. } => {
                assert_eq!(amount, 4);
            }
            _ => panic!("Expected Dedent variant"),
        }
    }

    /// Protocol wire format: transform mode names serialize to the
    /// snake_case forms agents expect. Locks in the API contract.
    #[test]
    fn transform_mode_wire_names() {
        let op = Op::Transform {
            find: "hello".into(),
            mode: TransformMode::SnakeCase,
            regex: true,
            case_insensitive: false,
        };
        let json = serde_json::to_value(&op).unwrap();
        assert_eq!(json["op"], "transform");
        assert_eq!(json["mode"], "snake_case");
    }

    // ── TransformMode Display and FromStr ──

    #[test]
    fn transform_mode_display_roundtrip() {
        let modes = [
            TransformMode::Upper,
            TransformMode::Lower,
            TransformMode::Title,
            TransformMode::SnakeCase,
            TransformMode::CamelCase,
        ];
        for mode in modes {
            let s = mode.to_string();
            let parsed: TransformMode = s.parse().unwrap();
            assert_eq!(mode, parsed);
        }
    }

    #[test]
    fn transform_mode_from_str_aliases() {
        assert_eq!(
            "snake".parse::<TransformMode>().unwrap(),
            TransformMode::SnakeCase
        );
        assert_eq!(
            "camel".parse::<TransformMode>().unwrap(),
            TransformMode::CamelCase
        );
    }

    #[test]
    fn transform_mode_from_str_unknown_fails() {
        assert!("unknown".parse::<TransformMode>().is_err());
    }
}
