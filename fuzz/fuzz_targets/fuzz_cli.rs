#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let args = text.split('\0').map(str::to_string);
    let _ = converge::Invocation::parse(args);
});
