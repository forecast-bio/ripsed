# ripsed Guide

Task-oriented walkthroughs. The [README](README.md) covers the flag
reference; this covers how to actually work with the tool.

## Bulk rename across a repository

```bash
# Preview first — dry-run shows the diff without writing
ripsed --dry-run 'OldServiceName' 'NewServiceName'

# Looks right? Apply (recursive, .gitignore-aware, parallel)
ripsed 'OldServiceName' 'NewServiceName'

# Something off? Every modified file has an undo entry
ripsed --undo-list
ripsed --undo 17        # restore the last 17 modified files
```

Scope with globs when the identifier is ambiguous:

```bash
ripsed 'OldServiceName' 'NewServiceName' --glob '*.rs' --ignore 'vendor/**'
```

## Renames that need a regex

Capture groups use `$1`-style references:

```bash
ripsed -e 'fn\s+legacy_(\w+)' 'fn modern_$1' --glob '*.rs'
```

Two things to know:

- **Replacement strings are literal apart from `$refs`.** `\n` in a
  replacement is a backslash and an `n`, not a newline. To insert real
  newlines from a shell, pass a real newline (`$'\n'` in bash) or use a
  `.rip` script, where `"\n"` inside double quotes *is* an escape.
- **Find patterns are literal by default.** `ripsed '$10.00' '$12.50'`
  just works; add `-e` only when you want regex.

## Multi-line refactors

`-U` matches across line boundaries (like ripgrep's `-U`):

```bash
# Collapse a two-line signature
ripsed -U -e 'fn old\(\n\s*x: u32,\n\)' 'fn new(x: u32)'

# Delete a marked block (the matched span, not whole lines)
ripsed -d -U -e '// BEGIN GENERATED.*?// END GENERATED' --glob '*.rs'
```

Multiline mode needs the whole file in memory and is incompatible with
line ranges and `--first`.

## Replacing only some occurrences

```bash
ripsed --first 'foo' 'bar'              # first occurrence per line (sed s/// without /g)
ripsed --first-in-file 'foo' 'bar'      # one replacement per file
ripsed --max-replacements 3 'foo' 'bar' # at most 3 occurrences per file
```

## Operating inside a region

Numeric ranges (`-n 10:20`) or sed-style pattern addressing:

```bash
# Only between the [dependencies] header and the next blank line
ripsed --range '/\[dependencies\]/,/^$/' 'old-crate' 'new-crate' --glob 'Cargo.toml'
```

Regions are inclusive of both boundary lines, `/a/,/a/` runs to the
*next* `a`, and an unclosed region extends to end of file.

## Multi-step refactors with .rip scripts

```bash
# refactor.rip
replace "oldApi" "newApi" --glob "*.ts"
replace "OldApi" "NewApi" --glob "*.ts"
delete -e "^\s*//\s*DEPRECATED" --glob "*.ts"
replace -U "import a;\nimport b;" "import ab;" --glob "*.ts"
```

```bash
ripsed --script refactor.rip --dry-run   # preview every step
ripsed --script refactor.rip             # apply
```

Operations run in order; each sees the previous one's output. Inside
double-quoted script strings, `\n` and `\t` are real escapes.

## Driving ripsed from an agent or program

Send a JSON request on stdin; `dry_run` defaults to **true** in JSON
mode, so nothing writes until you say so:

```bash
ripsed --json <<'EOF'
{
  "version": "1",
  "operations": [
    {"op": "replace", "find": "old_name", "replace": "new_name", "glob": "src/**/*.rs"},
    {"op": "delete", "find": "^\\s*//\\s*TODO:", "regex": true}
  ],
  "options": {"dry_run": false, "root": "./my-project", "atomic": true}
}
EOF
```

- `"atomic": true` makes the whole batch all-or-nothing.
- `--jsonl` streams one result line per file for progress display.
- Undo: `{"undo": {"last": 3}}` as a separate request.
- Exit codes: `0` changed, `1` nothing matched, `2` error — errors are
  also structured in the response body with `code`/`message`/`hint`.

## In a pipeline

```bash
tail -f app.log | ripsed 'ERROR' '🔥 ERROR'   # streams line-by-line, constant memory
ripsed --pipe 'foo' 'bar' < in.txt > out.txt
```

Piped input streams (mixed CRLF/LF passes through byte-exact); `| head`
terminates ripsed quietly, like any well-behaved filter.

## Undo, backups, and safety

- Every non-dry-run modification is recorded in `.ripsed/undo.jsonl`
  (0600 permissions; entry count capped by `[undo] max_entries`, and
  files over `[undo] max_file_bytes` — default 4 MiB — are edited but
  not recorded, with a stderr note). `--no-undo` skips recording for
  bulk runs where the log would just be ballast.
- `--backup` additionally writes `.ripsed.bak` files before modifying.
- Writes are atomic (temp file + rename) and per-file locked, so
  concurrent ripsed runs can't corrupt files.
- UTF-16 files (with BOM) are edited in their own encoding and undo
  restores their original bytes exactly.
