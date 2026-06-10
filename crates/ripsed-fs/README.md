# ripsed-fs

File system layer for [ripsed](https://github.com/dollspace-gay/ripsed) — a fast, modern stream editor.

This crate handles all file I/O:

- **File discovery** — recursive parallel directory walking with
  `.gitignore` support and glob filtering
- **Encoding-aware reading** — UTF-8 plus BOM-detected UTF-8-with-BOM
  and UTF-16 LE/BE, memory-mapped I/O for large files, and binary
  detection that knows UTF-16 text is not binary despite its NUL bytes
- **Atomic writes** — temp file + rename, re-encoding to the source
  encoding with the BOM re-attached; batch mode with all-or-nothing
  commit and rollback
- **Backups** — `.ripsed.bak` file creation with numbered suffixes
- **File locking** — kernel-level advisory locks (`flock(2)` on Unix,
  `LockFileEx` on Windows) on persistent sentinels, guaranteeing mutual
  exclusion between concurrent ripsed processes

## License

Licensed under either of [Apache License, Version 2.0](http://www.apache.org/licenses/LICENSE-2.0) or [MIT license](http://opensource.org/licenses/MIT) at your option.
