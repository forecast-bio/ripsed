# ripsed-core

Core edit engine for [ripsed](https://github.com/dollspace-gay/ripsed) — a fast, modern stream editor.

This crate contains the core logic:

- **Edit engine** — apply find/replace, delete, insert, transform, surround, indent/dedent operations to text
- **Pattern matching** — literal and regex matching with case-insensitive support
- **Operation IR** — the `Op` enum representing all supported operations
- **Script parser** — parse `.rip` script files into operation sequences
- **Error taxonomy** — structured errors with machine-readable codes and actionable hints
- **Configuration** — `.ripsed.toml` parsing and discovery
- **Undo** — undo log data structures for reversible operations

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
};

let matcher = Matcher::new(&op).unwrap();
let output = engine::apply("old text here\n", &op, &matcher, None, 3).unwrap();
assert_eq!(output.text.unwrap(), "new text here\n");
```

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0) or [MIT license](http://opensource.org/licenses/MIT) at your option.
