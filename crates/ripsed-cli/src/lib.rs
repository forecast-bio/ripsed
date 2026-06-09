//! Library surface for build tooling.
//!
//! Exposes the clap CLI definition so `cargo xtask gen-completions` can
//! generate shell completions and the man page from the real argument
//! parser instead of a drifting copy. The binary keeps its own `mod args`
//! (the module is compiled into both targets via `#[path]`); nothing else
//! is exported and this is not a stable library API.

#[path = "args.rs"]
pub mod args;
