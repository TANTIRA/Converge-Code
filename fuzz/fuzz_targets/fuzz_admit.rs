#![no_main]

use std::path::PathBuf;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let Ok(text) = std::str::from_utf8(&data[1..]) else {
        return;
    };
    let mut parts = text.split('\0');
    let work_mode = parts.next().unwrap_or("ask");
    let permission_mode = parts.next().unwrap_or("ask-each");
    let name = parts.next().unwrap_or("shell");
    let arguments = parts.next().unwrap_or("");
    let session = converge::Session {
        id: "fuzz".to_string(),
        dir: PathBuf::from("/tmp/converge-fuzz-absent"),
        workspace: PathBuf::from("/tmp/converge-fuzz-absent"),
        goal: None,
        budget: converge::Budget {
            steps: None,
            tokens: None,
            seconds: None,
        },
        used: converge::Usage::default(),
        work_mode: work_mode.to_string(),
        permission_mode: permission_mode.to_string(),
        source_closed: data[0] & 1 == 1,
        mcp_server: None,
    };
    let _ = converge::admit(&session, name, arguments, data[0] & 2 == 2);
});
