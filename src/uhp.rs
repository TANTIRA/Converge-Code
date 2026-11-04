use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::session::Session;

pub fn endpoint(home: &Path, session_id: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"\\.\pipe\converge-{session_id}"))
    } else {
        home.join("run").join(format!("{session_id}.sock"))
    }
}

pub struct Server {
    pub socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        {
            let _ = std::os::unix::net::UnixStream::connect(&self.socket);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.socket);
    }
}

pub fn serve(home: &Path, session: &Session, repo: &Path) -> Result<Server> {
    let socket = endpoint(home, &session.id);
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("create {}: {error}", parent.display())))?;
    }
    let _ = fs::remove_file(&socket);
    #[cfg(unix)]
    {
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .map_err(|error| Error::new(format!("bind {}: {error}", socket.display())))?;
        listener
            .set_nonblocking(false)
            .map_err(|error| Error::new(format!("bind {}: {error}", socket.display())))?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&shutdown);
        let session_dir = session.dir.clone();
        let session_id = session.id.clone();
        let repo = repo.to_path_buf();
        let clients: Arc<Mutex<Vec<std::os::unix::net::UnixStream>>> =
            Arc::new(Mutex::new(Vec::new()));
        let thread = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    break;
                };
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.set_nonblocking(false);
                let Ok(writer) = stream.try_clone() else {
                    continue;
                };
                clients.lock().expect("clients").push(writer);
                let clients = Arc::clone(&clients);
                let session_dir = session_dir.clone();
                let session_id = session_id.clone();
                let repo = repo.clone();
                thread::spawn(move || {
                    let _ = reader(stream, &clients, &session_dir, &session_id, &repo);
                });
            }
        });
        return Ok(Server {
            socket,
            shutdown,
            thread: Some(thread),
        });
    }
    #[cfg(not(unix))]
    {
        let _ = (session, repo);
        Err(Error::new(format!(
            "named pipe {} is the Windows transport",
            socket.display()
        )))
    }
}

#[cfg(unix)]
fn reader(
    stream: std::os::unix::net::UnixStream,
    clients: &Arc<Mutex<Vec<std::os::unix::net::UnixStream>>>,
    session_dir: &Path,
    session_id: &str,
    repo: &Path,
) -> Result<()> {
    let mut incoming = BufReader::new(stream);
    let mut line = String::new();
    while incoming
        .read_line(&mut line)
        .ok()
        .filter(|count| *count > 0)
        .is_some()
    {
        let trimmed = line.trim();
        if let Ok(request) = serde_json::from_str::<Value>(trimmed) {
            let response = dispatch_rpc(&request, session_dir, session_id, repo);
            if request.get("method").and_then(Value::as_str) == Some("session/input") {
                let text = request
                    .pointer("/params/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let note =
                    json!({"jsonrpc":"2.0","method":"session/stream","params":{"text": text}});
                broadcast(clients, &note.to_string());
            }
            broadcast(clients, &response);
        }
        line.clear();
    }
    Ok(())
}

fn dispatch_rpc(request: &Value, session_dir: &Path, session_id: &str, repo: &Path) -> String {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let id = request.get("id").cloned();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "session/attach" => json!({
            "session_id": session_id,
            "transcript": read_transcript(session_dir),
        }),
        "session/input" => {
            let text = params.get("text").and_then(Value::as_str).unwrap_or("");
            append_line(session_dir, &json!({"kind": "user", "text": text}));
            broadcast_note(
                session_dir,
                &json!({"jsonrpc": "2.0", "method": "session/stream", "params": {"text": text}}),
            );
            json!({"ok": true})
        }
        "session/selection" => {
            append_line(session_dir, &json!({"kind": "selection", "text": params}));
            json!({"ok": true})
        }
        "permission/respond" => {
            append_line(session_dir, &json!({"kind": "permission", "text": params}));
            json!({"ok": true})
        }
        "session/diff" => json!({"diff": git_diff(repo)}),
        "session/status" => json!({"session_id": session_id}),
        "session/tool" => {
            let note = json!({"jsonrpc": "2.0", "method": "session/tool", "params": params});
            broadcast_note(session_dir, &note);
            json!({"ok": true})
        }
        _ => json!({"error": "unknown method"}),
    };
    let response = if let Some(id) = id {
        json!({"jsonrpc": "2.0", "id": id, "result": result})
    } else {
        result
    };
    response.to_string()
}

fn broadcast_note(session_dir: &Path, note: &Value) {
    let path = session_dir.join("uhp-notes.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{note}");
    }
}

#[cfg(unix)]
fn broadcast(clients: &Mutex<Vec<std::os::unix::net::UnixStream>>, line: &str) {
    let streams = {
        let guard = clients.lock().expect("clients");
        guard
            .iter()
            .filter_map(|stream| stream.try_clone().ok())
            .collect::<Vec<_>>()
    };
    let payload = format!("{line}\n");
    for mut stream in streams {
        let _ = stream.write_all(payload.as_bytes());
        let _ = stream.flush();
    }
}

fn read_transcript(session_dir: &Path) -> String {
    fs::read_to_string(session_dir.join("transcript.jsonl")).unwrap_or_default()
}

fn append_line(session_dir: &Path, value: &Value) {
    let path = session_dir.join("transcript.jsonl");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{value}");
    }
}

fn git_diff(repo: &Path) -> String {
    let output = std::process::Command::new("git")
        .args(["diff"])
        .current_dir(repo)
        .output();
    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

pub fn allowed_methods() -> &'static [&'static str] {
    &[
        "session/attach",
        "session/input",
        "session/selection",
        "permission/respond",
        "session/stream",
        "session/tool",
        "permission/ask",
        "session/diff",
        "session/status",
    ]
}
