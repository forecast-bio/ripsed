#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use ripsed_core::engine;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::Op;

#[derive(Arbitrary, Debug)]
struct EngineInput {
    text: String,
    find: String,
    replace: String,
    multiline: bool,
}

fuzz_target!(|input: EngineInput| {
    // Build an Op::Replace from the fuzzed fields. `multiline` exercises
    // the whole-buffer span path as well as the per-line path.
    let op = Op::Replace {
        count: Default::default(),
        find: input.find,
        replace: input.replace,
        regex: false,
        case_insensitive: false,
        multiline: input.multiline,
    };

    // Build a matcher; if this fails (shouldn't for literal mode), just return.
    let matcher = match Matcher::new(&op) {
        Ok(m) => m,
        Err(_) => return,
    };

    // Apply the engine. This must never panic.
    let result = engine::apply(&input.text, &op, &matcher, None, 0);

    // If the engine returned Ok, verify the output text is valid UTF-8
    // (it should be, since it is a String).
    if let Ok(output) = result {
        if let Some(ref text) = output.text {
            // text is a String, so it is guaranteed valid UTF-8.
            // Explicitly verify as a sanity check.
            assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        }
    }
});
