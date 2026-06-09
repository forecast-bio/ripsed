# Contributing to ripsed

Thanks for considering a contribution! Issues and pull requests are
welcome.

## Development setup

```bash
git clone https://github.com/dollspace-gay/ripsed
cd ripsed
cargo test --workspace
```

Requires Rust **1.93+** (the workspace MSRV; CI enforces it).

## Workspace map

| Crate | Purpose |
|---|---|
| `crates/ripsed-core` | Pure logic: engine, matcher, operation IR, ranges, undo, errors — no I/O |
| `crates/ripsed-fs` | Discovery, encoding-aware reading, atomic writes, file locks |
| `crates/ripsed-json` | Agent protocol: request/response schema, validation, auto-detection |
| `crates/ripsed-cli` | The `ripsed` binary: modes, args, output |
| `crates/ripsed` | Library facade (`apply_to_file` etc.) |
| `xtask` | Build tooling: schema/completions/fixtures generation, benchmarks |
| `fuzz` | libfuzzer targets (separate workspace) |

## Before opening a PR

All of these run in CI and must pass:

```bash
cargo test --workspace
cargo clippy --all-targets --workspace -- -D warnings
cargo fmt --all --check
cargo run -p xtask -- gen-completions   # CLI changes must still generate docs
```

Please also:

- **Add tests with the change.** Behavioral tests, not tautologies —
  assert specific outputs, exercise the edge cases (empty files, CRLF,
  no trailing newline, Unicode). Engine invariants often fit a
  proptest.
- **Add a CHANGELOG entry** under `## [Unreleased]` for anything
  user-visible. Mark breaking changes loudly.
- **Keep `Change` metadata byte-faithful**: anything surfaced in the
  JSON diff must equal the bytes actually written. This invariant is
  load-bearing for agent consumers.

## Running the extras

```bash
cargo run -p xtask -- bench                   # criterion micro-benchmarks
cargo run -p xtask -- bench-compare --quick   # vs sed/sd/perl (needs hyperfine)
cargo +nightly fuzz run fuzz_engine -- -max_total_time=60   # needs cargo-fuzz
```

Heads-up: run `cargo` commands from the repo root. The `fuzz/`
directory is its own workspace, and `cargo test --workspace` from
inside it will run libfuzzer binaries — which never terminate.

## Licensing

Dual-licensed under MIT or Apache-2.0. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in
this project by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
