# FAQ

## Why is FIND literal by default? ripgrep defaults to regex.

Because the dominant use case for an *editor* is pasting an exact
identifier or string from code, where `$`, `.`, `(` are common and
regex-escaping them is exactly the sed pain ripsed exists to remove.
Searching (rg) skews toward patterns; replacing skews toward literals.
Add `-e` when you want regex.

## Why doesn't `\n` in my replacement insert a newline?

Replacement strings on the CLI are literal apart from `$1`-style
capture references — `\n` is two characters. Pass a real newline from
your shell (`$'\n'` in bash) or use a `.rip` script, where `"\n"`
inside double quotes is interpreted. (Find patterns with `-e` *do*
understand `\n`, because the regex engine interprets it.)

## What do the exit codes mean?

ripgrep's convention: `0` = made (or previewed) changes, `1` = ran
cleanly but nothing matched, `2` = error. Errors win over partial
success — if any file errored, the run exits `2` even when other files
were changed.

## What are these `*.ripsed.lock` files?

Advisory-lock sentinels. Mutual exclusion between concurrent ripsed
processes uses kernel file locks (`flock`/`LockFileEx`) on a sentinel
that is deliberately **never deleted** — removing it would let another
process lock a different inode at the same path and race. They're
empty-ish, harmless, and safe to gitignore (`*.ripsed.lock`).

## How does binary/encoding detection work?

A file is skipped as binary if its first 8 KB contains a NUL byte —
unless it starts with a UTF-16 byte-order mark, in which case it's
decoded, edited, and re-encoded byte-compatibly (BOM preserved). UTF-8
BOMs are stripped before matching (so `^` anchors work on line 1) and
re-attached on write. UTF-16 *without* a BOM and other encodings
(Latin-1, Shift-JIS, …) are out of scope: undetectable cheaply, and
guessing wrong corrupts files.

## Where does undo state live, and is it safe?

`.ripsed/undo.jsonl` in the directory you ran from — written 0600
because it contains pre-edit file contents, entry count capped via
`.ripsed.toml`. Undo restores original bytes exactly, including
encoding and BOM. Note: undo stores the *full* original text per
modified file, which is why editing very large files is comparatively
slow today (see BENCHMARKS.md).

## Is ripsed fast?

For tree-wide refactors with discovery, yes — and when most files
don't match, the whole-buffer prescreen rejects them at substring-search
speed. For a single multi-hundred-MB file, GNU sed is currently faster
(measured honestly in [BENCHMARKS.md](BENCHMARKS.md)). Use the right
tool.

## Why both `ripsed` and `ripsed-cli` on crates.io?

`ripsed-cli` is the binary (`cargo install ripsed-cli` puts `ripsed`
on your PATH). `ripsed` is the library facade for embedding the engine
(`apply_to_file`, `apply_to_files`) in your own tools; `ripsed-core`,
`ripsed-fs`, and `ripsed-json` are its layers.

## Does `--confirm` work in scripts/CI?

No — it prompts on a TTY between preview and write, so it's
interactive by definition (and forces single-threaded processing).
For automation, use `--dry-run` to inspect and JSON mode's
`dry_run`/`atomic` options to gate writes.
