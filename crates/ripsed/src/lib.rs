//! # ripsed
//!
//! Bulk find-and-replace engine with regex support, multi-file operations,
//! and atomic writes. This crate is the public facade that re-exports the
//! sub-crates and provides a high-level, concurrency-safe API.
//!
//! ## Quick start
//!
//! ```no_run
//! use ripsed::{Op, apply_to_file, ApplyOptions};
//! use std::path::Path;
//!
//! let op = Op::Replace {
//!     find: "foo".into(),
//!     replace: "bar".into(),
//!     regex: false,
//!     case_insensitive: false,
//!     multiline: false,
//!     count: Default::default(),
//! };
//!
//! let result = apply_to_file(
//!     Path::new("src/main.rs"),
//!     &op,
//!     &ApplyOptions::default(),
//! );
//! ```
//!
//! ## Crate organisation
//!
//! | Crate | Purpose |
//! |-------|---------|
//! | [`core`] | Engine, operations, config, diff, undo |
//! | [`fs`]   | File discovery, reading, atomic writes, advisory locks |
//! | [`json`] | JSON request/response protocol |

// Re-export sub-crates for full access.
pub use ripsed_core as core;
pub use ripsed_fs as fs;
pub use ripsed_json as json;

// Convenience re-exports of the most commonly used types.
pub use ripsed_core::config::Config;
pub use ripsed_core::diff::Change;
pub use ripsed_core::engine::{self, EngineOutput};
pub use ripsed_core::error::RipsedError;
pub use ripsed_core::matcher::Matcher;
pub use ripsed_core::operation::{LineRange, Op, PatternRange, RangeSpec};
pub use ripsed_core::undo::UndoEntry;
pub use ripsed_fs::lock::FileLock;

mod apply;
pub use apply::{ApplyOptions, apply_to_file, apply_to_files};
