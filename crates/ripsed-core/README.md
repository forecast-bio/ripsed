# ripsed-core

Core edit engine for [ripsed](https://github.com/dollspace-gay/ripsed) — a fast, modern stream editor.

This crate contains the pure logic (no I/O):

- **Edit engine** — find/replace, delete, insert, transform, surround,
  indent/dedent, applied through whichever execution path fits: the
  per-line loop, a whole-buffer splice fast path for eligible literal
  replaces, multiline (cross-line) matching like ripgrep's `-U`, or the
  incremental `LineProcessor` for streaming callers
- **Pattern matching** — literal and regex matching with
  case-insensitive support and a whole-buffer prescreen that rejects
  non-matching files at substring-search speed
- **Operation IR** — the `Op` enum representing all supported
  operations, including replacement-count control (`first_per_line`,
  `first_in_file`, `{"max": n}`)
- **Ranges** — numeric line ranges and sed-style pattern-addressed
  regions (`/start/,/end/`)
- **Script parser** — parse `.rip` script files into operation sequences
- **Error taxonomy** — structured errors with machine-readable codes and
  actionable hints
- **Configuration** — `.ripsed.toml` parsing and discovery
- **Undo** — undo log data structures for reversible operations

A load-bearing invariant for agent consumers: `Change` metadata always
equals the bytes actually written, including line separators.

## Usage

```rust
use ripsed_core::engine;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::Op;

let op = Op::Replace {
    find: "old".to_string(),
    replace: "new".to_string(),
    regex: false,
    case_insensitive: false,
    multiline: false,
    count: Default::default(),
};

let matcher = Matcher::new(&op).unwrap();
let output = engine::apply("old text here\n", &op, &matcher, None, 3).unwrap();
assert_eq!(output.text.unwrap(), "new text here\n");
```

For embedding with file I/O included (locking, atomic writes, undo), use
the [`ripsed`](https://crates.io/crates/ripsed) facade crate instead.

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0) or [MIT license](http://opensource.org/licenses/MIT) at your option.
