# ripsed

A fast, modern stream editor built in Rust. Like [ripgrep](https://github.com/BurntSushi/ripgrep) is to grep, ripsed is to sed.

Designed for humans **and** machines — with first-class JSON support for AI coding agents.

## Features

- **Sensible defaults.** Recursive, `.gitignore`-aware, UTF-8 plus
  BOM-detected UTF-16 (files keep their encoding and BOM across edits).
  No flags needed for the common case.
- **No escape hell.** Standard Rust regex syntax. No sed-style delimiters.
- **Agent-native.** Structured JSON I/O as a first-class interface, not an afterthought.
- **Safe by default.** Dry-run previews, atomic writes, undo log, backup files.
- **Fast.** Parallel discovery *and* application, memory-mapped I/O,
  whole-buffer fast-reject — same philosophy as ripgrep. Numbers and
  methodology in [BENCHMARKS.md](BENCHMARKS.md).
- **Scriptable.** Chain operations in `.rip` script files for multi-step refactors.

## Installation

### From crates.io

```bash
cargo install ripsed-cli   # installs the `ripsed` binary
```

### Prebuilt binaries

Every [GitHub release](https://github.com/dollspace-gay/ripsed/releases)
ships binaries for Linux (x86_64, aarch64, plus a `.deb`), macOS
(x86_64, aarch64), and Windows (x86_64), each with shell completions, a
man page, and SHA256 checksums. Homebrew/Scoop/AUR templates live in
[`packaging/`](packaging/).

### From source

```bash
cargo install --path crates/ripsed-cli
```

Requires Rust 1.93+.

## Quick Start

```bash
# Find-and-replace across all files (recursive, respects .gitignore)
ripsed 'old_function' 'new_function'

# Regex with capture groups
ripsed -e 'fn\s+old_(\w+)' 'fn new_$1'

# Scope to specific files
ripsed 'TODO' 'DONE' --glob '*.rs'

# Delete lines matching a pattern
ripsed -d 'console\.log'

# Multiline: match across line boundaries (like rg -U)
ripsed -U -e 'fn old\(\n\s*x: u32,\n\)' 'fn new(x: u32)'

# Replace only the first occurrence per line (sed s/// without /g)
ripsed --first 'foo' 'bar'

# Cap total replacements per file
ripsed --max-replacements 5 'foo' 'bar'

# Only operate between pattern-matched lines (like sed /start/,/end/)
ripsed --range '/\[dependencies\]/,/^$/' 'old-crate' 'new-crate'

# Insert text after matching lines
ripsed 'use serde;' --after 'use serde_json;'

# Transform matched text (upper, lower, title, snake_case, camel_case)
ripsed --transform upper 'select|from|where' -e

# Surround matching lines with prefix/suffix
ripsed --surround '/* ' ' */' 'HACK'

# Indent/dedent matching lines
ripsed --indent 4 'nested_block'
ripsed --dedent 2 'over_indented'

# Run a multi-step refactor from a .rip script
ripsed --script refactor.rip

# Preview changes without applying
ripsed 'foo' 'bar' --dry-run

# Pipe mode (stdin/stdout, like traditional sed)
echo 'hello world' | ripsed 'hello' 'goodbye'
```

## CLI Reference

```
USAGE:
    ripsed [OPTIONS] <FIND> [REPLACE]

ARGS:
    <FIND>       Pattern to search for (literal by default, regex with -e)
    [REPLACE]    Replacement string

OPTIONS:
    -e, --regex              Treat FIND as a regex
    -U, --multiline          Match across line boundaries (replace/delete only)
        --first              Replace only the first occurrence per line
        --first-in-file      Replace only the first occurrence in each file
        --max-replacements <N>  Replace at most N occurrences per file
    -d, --delete             Delete matching lines
        --dry-run            Preview changes without writing
        --backup             Create .ripsed.bak files before modifying
        --glob <PATTERN>     Only process files matching glob
        --ignore <PATTERN>   Skip files matching glob
        --hidden             Include hidden files
        --no-gitignore       Don't respect .gitignore
        --case-insensitive   Case-insensitive matching
        --after <TEXT>       Insert text after matching lines
        --before <TEXT>      Insert text before matching lines
        --replace-line <TEXT> Replace entire matching line
    -n, --line-range <N:M>   Only operate on lines N through M
        --range </S/,/E/>    Only operate between pattern-matched lines
                             (regex; regions inclusive, sed semantics)
        --max-depth <N>      Maximum directory recursion depth
    -c, --count              Print count of matches only
    -q, --quiet              Suppress all non-error output
        --confirm            Interactive confirmation before each file
        --undo [N]           Undo the last N operations (default: 1)
        --undo-list          Show recent undo log entries
        --no-undo            Don't record undo entries for this run
        --follow             Follow symbolic links during discovery
        --config <PATH>      Path to .ripsed.toml config file
        --transform <MODE>   Transform matched text (upper, lower, title, snake_case, camel_case)
        --surround <P> <S>   Surround matching lines with prefix and suffix
        --indent <N>         Indent matching lines by N spaces
        --dedent <N>         Remove up to N leading whitespace chars from matching lines
        --script <PATH>      Run operations from a .rip script file
    -j, --json               Enable agent/JSON mode
        --jsonl              Stream results as JSON Lines
        --no-json            Force human mode even if stdin looks like JSON
```

## How does this compare to sed and sd?

| | GNU sed | [sd](https://github.com/chmln/sd) | ripsed |
|---|---|---|---|
| Recursive file discovery | — (`find`/`xargs`) | — (`find`/`xargs`) | built-in, parallel |
| `.gitignore`-aware | — | — | ✓ |
| Literal-by-default patterns | — | partial (regex default) | ✓ |
| Undo log | — | — | ✓ |
| Atomic writes + file locking | — | — | ✓ |
| Dry-run preview with diff | — | `--preview` | ✓ |
| Multiline matching | scripting (`N;P;D`) | ✓ | ✓ (`-U`) |
| Pattern-addressed regions | ✓ (`/a/,/b/`) | — | ✓ (`--range`) |
| Hold space / full sed programs | ✓ | — | — |
| Structured JSON interface | — | — | ✓ |
| UTF-16 (BOM) editing | — | — | ✓ |
| Huge single files | **fastest** | fast | slower today ([why](BENCHMARKS.md)) |

In short: `sed` remains unbeatable for stream-programming and giant
single files; `sd` is a lean find/replace on explicit file lists;
ripsed is for **project-wide refactors** — discovery, safety rails
(dry-run, undo, atomic writes), and an agent-native JSON protocol.
Honest performance numbers, including the cases ripsed loses, live in
[BENCHMARKS.md](BENCHMARKS.md). There's also a task-oriented
[GUIDE.md](GUIDE.md) and a [FAQ](FAQ.md).

## Exit Codes

ripsed follows the ripgrep convention:

| Code | Meaning |
|---|---|
| 0 | Ran and made (or previewed) changes |
| 1 | Ran cleanly, but nothing matched |
| 2 | An error occurred (bad regex, IO failure, invalid request) |

Errors take precedence: a run with per-file errors exits 2 even if
other files were changed.

## Agent / JSON Mode

ripsed has a structured JSON interface designed for AI coding agents, editor plugins, and automation pipelines. In agent mode, `dry_run` defaults to `true` for safety.

### Request

```bash
ripsed --json << 'EOF'
{
  "version": "1",
  "operations": [
    {
      "op": "replace",
      "find": "old_function",
      "replace": "new_function",
      "glob": "src/**/*.rs"
    },
    {
      "op": "delete",
      "find": "^\\s*//\\s*TODO:.*$",
      "regex": true
    }
  ],
  "options": {
    "dry_run": true,
    "root": "./my-project"
  }
}
EOF
```

### Response

```json
{
  "version": "1",
  "success": true,
  "dry_run": true,
  "summary": {
    "files_matched": 12,
    "files_modified": 0,
    "total_replacements": 34
  },
  "results": [
    {
      "operation_index": 0,
      "files": [
        {
          "path": "src/lib.rs",
          "changes": [
            {
              "line": 42,
              "before": "    let result = old_function(x);",
              "after": "    let result = new_function(x);",
              "context": {
                "before": ["fn main() {", "    let x = 5;"],
                "after": ["    println!(\"{}\", result);", "}"]
              }
            }
          ]
        }
      ]
    }
  ],
  "errors": []
}
```

### Operations

| Operation | JSON `op` | Human flag | Description |
|---|---|---|---|
| Replace | `replace` | `ripsed 'find' 'replace'` | Find and replace text |
| Delete | `delete` | `-d` | Remove lines matching pattern |
| Insert after | `insert_after` | `--after` | Insert text after matching lines |
| Insert before | `insert_before` | `--before` | Insert text before matching lines |
| Replace line | `replace_line` | `--replace-line` | Replace entire matching line |
| Transform | `transform` | `--transform MODE` | Change case of matched text |
| Surround | `surround` | `--surround P S` | Wrap matching lines with prefix/suffix |
| Indent | `indent` | `--indent N` | Add N spaces before matching lines |
| Dedent | `dedent` | `--dedent N` | Remove up to N leading whitespace chars from matching lines |

Replace and delete additionally accept `"multiline": true` (CLI: `-U`) to
match against the whole file instead of line-by-line, allowing patterns to
span line boundaries. In multiline mode, delete removes the matched span
rather than whole lines, and line ranges are not supported.

Options accept `"range": {"start_pattern": "...", "end_pattern": "..."}` to
scope operations to pattern-addressed regions (sed's `/start/,/end/`):
regions open on a line matching the start regex, close on the next line
matching the end regex (boundaries inclusive, unclosed regions run to EOF),
and multiple regions are supported. Mutually exclusive with `line_range`.

Replace also accepts `"count"` to limit how many occurrences are replaced:
`"all"` (default), `"first_per_line"` (CLI: `--first`), `"first_in_file"`
(CLI: `--first-in-file`), or `{"max": n}` (CLI: `--max-replacements N`,
counting occurrences per file). `first_per_line` cannot be combined with
multiline mode.

### Error Handling

Every error includes a machine-readable `code`, human-readable `message`, and actionable `hint`:

| Code | Description |
|---|---|
| `no_matches` | Pattern matched nothing |
| `invalid_regex` | Regex failed to compile |
| `invalid_request` | Malformed JSON or missing fields |
| `file_not_found` | Target path doesn't exist |
| `permission_denied` | Can't read/write target files |
| `binary_file_skipped` | Binary file was skipped |
| `write_failed` | Could not write output file |

## Script Files

Chain multiple operations in a `.rip` file:

```bash
# refactor.rip — rename and clean up
replace "oldApi" "newApi" --glob "*.ts"
replace "OldApi" "NewApi" --glob "*.ts"
delete "// DEPRECATED" -e
transform "select|from|where|join" --mode upper -e --glob "*.sql"
```

```bash
ripsed --script refactor.rip --dry-run   # preview
ripsed --script refactor.rip             # apply
```

Each line is an operation with the same flags as the CLI. Comments start with `#`. Strings with spaces use quotes (single or double, with escape support).

## Configuration

Create a `.ripsed.toml` in your project root:

```toml
[defaults]
backup = true
max_depth = 10

[undo]
max_entries = 100
# Files larger than this get no undo entry (the log stores a full copy
# of the original text). 0 = unlimited. Default: 4 MiB.
max_file_bytes = 4194304

[defaults]
# Files at least this large stream straight to the output in constant
# memory (sed-style) when no undo entry will be recorded — this is how
# files larger than RAM stay editable. 0 disables. Default: 256 MiB.
stream_min_bytes = 268435456
```

ripsed discovers this file by walking up from the current directory, similar to `.gitignore`.

## Architecture

ripsed is organized as a Rust workspace with four crates:

| Crate | Description |
|---|---|
| `ripsed-core` | Pure logic: edit engine, matcher, operation IR, error taxonomy |
| `ripsed-fs` | File I/O: discovery, reading (with mmap), atomic writes, locking |
| `ripsed-json` | Agent interface: request/response schemas, auto-detection |
| `ripsed-cli` | Binary: CLI args, human output formatting, interactive confirm |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for
the dev setup, workspace map, and PR checklist.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
