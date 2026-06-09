use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        Some("gen-schema") => gen_schema(),
        Some("gen-fixtures") => gen_fixtures(),
        Some("bench") => bench(),
        Some("bench-compare") => bench_compare(args.iter().any(|a| a == "--quick")),
        _ => {
            eprintln!("Usage: cargo xtask <gen-schema|gen-fixtures|bench|bench-compare [--quick]>");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// bench-compare: reproducible comparison against sed, sd, and perl
// ---------------------------------------------------------------------------

/// One benchmark scenario: a corpus and the equivalent command per tool.
struct Scenario {
    name: &'static str,
    description: &'static str,
    /// Shell command per (tool label, command). `$WORK` is the corpus copy.
    commands: Vec<(String, String)>,
    /// Hyperfine minimum runs.
    runs: u32,
}

/// Generate corpora, run every available tool through hyperfine with a
/// pristine corpus restored before each timing iteration, and print a
/// markdown results table (paste into BENCHMARKS.md).
fn bench_compare(quick: bool) {
    use std::process::Command;

    // Release binary, built fresh so the numbers reflect HEAD.
    eprintln!("building ripsed --release...");
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "ripsed-cli"])
        .status()
        .expect("cargo build");
    assert!(status.success(), "release build failed");
    let ripsed = std::fs::canonicalize("target/release/ripsed").expect("release binary");
    let ripsed = ripsed.display();

    let have = |tool: &str| {
        Command::new(tool)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let have_sed = have("sed");
    let have_sd = have("sd");
    let have_perl = have("perl");
    eprintln!("tools: sed={have_sed} sd={have_sd} perl={have_perl} (missing tools are skipped)");

    // ── Corpora ──────────────────────────────────────────────────────
    let root = std::path::Path::new("target/bench-compare");
    let _ = std::fs::remove_dir_all(root);

    // Tree corpus: many files, ~10% of lines match.
    let (n_files, n_lines) = if quick { (200, 100) } else { (1000, 200) };
    let tree = root.join("tree-pristine");
    for i in 0..n_files {
        let dir = tree.join(format!("d{}", i % 20));
        std::fs::create_dir_all(&dir).unwrap();
        let mut content = String::new();
        for l in 0..n_lines {
            if l % 10 == 0 {
                content.push_str("a needle line that will be edited\n");
            } else {
                content.push_str("plain filler content on this line\n");
            }
        }
        std::fs::write(dir.join(format!("f{i}.txt")), content).unwrap();
    }

    // No-match corpus: same shape, pattern absent (prescreen showcase).
    let nomatch = root.join("nomatch-pristine");
    for i in 0..n_files {
        let dir = nomatch.join(format!("d{}", i % 20));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("f{i}.txt")),
            "plain filler content on this line\n".repeat(n_lines),
        )
        .unwrap();
    }

    // Single big file.
    let big_mb = if quick { 8 } else { 64 };
    let bigdir = root.join("big-pristine");
    std::fs::create_dir_all(&bigdir).unwrap();
    let line = "a needle line that will be edited\nplain filler content on this line\n";
    let big_content = line.repeat(big_mb * 1024 * 1024 / line.len());
    std::fs::write(bigdir.join("big.txt"), &big_content).unwrap();
    drop(big_content);

    // ── Scenarios ────────────────────────────────────────────────────
    // Every tool gets a functionally equivalent in-place edit. sed/sd/perl
    // receive an explicit file list via find -print0 | xargs -0 (they have
    // no recursive discovery); ripsed discovers recursively itself — that
    // built-in parallel discovery is part of what is being measured.
    let tree_cmds = |pattern: &str| {
        let mut v = Vec::new();
        v.push((
            "ripsed".to_string(),
            format!("cd $WORK && {ripsed} '{pattern}' thread"),
        ));
        if have_sed {
            v.push((
                "sed".to_string(),
                format!(
                    "find $WORK -name '*.txt' -print0 | xargs -0 sed -i 's/{pattern}/thread/g'"
                ),
            ));
        }
        if have_sd {
            v.push((
                "sd".to_string(),
                format!("find $WORK -name '*.txt' -print0 | xargs -0 sd '{pattern}' thread"),
            ));
        }
        if have_perl {
            v.push((
                "perl".to_string(),
                format!(
                    "find $WORK -name '*.txt' -print0 | xargs -0 perl -pi -e 's/{pattern}/thread/g'"
                ),
            ));
        }
        v
    };

    let big_cmds = {
        let mut v = vec![(
            "ripsed".to_string(),
            format!("cd $WORK && {ripsed} needle thread"),
        )];
        if have_sed {
            v.push((
                "sed".to_string(),
                "sed -i 's/needle/thread/g' $WORK/big.txt".to_string(),
            ));
        }
        if have_sd {
            v.push((
                "sd".to_string(),
                "sd needle thread $WORK/big.txt".to_string(),
            ));
        }
        if have_perl {
            v.push((
                "perl".to_string(),
                "perl -pi -e 's/needle/thread/g' $WORK/big.txt".to_string(),
            ));
        }
        v
    };

    let scenarios = [
        Scenario {
            name: "tree-replace",
            description: "literal replace across a source tree",
            commands: tree_cmds("needle"),
            runs: if quick { 3 } else { 10 },
        },
        Scenario {
            name: "tree-no-match",
            description: "pattern matches nothing in the tree",
            commands: tree_cmds("zebra"),
            runs: if quick { 3 } else { 10 },
        },
        Scenario {
            name: "big-file",
            description: "literal replace in one large file",
            commands: big_cmds,
            runs: if quick { 3 } else { 10 },
        },
    ];

    // ── Run ──────────────────────────────────────────────────────────
    println!("\n## Results\n");
    println!(
        "Corpus: {n_files} files x {n_lines} lines (tree scenarios), {big_mb} MiB (big-file)."
    );
    for scenario in &scenarios {
        let pristine = match scenario.name {
            "tree-replace" => &tree,
            "tree-no-match" => &nomatch,
            _ => &bigdir,
        };
        let work = root.join(format!("{}-work", scenario.name));
        let json_out = root.join(format!("{}.json", scenario.name));

        let mut cmd = Command::new("hyperfine");
        // --ignore-failure: ripsed currently exits 1 on zero matches
        // (tree-no-match scenario); hyperfine would refuse to time it.
        cmd.arg("--ignore-failure")
            .arg("--warmup")
            .arg("1")
            .arg("--min-runs")
            .arg(scenario.runs.to_string())
            .arg("--prepare")
            .arg(format!(
                "rm -rf {work} && cp -r {pristine} {work}",
                work = work.display(),
                pristine = pristine.display()
            ))
            .arg("--export-json")
            .arg(&json_out);
        for (label, shell_cmd) in &scenario.commands {
            cmd.arg("--command-name").arg(label);
            cmd.arg(shell_cmd.replace("$WORK", &work.display().to_string()));
        }
        eprintln!("\n=== {} ({}) ===", scenario.name, scenario.description);
        let status = cmd
            .status()
            .expect("hyperfine (install it to run bench-compare)");
        assert!(status.success(), "hyperfine failed for {}", scenario.name);

        // Markdown table row from hyperfine's JSON export.
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_out).unwrap()).unwrap();
        println!("\n### {} — {}\n", scenario.name, scenario.description);
        println!("| tool | mean | min |");
        println!("|---|---|---|");
        let results = data["results"].as_array().unwrap();
        let best = results
            .iter()
            .map(|r| r["mean"].as_f64().unwrap())
            .fold(f64::INFINITY, f64::min);
        for r in results {
            let mean = r["mean"].as_f64().unwrap();
            let min = r["min"].as_f64().unwrap();
            let marker = if (mean - best).abs() < f64::EPSILON {
                " **(fastest)**"
            } else {
                ""
            };
            println!(
                "| {}{} | {:.1} ms | {:.1} ms |",
                r["command"].as_str().unwrap(),
                marker,
                mean * 1000.0,
                min * 1000.0
            );
        }
    }
    eprintln!("\ncorpora and JSON exports left in target/bench-compare/");
}

fn gen_schema() {
    // Build a JSON description of the JsonRequest schema based on the actual
    // types in ripsed-json and ripsed-core.  We construct a hand-written
    // schema object that mirrors the Rust structs (JsonRequest, Op, OpOptions,
    // etc.) rather than pulling in a full JSON-Schema derive crate.

    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "RipsedJsonRequest",
        "description": "Schema for the ripsed JSON request protocol (version 1).",
        "type": "object",
        "properties": {
            "version": {
                "type": "string",
                "description": "Protocol version. Currently only \"1\" is supported.",
                "default": "1",
                "enum": ["1"]
            },
            "operations": {
                "type": "array",
                "description": "List of operations to apply.",
                "items": {
                    "$ref": "#/$defs/JsonOp"
                }
            },
            "options": {
                "$ref": "#/$defs/OpOptions"
            },
            "undo": {
                "$ref": "#/$defs/UndoRequest"
            }
        },
        "additionalProperties": true,
        "$defs": {
            "JsonOp": {
                "description": "A single operation with an optional per-operation glob.",
                "type": "object",
                "required": ["op", "find"],
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["replace", "delete", "insert_after", "insert_before", "replace_line", "transform", "surround", "indent", "dedent"],
                        "description": "The operation type."
                    },
                    "find": {
                        "type": "string",
                        "description": "The pattern to search for (literal or regex).",
                        "minLength": 1
                    },
                    "replace": {
                        "type": "string",
                        "description": "Replacement text (required for 'replace' op)."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to insert or replace the line with (required for insert_after, insert_before, replace_line).",
                        "minLength": 1
                    },
                    "regex": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether 'find' is a regex pattern."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether matching is case-insensitive."
                    },
                    "multiline": {
                        "type": "boolean",
                        "default": false,
                        "description": "Match across line boundaries against the whole buffer (replace and delete only)."
                    },
                    "count": {
                        "default": "all",
                        "description": "How many occurrences to replace (replace only): 'all', 'first_per_line', 'first_in_file', or {\"max\": n}.",
                        "oneOf": [
                            {"type": "string", "enum": ["all", "first_per_line", "first_in_file"]},
                            {
                                "type": "object",
                                "required": ["max"],
                                "properties": {"max": {"type": "integer", "minimum": 1}},
                                "additionalProperties": false
                            }
                        ]
                    },
                    "glob": {
                        "type": "string",
                        "description": "Per-operation file glob (overrides options.glob)."
                    }
                }
            },
            "OpOptions": {
                "description": "Options that control how operations are applied.",
                "type": "object",
                "properties": {
                    "dry_run": {
                        "type": "boolean",
                        "default": true,
                        "description": "Preview changes without writing."
                    },
                    "root": {
                        "type": "string",
                        "description": "Root directory to operate in."
                    },
                    "gitignore": {
                        "type": "boolean",
                        "default": true,
                        "description": "Respect .gitignore rules."
                    },
                    "backup": {
                        "type": "boolean",
                        "default": false,
                        "description": "Create .bak backup files."
                    },
                    "atomic": {
                        "type": "boolean",
                        "default": false,
                        "description": "Atomic batch mode: all-or-nothing writes."
                    },
                    "glob": {
                        "type": "string",
                        "description": "Global file glob pattern."
                    },
                    "ignore": {
                        "type": "string",
                        "description": "Glob pattern for files to ignore."
                    },
                    "hidden": {
                        "type": "boolean",
                        "default": false,
                        "description": "Include hidden files."
                    },
                    "range": {
                        "type": "object",
                        "description": "Pattern-addressed regions, like sed /start/,/end/ (mutually exclusive with line_range). Both patterns are regexes; regions are inclusive of boundary lines and an unclosed region extends to EOF.",
                        "required": ["start_pattern", "end_pattern"],
                        "properties": {
                            "start_pattern": {"type": "string"},
                            "end_pattern": {"type": "string"}
                        },
                        "additionalProperties": false
                    },
                    "max_depth": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum directory traversal depth."
                    },
                    "line_range": {
                        "$ref": "#/$defs/LineRange"
                    }
                }
            },
            "LineRange": {
                "description": "A range of lines to operate on (1-indexed, inclusive).",
                "type": "object",
                "required": ["start"],
                "properties": {
                    "start": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Start line (1-indexed, inclusive)."
                    },
                    "end": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "End line (1-indexed, inclusive). If omitted, extends to end of file."
                    }
                }
            },
            "UndoRequest": {
                "description": "Request to undo the last N operations.",
                "type": "object",
                "required": ["last"],
                "properties": {
                    "last": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of operations to undo."
                    }
                }
            }
        }
    });

    // Verify the schema round-trips through our own parser by checking it's
    // valid JSON (it always will be since we built it with serde_json, but
    // this also exercises the ripsed-json crate link).
    let _: ripsed_json::request::JsonRequest = serde_json::from_value(json!({
        "operations": [{"op": "replace", "find": "a", "replace": "b"}]
    }))
    .expect("sanity: example request should parse");

    println!(
        "{}",
        serde_json::to_string_pretty(&schema).expect("schema serialization should not fail")
    );
}

fn gen_fixtures() {
    use std::fs;
    use std::path::Path;

    // Locate the project root (xtask is run from the workspace root by cargo).
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask crate should be inside the workspace");
    let base = root.join("tests/fixtures/sample-project");

    let files: &[(&str, &[u8])] = &[
        ("hello.txt", b"Hello, world!\n"),
        ("src/main.rs", b"fn main() {\n    println!(\"Hello\");\n}\n"),
        (
            "src/lib.rs",
            b"pub fn greet() -> &'static str {\n    \"Hello\"\n}\n",
        ),
        (".gitignore", b"target/\n*.log\n"),
        ("binary.dat", b"\x00\x00\x00\x00\x00"),
        ("data.log", b"log line 1\nlog line 2\n"),
    ];

    for (rel_path, contents) in files {
        let full = base.join(rel_path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|e| {
                panic!("failed to create directory {}: {e}", parent.display());
            });
        }
        fs::write(&full, contents).unwrap_or_else(|e| {
            panic!("failed to write {}: {e}", full.display());
        });
    }

    println!(
        "Generated {} fixture files in {}",
        files.len(),
        base.display()
    );
}

fn bench() {
    let status = std::process::Command::new("cargo")
        .args(["bench", "--workspace"])
        .status()
        .expect("failed to execute cargo bench");

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}
