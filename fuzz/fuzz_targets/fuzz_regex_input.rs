#![no_main]

use libfuzzer_sys::fuzz_target;
use ripsed_core::matcher::Matcher;
use ripsed_core::operation::Op;

fuzz_target!(|data: &[u8]| {
    // Convert random bytes to a string; skip non-UTF-8 inputs.
    let pattern = match std::str::from_utf8(data) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Build an Op::Replace with regex=true so that Matcher::new attempts
    // to compile the fuzzed pattern as a regex.
    let op = Op::Replace {
        count: Default::default(),
        find: pattern.to_string(),
        replace: String::new(),
        regex: true,
        case_insensitive: false,
        multiline: false,
    };

    // This must never panic. It either succeeds or returns an Err with
    // an invalid_regex error code.
    let _ = Matcher::new(&op);
});
