use ripsed_core::diff::Change;
use std::io::{self, Write};
use std::path::Path;

/// Actions the user can take when confirming a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Apply this change.
    Yes,
    /// Skip this change.
    No,
    /// Apply all remaining changes without further prompts.
    ApplyAll,
    /// Skip all remaining changes in the current file.
    SkipFile,
    /// Abort the entire operation immediately.
    Quit,
}

/// Prompt the user to confirm all changes in a file.
/// Shows a preview of each change, then asks once whether to apply.
///
/// Accepted inputs:
///   y / yes  -> Yes
///   n / no   -> No  (default on empty input)
///   a / all  -> ApplyAll (apply this file and all remaining without prompts)
///   s / skip -> SkipFile
///   q / quit -> Quit
pub fn confirm_file(path: &Path, changes: &[Change]) -> ConfirmAction {
    let bold = anstyle::Style::new().bold();
    let red = anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red)));
    let green =
        anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green)));
    let reset = anstyle::Reset;

    eprintln!(
        "\n{bold}{}{reset} ({} change{}):",
        path.display(),
        changes.len(),
        if changes.len() == 1 { "" } else { "s" }
    );
    for change in changes {
        eprintln!("  line {}:", change.line);
        eprintln!("  {red}- {}{reset}", change.before);
        if let Some(ref after) = change.after {
            eprintln!("  {green}+ {after}{reset}");
        }
    }
    eprint!("Apply changes to this file? [y/n/a/s/q] ");
    io::stderr().flush().ok();

    let mut response = String::new();
    if io::stdin().read_line(&mut response).is_ok() {
        parse_confirm_response(&response)
    } else {
        ConfirmAction::Quit
    }
}

/// Map a raw prompt response to a [`ConfirmAction`].
///
/// Matching is case-insensitive and ignores surrounding whitespace.
/// Anything unrecognized — including an empty line or EOF — is `No`,
/// so the safe choice is always the default.
fn parse_confirm_response(response: &str) -> ConfirmAction {
    match response.trim().to_lowercase().as_str() {
        "y" | "yes" => ConfirmAction::Yes,
        "a" | "all" => ConfirmAction::ApplyAll,
        "s" | "skip" => ConfirmAction::SkipFile,
        "q" | "quit" => ConfirmAction::Quit,
        _ => ConfirmAction::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yes_variants_map_to_yes() {
        for input in ["y", "yes", "Y", "YES", "Yes", "  y  ", "y\n", "yes\r\n"] {
            assert_eq!(
                parse_confirm_response(input),
                ConfirmAction::Yes,
                "input {input:?} should be Yes"
            );
        }
    }

    #[test]
    fn all_variants_map_to_apply_all() {
        for input in ["a", "all", "A", "ALL", "all\n"] {
            assert_eq!(parse_confirm_response(input), ConfirmAction::ApplyAll);
        }
    }

    #[test]
    fn skip_variants_map_to_skip_file() {
        for input in ["s", "skip", "S", "SKIP", "skip\n"] {
            assert_eq!(parse_confirm_response(input), ConfirmAction::SkipFile);
        }
    }

    #[test]
    fn quit_variants_map_to_quit() {
        for input in ["q", "quit", "Q", "QUIT", "quit\n"] {
            assert_eq!(parse_confirm_response(input), ConfirmAction::Quit);
        }
    }

    #[test]
    fn explicit_no_maps_to_no() {
        for input in ["n", "no", "N", "NO", "no\n"] {
            assert_eq!(parse_confirm_response(input), ConfirmAction::No);
        }
    }

    #[test]
    fn empty_input_defaults_to_no() {
        // read_line yields "" at EOF and "\n" for a bare Enter — both must
        // default to the safe answer, not apply changes.
        for input in ["", "\n", "\r\n", "   ", "\t\n"] {
            assert_eq!(
                parse_confirm_response(input),
                ConfirmAction::No,
                "input {input:?} should default to No"
            );
        }
    }

    #[test]
    fn unrecognized_input_defaults_to_no() {
        for input in ["x", "yep", "nah", "quit now", "ja", "1", "true", "ye s"] {
            assert_eq!(
                parse_confirm_response(input),
                ConfirmAction::No,
                "input {input:?} should default to No"
            );
        }
    }
}
