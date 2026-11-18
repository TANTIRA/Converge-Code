use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("{prefix}-{}-{}-{tick}", std::process::id(), n));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git_repo() -> TempDir {
    let dir = TempDir::new("converge-repo");
    git(dir.path(), &["init"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    dir
}

fn git(repo: &Path, args: &[&str]) {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git");
}

fn call(id: &str, name: &str, arguments: &str) -> String {
    let arguments = serde_json::to_string(arguments).expect("arguments");
    format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":null,"tool_calls":[{{"id":"{id}","type":"function","function":{{"name":"{name}","arguments":{arguments}}}}}]}}}}],"usage":{{"total_tokens":5}}}}"#
    )
}

fn say(text: &str) -> String {
    format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":"{text}"}}}}],"usage":{{"total_tokens":5}}}}"#
    )
}

// A model server that answers with one scripted body per request and keeps what it was sent.
fn serve(replies: Vec<String>) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let (send, received) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for body in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(rest) = lower.strip_prefix("content-length:") {
                    length = rest.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut request = vec![0u8; length];
            let _ = reader.read_exact(&mut request);
            let _ = send.send(String::from_utf8_lossy(&request).into_owned());
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (format!("http://{address}/v1"), received)
}

fn converge(repo: &Path, user_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(args)
        .current_dir(repo)
        .env("HOME", user_home)
        .env("USERPROFILE", user_home)
        .env("CONVERGE_API_KEY", "test-key")
        .env("CONVERGE_MODEL", "test-model")
        .env_remove("CONVERGE_PROVIDER")
        .env_remove("CONVERGE_CHILD_PERMISSION")
        .output()
        .expect("converge")
}

fn print(repo: &Path, user_home: &Path, base_url: &str, steps: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["-p", "do the task", "--test", "true"])
        .current_dir(repo)
        .env("HOME", user_home)
        .env("USERPROFILE", user_home)
        .env("CONVERGE_BASE_URL", base_url)
        .env("CONVERGE_API_KEY", "test-key")
        .env("CONVERGE_MODEL", "test-model")
        .env("CONVERGE_BUDGET_STEPS", steps)
        .env_remove("CONVERGE_PROVIDER")
        .env_remove("CONVERGE_CHILD_PERMISSION")
        .output()
        .expect("converge")
}

fn walk(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = walk(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

fn session_json(user_home: &Path) -> serde_json::Value {
    let path = walk(&user_home.join(".converge"), "session.json").expect("session.json");
    serde_json::from_slice(&fs::read(path).expect("read")).expect("json")
}

// Plan and workflow are kernel tools, so the model can reach them without the shell.
#[test]
fn plan_and_workflow_tools_reach_the_session_store() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let mode = converge(repo.path(), user_home.path(), &["mode", "work", "plan"]);
    assert!(mode.status.success(), "{:?}", mode);
    let plan_arguments = serde_json::json!({"text": "# Plan\n\nOne step."}).to_string();
    let script = serde_json::json!({
        "version": 1,
        "steps": [{"op": "agent", "id": "alpha", "goal": "fix it", "model": null, "worktree": true}],
    })
    .to_string();
    let workflow_arguments = serde_json::json!({"script": script}).to_string();
    let (base_url, received) = serve(vec![
        call("1", "plan", &plan_arguments),
        call("2", "workflow", &workflow_arguments),
        call("3", "shell", r#"{"command":"git status"}"#),
    ]);
    // The third step is not reached: the budget stops the run and the session stays open.
    let output = print(repo.path(), user_home.path(), &base_url, "3");
    assert_eq!(output.status.code(), Some(1));
    let first = received.recv().expect("first request");
    assert!(first.contains(r#""name":"plan""#), "{first}");
    assert!(first.contains(r#""name":"workflow""#), "{first}");
    assert!(!first.contains(r#""name":"edit""#), "{first}");
    let plan = walk(&user_home.path().join(".converge"), "plan.md").expect("plan.md");
    assert!(fs::read_to_string(plan).unwrap().contains("One step."));
    let workflow = walk(&user_home.path().join(".converge"), "workflow.json").expect("workflow");
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(workflow).expect("read")).expect("json");
    assert_eq!(stored["version"], 1);
    assert_eq!(stored["steps"][0]["op"], "agent");
    // Storing is not running: the child worktree does not exist yet.
    assert!(!user_home.path().join(".converge").join("run").exists());
    assert_eq!(session_json(user_home.path())["work_mode"], "plan");
    let approve = converge(repo.path(), user_home.path(), &["plan", "approve"]);
    assert!(approve.status.success(), "{:?}", approve);
    let after = session_json(user_home.path());
    assert_eq!(after["work_mode"], "dynamic-workflow");
    assert_eq!(after["permission_mode"], "dontAsk");
}

// A lesson is memory only after the completion command exits 0.
#[test]
fn a_lesson_becomes_memory_only_when_the_test_passes() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let (base_url, received) = serve(vec![
        call(
            "1",
            "lesson",
            r#"{"name":"lint-order","symptom":"lint failed after an import","rule":"run lint before the commit","test":"cargo test"}"#,
        ),
        say("done"),
    ]);
    let output = print(repo.path(), user_home.path(), &base_url, "8");
    assert!(output.status.success(), "{:?}", output);
    let first = received.recv().expect("first request");
    assert!(first.contains(r#""name":"lesson""#), "{first}");
    assert!(!first.contains(r#""name":"plan""#), "{first}");
    let index = walk(&user_home.path().join(".converge"), "MEMORY.md").expect("MEMORY.md");
    let memory = fs::read_to_string(index).unwrap();
    assert!(memory.contains("lint-order"), "{memory}");
    let transcript = walk(&user_home.path().join(".converge"), "transcript.jsonl").unwrap();
    let lines = fs::read_to_string(transcript).unwrap();
    assert!(lines.contains("lesson staged"), "{lines}");
}

#[test]
fn compaction_keeps_the_goal_and_a_user_turn() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let workspace = converge::identify(repo.path()).expect("workspace");
    let mut session = converge::create_session(
        &user_home.path().join(".converge"),
        &workspace,
        Some("stay".to_string()),
        converge::Budget {
            steps: Some(9),
            tokens: None,
            seconds: None,
        },
    )
    .expect("session");
    session.goal = Some("stay".to_string());
    let mut messages = vec![serde_json::json!({"role": "system", "content": "prompt"})];
    for index in 0..60 {
        let role = if index % 2 == 0 { "user" } else { "assistant" };
        messages.push(serde_json::json!({"role": role, "content": format!("turn {index}")}));
    }
    converge::compact_messages(&mut messages, &session, "project instructions");
    assert!(messages.len() < 62, "{}", messages.len());
    assert_eq!(messages[0]["role"], "system");
    let summary = messages[1]["content"].as_str().unwrap();
    assert!(summary.contains("stay"), "{summary}");
    assert!(summary.contains("project instructions"), "{summary}");
    assert_eq!(messages[2]["role"], "user");
    assert!(!messages[2]["content"].as_str().unwrap().contains("turn 0"));
}
