# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Undo-log size cap and opt-out (#106): files larger than
  `undo.max_file_bytes` (`.ripsed.toml`, default 4 MiB, `0` =
  unlimited) are edited but get no undo entry, with a one-line stderr
  note — the log stores a full copy of each original, which made
  huge-file edits pay a second full serialization. `--no-undo`
  (JSON: `"record_undo": false`) skips recording entirely. Measured
  on the 64 MiB benchmark corpus, the default cap trims roughly a
  second off the mean; the remaining gap to sed is the line-oriented
  engine, not undo.

## [0.3.0] - 2026-06-09

### Changed (BREAKING)
- **Exit codes now follow the ripgrep convention** (#100): `0` = made
  or previewed changes, `1` = ran cleanly but nothing matched, `2` =
  error (bad regex, IO failure, invalid request, lock timeout).
  Previously every failure — including real errors — exited `1`.
  Scripts checking `!= 0` keep working; scripts that treated `1` as
  "error" must now check for `2`. Errors take precedence over partial
  success: a run with per-file errors exits `2` even if other files
  changed. Pipe mode now exits `1` on clean passthrough with no
  matches (was `0`); JSON mode exits `1` on a successful response with
  zero matched files (the response body is unchanged).

### Added
- Community scaffolding (#105): CONTRIBUTING.md (dev setup, workspace
  map, PR checklist, the fuzz-workspace footgun), GitHub issue
  templates for bugs and feature requests, and a PR template.
- Documentation (#104): task-oriented GUIDE.md (bulk renames, regex
  captures, multiline refactors, replacement counts, pattern regions,
  .rip scripts, agent JSON integration, pipelines, undo/safety), a
  FAQ covering the questions the design actually raises (literal-by-
  default rationale, `\n` in replacements, exit codes, lock sentinels,
  encoding policy, undo internals), and an honest sed/sd comparison
  table in the README including where each tool wins.
- Fuzzing in CI and coverage tracking (#103): the four libfuzzer
  targets now run as a CI matrix job (60 s each per push/PR, 10
  minutes each on a weekly cron) with crash artifacts uploaded on
  failure and a committed seed corpus; a `cargo llvm-cov` job
  publishes lcov + summary as workflow artifacts.
- Distribution packaging (#102): crates.io metadata (keywords,
  categories, homepage) on all five crates and a real description for
  `ripsed-cli`; `cargo install ripsed-cli` documented as the primary
  install path in the README; Debian packages built in the release
  workflow via `cargo deb` (binary + man page + completions);
  Homebrew formula, Scoop manifest, and AUR PKGBUILD templates under
  `packaging/` with per-release update instructions.
- Shell completions and man page (#101): `cargo xtask gen-completions`
  generates bash/zsh/fish/powershell completions and `ripsed.1` from
  the real clap definition (so they can never drift from the actual
  flags); release artifacts ship them under `completions/` and CI
  verifies generation on every push.
- Comparative benchmark harness (#99): `cargo xtask bench-compare`
  generates corpora and measures ripsed against GNU sed, sd, and
  `perl -pi` via hyperfine, with a pristine corpus restored before
  every timing iteration. Methodology and honest results — including
  the losses — are published in BENCHMARKS.md: ripsed wins the
  no-match tree (prescreen), loses the 64 MiB single-file case ~6×
  (undo-log full-text copy; follow-up filed as #106).
- Whole-buffer prescreen fast-reject (#98): before the per-line loop,
  the engine checks whether the pattern can match anywhere in the
  buffer (literal `contains`; for regexes a `(?m)`-compiled shadow so
  `^`/`$` keep per-line semantics) and skips non-matching files
  outright. Patterns using `\A`/`\z` or flag-negating groups get no
  shadow and always proceed (soundness locked by a proptest: a
  prescreen reject implies no line matches). A 10k-line non-matching
  buffer drops from ~2.8 ms to ~10 µs in the engine benchmark.
- Parallel file application (#97): file mode and script mode fan files
  out across a rayon worker pool (one file per worker; the existing
  per-file advisory locks make this safe), with `--threads N` to
  control the pool size (default: all cores). Output ordering, undo
  recording, and exit codes are identical to a single-threaded run —
  outcomes are collected in discovery order and printed from the main
  thread. `--confirm` remains sequential (it prompts interactively).
  In script mode the per-file lock is now scoped to each
  (operation, file) pass rather than held across all operations.
- Streaming pipe mode (#96): piped input is now processed line by line
  in constant memory instead of buffering all of stdin, so arbitrarily
  large (or infinite) streams work like sed in a pipeline. Each line
  keeps its own terminator (mixed CRLF/LF streams pass through
  byte-exact — better than the buffered majority vote), `--count`,
  `-n`, `--range`, and the replacement-count flags all work on
  streams, and a closed downstream (`| head`) terminates quietly with
  success. Multiline (`-U`) operations still buffer (they need the
  whole input), as does anything that looks like a JSON request. New
  library API: `engine::LineProcessor` for incremental line
  processing.
- Encoding support (#95): BOM-based detection of UTF-8-with-BOM and
  UTF-16 LE/BE. UTF-16 files — previously skipped as binary because
  of their NUL bytes — are now discovered, edited, and written back
  in their original encoding with the BOM re-attached; UTF-8 BOMs are
  stripped before matching (so `^` anchors work on line 1) and
  restored on write. The undo log stores an encoding tag so undo
  restores original bytes exactly (older logs read fine; absent tag
  means UTF-8). Truncated UTF-16 and unpaired surrogates are clean
  per-file errors, never panics. No new dependencies — std covers
  both UTF-16 directions. UTF-16 without a BOM remains out of scope
  (indistinguishable from binary by a cheap check).
- Pattern-based line ranges (#94): CLI `--range '/start/,/end/'` and
  JSON `options.range` (`{"start_pattern", "end_pattern"}`) scope
  operations to sed-style pattern-addressed regions — regexes, regions
  open on a start match and close on the next end match (boundaries
  inclusive, `/a/,/a/` spans to the next `a`), multiple regions
  supported, unclosed regions run to EOF. Mutually exclusive with
  `line_range`/`-n` and multiline mode; pattern regexes are validated
  at parse time. Breaking for library users: `engine::apply` takes
  `Option<RangeSpec>` (wrapping `LineRange` or `PatternRange`) instead
  of `Option<LineRange>`, and `ApplyOptions.line_range` is now
  `ApplyOptions.range: Option<RangeSpec>`.
- Replacement count control (#93): `Op::Replace` gains a `count` field
  — `"all"` (default), `"first_per_line"` (sed `s///` without `/g`),
  `"first_in_file"`, or `{"max": n}` (occurrence cap per file). CLI:
  `--first`, `--first-in-file`, `--max-replacements N` (mutually
  exclusive, replace-only). `.rip` scripts accept the same flags.
  `first_per_line` is rejected in multiline mode; `{"max": 0}` is
  rejected. Breaking for library users: constructing `Op::Replace`
  literals now requires the `count` field; the JSON wire format is
  unaffected (optional, defaults to `"all"`).
- Multiline mode surface (#92): CLI `-U`/`--multiline` (conflicts with
  line-scoped flags via clap), JSON `"multiline": true` on replace and
  delete ops (rejected with `invalid_request` + `operation_index` on
  any other op type; explicit `false` tolerated), `.rip` script flag
  `--multiline`/`-U` (parse error on line-scoped ops), JSON Schema and
  README updated. The generated schema's `op` enum also now lists
  `transform`/`surround`/`indent`/`dedent`, which were missing.
- Multiline match mode in the engine (#91): `Op::Replace` and
  `Op::Delete` gain a `multiline` field (serde default `false`). When
  set, the pattern matches against the whole buffer instead of
  line-by-line, so finds can span line boundaries (like ripgrep's
  `-U`). Delete removes the matched span rather than whole lines.
  Buffer mode splices replacements into the original text, so line
  endings outside the match are untouched byte-for-byte and `Change`
  metadata always equals the written bytes. Line ranges are rejected
  in multiline mode (`invalid_request`). Breaking for library users:
  constructing `Op::Replace`/`Op::Delete` literals now requires the
  `multiline` field; the JSON wire format is unaffected (field is
  optional, defaults to `false`). CLI/JSON surface lands
  separately (#92).

### Fixed
- Fix `Change.after` metadata for InsertAfter/InsertBefore hardcoding
  LF between the matched line and the inserted content. On CRLF files
  the written output was already correct, but the structured diff shown
  in JSON mode didn't match the actual file bytes. The metadata now
  uses the file's detected line separator. (#89)

### Changed
- Semver-compatible dependency refresh via `cargo update` (regex,
  serde_json, libc, ignore, zerocopy, shlex 1→2 transitive, and
  others), including the fuzz workspace lockfile. No advisories
  (`cargo audit` clean). (#86)
- Harden test suite: remove tautologies, add adversarial tests (#84)
- Replace process::exit calls in run() with Result propagation; the
  only `process::exit` is now in `main()` after `run()` returns, so
  RAII destructors (file locks, temp files) always execute (#22)

## [0.2.9] - 2026-04-17

### Fixed
- Fix Windows build: missing windows-sys features for FileSystem/IO (#85)

### Changed
- **MSRV bumped to 1.93** (from 1.85). Required for the let-chain syntax
  used to collapse several nested `if let` sites per the updated
  `clippy::collapsible_if` lint.
- Major dependency updates:
  - `toml` 0.8 → 1
  - `anstream` 0.6 → 1
  - `criterion` 0.5 → 0.8 (dev-dep; `criterion::black_box` replaced
    with `std::hint::black_box` in benches)
  - `windows-sys` 0.59 → 0.61
- Semver-compatible bumps for clap, proptest, rayon, tempfile, libc,
  indexmap, hashbrown, zerocopy, and others via `cargo update`.

### Security
- Updated `rand` 0.9.2 → 0.9.4 to address RUSTSEC-2026-0097
  (unsound behavior with custom logger using `rand::rng()`).
  `rand` is a transitive dev-dep (via `proptest`); no runtime impact.

## [0.2.8] - 2026-04-16

### Fixed
- Fix FileLock mutual exclusion race under concurrent acquire. The previous
  `O_CREAT|O_EXCL` + PID-staleness approach had an inherent window where
  another thread could observe the empty lock file between `create_new`
  and `writeln!(pid)`, declare it stale, remove it, and create its own —
  producing two concurrent "holders" and silently losing writes.
  `FileLock::acquire` now uses kernel-level file locking (`flock(2)` on
  Unix, `LockFileEx` on Windows) on a persistent sentinel, guaranteeing
  atomic mutual exclusion. The lock file's PID/timestamp content is
  purely informational.
- Fix engine producing `"\n"` (a file with one empty line) instead of
  `""` (an empty file) when a Delete operation removes every line of
  a one-line file. Regression surfaced by a new proptest.
- Fix symlink and hard-link aliases causing the same inode to be
  discovered twice by `discover_files` / `discover_files_parallel`.
  Two paths to the same inode would take out separate per-path locks
  in `ripsed::apply_to_file` and race each other. Paths are now
  deduplicated by `(dev, ino)` on Unix and by canonical path elsewhere.

### Added
- Property-based (proptest) tests in `ripsed-core`: line-number
  invariants (ascending, 1-indexed, in-bounds), exact delete-line-count,
  CRLF majority preservation, Replace change-count vs. containing-lines,
  no-trailing-newline preservation, literal-match equivalence,
  case-insensitive ASCII symmetry.
- Concurrent integration tests in `crates/ripsed/tests/concurrent_write_test.rs`
  exercising `apply_to_file` under heavy contention (distinct
  replacements, idempotent replacement, dry-run safety, reader/writer
  races).
- Real atomic-batch rollback test in `ripsed-fs::writer` — forces a
  commit-phase failure and verifies already-persisted files are
  restored to their pre-commit content.
- Symlink-scope safety tests in `ripsed-fs::discovery` (Unix): external
  targets are NOT followed by default, and alias paths deduplicate
  to one entry.
- Pathological-pattern tests in `ripsed-core::matcher`: literal `$1`,
  regex backreferences, ReDoS-safety smoke test, control-char
  replacements, empty-regex-match safety.
- `no_concurrent_holders_under_hammer` hammer test in `ripsed-fs::lock`
  as a direct mutual-exclusion regression guard.

### Removed
- Tautological serde-roundtrip tests in `ripsed-core` (`Op` roundtrips,
  accessor-over-field tests) and `ripsed-json` (request/schema
  roundtrip). Replaced where meaningful with wire-format-locking
  tests (`replace_op_tag_wire_format`, `transform_mode_wire_names`).
- PID-staleness scaffolding in `ripsed-fs::lock` (`is_lock_stale`,
  `is_process_alive`, `is_older_than`, `EMPTY_LOCK_GRACE`). No
  longer needed now that mutual exclusion is enforced by the kernel
  via `flock`/`LockFileEx`.

## [0.2.7] - 2026-03-28

### Fixed
- Fix lock acquisition in dry_run mode creating unwanted lock files (#81)
- Fix stale lock tests failing on Windows due to conservative is_process_alive (#76)
- Fix flaky lock tests in CI (concurrent race, tempdir lifetime) (#75)
- Fix lock staleness check failing on macOS due to /proc not existing (#71)

### Added
- Add ripsed facade crate and fix read-modify-write locking (#78)
- Integrate FileLock into write_atomic, AtomicBatch::commit, and save_undo_log for inter-process safety (#71)
- Expand lock module test suite from 7 to 26 tests covering staleness, concurrency, and edge cases (#71)

## [0.2.5] - 2026-03-27

### Security
- Add input size limits (64 MiB) to stdin and JSON deserialization (#14)
- Set restrictive file permissions (0600) on undo log writes (#65)
- Improve unsafe mmap safety argument and document invariants (#35)

### Fixed
- Fix --confirm flag applying all changes regardless of per-change user response (#12)
- Fix silent acceptance of invalid glob patterns in file discovery (#13)
- Fix silent backup failure in JSON mode causing potential data loss (#15)
- Fix JSON mode re-reading files per operation instead of composing results (#16)
- Fix uses_crlf normalizing mixed line-ending files to all CRLF (#17)
- Fix Dedent not handling tabs, breaking round-trip with tab Indent (#21)
- Fix spurious change recording for no-op Surround and Indent operations (#29)
- Propagate Config::discover errors instead of silently returning None (#32)
- Fix parallel discovery heuristic — always use parallel walker (#38)
- Fix detect_buffered partial-buffer edge case (#44)
- Fix lock_path_for producing double-dot for extensionless files (#45)
- Replace TOCTOU exists+read pair with single read in AtomicBatch::commit (#48)
- Use schema::CURRENT_VERSION constant instead of hardcoded strings (#53, #54)
- Fix hardcoded operation index 0 in Matcher::new error context (#57)
- Fix lock file PID not flushed before staleness check (#68)
- Reject unknown Op variants in validate_op instead of silent accept (#20)

### Added
- Update README and crate documentation for v0.2.5 changes (#70)
- Add PID and staleness detection to file lock mechanism (#19)
- Add WalkStrategy enum replacing boolean force_parallel parameter (#37)
- Add Default impl for DiscoveryOptions (#36)
- Add test coverage for mmap code path and detect_buffered (#47, #55)
- Extract shared test helpers into common module in CLI tests (#61)

### Changed
- Remove ripsed-core dependency from ripsed-fs (dependency inversion fix) (#40)
- Extract engine apply() match arms into LineCtx-based helpers (#30)
- Deduplicate WalkBuilder configuration into shared helper (#39)
- Deduplicate file-processing logic between file_mode and script_mode (#25)
- Extract mode resolution from run() into Mode enum dispatch (#24, #27)
- Extract repetitive validate_op arms into shared validation helper (#42)
- Eliminate double JSON deserialization in detect_stdin path (#43)
- Replace process::exit in load_config with Result return (#23)
- Deduplicate default_true helper into crate-level function (#56)
- Remove dead code: Matcher::Literal case_insensitive field, rollback wrapper, unused proptest dep (#58, #49, #46, #52)
- Change pub mod to mod for internal CLI modules (#64)
- Replace unwrap_or_else serialization fallback with expect (#51)
- cargo fmt + clippy -D warnings clean (#66)

## [0.2.3] - 2026-03-26

### Fixed
- Fix crosslink cache files tracked in git blocking crate publish (#11)
- Fix Unicode byte-offset mismatch in case-insensitive literal matching (#1)
- Fix non-atomic batch commit in AtomicBatch::commit (#2)
- Fix discovery reading entire files for binary detection (#3)
- Fix silent undo log write failures in save_undo_log (#4)
- Fix silent file read error swallowing in JSON mode (#6)

### Changed
- Consolidate process::exit into single call site in main() (#8)
- Extract shared record_undo() and build_op_options() helpers (#5)
- Replace wasteful matcher.replace() with is_match() in Transform arm (#7)
- Remove unused read_file_with_encoding and read_file_streaming (#10)
- Add test for Transform no-op edge case (#9)

## [0.3.0] - 2026-03-01

### Added
- New operation: `--transform` — change case of matched text (upper, lower, title, snake_case, camel_case)
- New operation: `--surround PREFIX SUFFIX` — wrap matching lines with prefix and suffix
- New operation: `--indent N` — add N spaces before matching lines
- New operation: `--dedent N` — remove up to N leading spaces from matching lines
- `.rip` script files: chain multiple operations in a file, run with `--script path.rip`
- Script parser with quoted strings, escape sequences, inline comments, and per-operation `--glob` scoping
- 4 libfuzzer fuzz targets (regex input, JSON request, engine, autodetect)
- CI: cargo-semver-checks job (advisory) for API compatibility checking
- Claude Code `/ripsed` skill for AI-assisted bulk find-and-replace
- 172 new tests (495 total across all crates)

### Changed
- `Op` and `TransformMode` enums are now `#[non_exhaustive]` for forward-compatible API evolution

### Fixed
- Fix silent file read error swallowing in JSON mode (#6)
- Fix silent undo log write failures in save_undo_log (#4)
- Fix discovery reading entire files for binary detection (#3)
- Fix non-atomic batch commit in AtomicBatch::commit (#2)
- Fix Unicode byte-offset mismatch in case-insensitive literal matching (#1)
- Two integration tests that ran JSON mode with `dry_run: false` and no `root`, causing ripsed to modify its own source tree during `cargo test`

## [0.2.0] - 2026-03-01

### Added
- JSON undo dispatch: send `{"undo": {"last": N}}` to undo operations via JSON mode
- JSONL streaming output with `--jsonl` flag for real-time per-file results
- Atomic batch mode: all-or-nothing writes when `options.atomic` is true in JSON mode
- Undo logging in JSON mode (previously only file mode recorded undo entries)
- Parallel file discovery: auto-switches to parallel walker for large directories
- Config defaults merging: `.ripsed.toml` defaults now apply to CLI invocations
- `--pipe` flag to force pipe mode regardless of TTY detection
- `--follow` flag to follow symbolic links during file discovery
- Integration tests for undo, gitignore, config, cross-platform, and atomic writes
- CI: cargo-deny (license + advisory auditing)
- CI: cargo-audit (CVE checking)
- CI: Miri job (advisory, for undefined behavior detection)
- Release: aarch64-unknown-linux-gnu target
- Release: SHA256 checksum generation

### Changed
- File discovery now uses auto-switching heuristic (serial for small dirs, parallel for large)
- Refactored CLI into separate modules (json_mode, file_mode, pipe_mode, shared)

### Removed
- `--in-place` flag (redundant; file mode writes in-place by default)

## [0.1.0] - 2026-03-01

### Added
- Initial release
- Four-crate workspace architecture (ripsed-core, ripsed-fs, ripsed-json, ripsed-cli)
- JSON agent mode with auto-detection from stdin
- File mode with colored diffs and dry-run preview
- Pipe mode (stdin -> stdout) for Unix pipeline integration
- Operations: replace, delete, insert_after, insert_before, replace_line
- Regex support with capture group replacement
- Case-insensitive matching
- Per-operation glob filtering in JSON mode
- File discovery with .gitignore support
- Atomic file writes with temp file + rename
- Backup file creation (`.ripsed.bak`)
- Undo support (`--undo`, `--undo-list`)
- Interactive confirmation mode (`--confirm`)
- Configuration via `.ripsed.toml` with directory discovery
- CRLF line ending preservation
- Binary file detection and skipping
- Memory-mapped I/O for large files
- Cross-platform support (Linux, macOS, Windows)
- 273 tests across all crates
