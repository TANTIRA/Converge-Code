#![no_main]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

fn workspace() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("converge-fuzz-edit-{}", std::process::id()));
        let _ = fs::create_dir_all(&path);
        path
    })
}

fn clear(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > 1024 {
        return;
    }
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut parts = text.splitn(3, '\0');
    let path = parts.next().unwrap_or("");
    let old = parts.next().unwrap_or("");
    let new = parts.next().unwrap_or("");
    let _ = converge::edit_file(workspace(), path, old, new);
    clear(workspace());
});
