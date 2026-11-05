use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{Error, Result};

pub const ASKS_ENV: &str = "CONVERGE_CHILD_ASKS";
pub const PERMISSION_ENV: &str = "CONVERGE_CHILD_PERMISSION";
pub const ID_ENV: &str = "CONVERGE_CHILD_ID";

// A child that nobody answers stops rather than wait forever.
const ANSWER_LIMIT: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny,
    // No person can be asked, so the child exits non-zero as any run without a terminal does.
    Stop,
}

impl Answer {
    fn word(self) -> &'static str {
        match self {
            Answer::Allow => "allow",
            Answer::Deny => "deny",
            Answer::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ask {
    pub id: String,
    pub child: String,
    pub name: String,
    pub arguments: String,
}

pub fn asks_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("asks")
}

// Called in the child. Blocks until the parent answers.
pub fn ask(dir: &Path, child: &str, name: &str, arguments: &str) -> Result<Answer> {
    let id = Uuid::new_v4().to_string();
    let request = json!({"child": child, "name": name, "arguments": arguments});
    write_atomic(dir, &format!("{id}.ask"), request.to_string().as_bytes())?;
    let answer_path = dir.join(format!("{id}.answer"));
    let started = Instant::now();
    let answer = loop {
        if let Ok(word) = fs::read_to_string(&answer_path) {
            break match word.trim() {
                "allow" => Answer::Allow,
                "deny" => Answer::Deny,
                _ => Answer::Stop,
            };
        }
        if started.elapsed() >= ANSWER_LIMIT {
            break Answer::Stop;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let _ = fs::remove_file(dir.join(format!("{id}.ask")));
    let _ = fs::remove_file(answer_path);
    Ok(answer)
}

// Called in the parent. Lists the questions that have no answer yet.
pub fn pending(dir: &Path) -> Vec<Ask> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut asks = Vec::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().and_then(|ext| ext.to_str()) != Some("ask")
            || path.with_extension("answer").exists()
        {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Some(request) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        else {
            continue;
        };
        let field = |key: &str| request[key].as_str().unwrap_or("").to_string();
        asks.push(Ask {
            id: id.to_string(),
            child: field("child"),
            name: field("name"),
            arguments: field("arguments"),
        });
    }
    asks
}

pub fn answer(dir: &Path, id: &str, answer: Answer) -> Result<()> {
    write_atomic(dir, &format!("{id}.answer"), answer.word().as_bytes())
}

// The rename means a reader never sees half a file.
fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(dir)
        .map_err(|error| Error::new(format!("create {}: {error}", dir.display())))?;
    let temporary = dir.join(format!("{name}.tmp"));
    let path = dir.join(name);
    fs::write(&temporary, bytes)
        .and_then(|_| fs::rename(&temporary, &path))
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}
