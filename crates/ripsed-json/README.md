# ripsed-json

Agent/JSON interface for [ripsed](https://github.com/dollspace-gay/ripsed) — a fast, modern stream editor.

This crate provides the structured JSON protocol for AI coding agents, editor plugins, and automation pipelines:

- **Request parsing** — versioned JSON request schema with validation
  and helpful, machine-readable error messages (unsupported fields on
  an operation are rejected, not silently ignored)
- **Response building** — structured JSON responses with per-file
  diffs, change counts, and error details (`code`/`message`/`hint`)
- **Auto-detection** — determine whether stdin contains a JSON request
  or plain pipe text for seamless mode switching
- **Schema versioning** — protocol version management with
  forward-compatible validation
- **Undo protocol** — JSON interface for undo operations

## Example request

```json
{
  "version": "1",
  "operations": [
    {
      "op": "replace",
      "find": "old_function",
      "replace": "new_function",
      "glob": "src/**/*.rs",
      "count": "first_per_line"
    },
    {
      "op": "delete",
      "find": "^\\s*//\\s*BEGIN DEBUG[\\s\\S]*?END DEBUG",
      "regex": true,
      "multiline": true
    }
  ],
  "options": {
    "dry_run": true,
    "root": "./my-project",
    "range": {"start_pattern": "fn main", "end_pattern": "^}"},
    "atomic": true,
    "record_undo": true
  }
}
```

A machine-readable JSON Schema for the full protocol can be generated
from the repository with `cargo xtask gen-schema`.

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0) or [MIT license](http://opensource.org/licenses/MIT) at your option.
