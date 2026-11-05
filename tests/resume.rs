use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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
    let status = Command::new("git")
        .arg("init")
        .current_dir(dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git init");
    assert!(status.success());
    dir
}

fn budget() -> converge::Budget {
    converge::Budget {
        steps: None,
        tokens: None,
        seconds: None,
    }
}

fn stamp(path: &Path, when: &str) {
    let status = Command::new("touch")
        .args(["-t", when])
        .arg(path)
        .status()
        .expect("touch");
    assert!(status.success());
}

fn contains_lesson(root: &Path) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if name.to_ascii_lowercase().contains("lesson") {
                return true;
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    false
}

fn case_variant(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let mut chars = name.chars();
    let first = chars.next()?;
    let flipped = if first.is_ascii_uppercase() {
        first.to_ascii_lowercase()
    } else {
        first.to_ascii_uppercase()
    };
    let alt = path.parent()?.join(format!("{flipped}{}", chars.as_str()));
    alt.exists().then_some(alt)
}

#[test]
fn resume_loads_the_same_session_for_two_spellings() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).expect("repo");
    let session = converge::create_session(
        home.path(),
        &workspace,
        Some("keep the goal".to_string()),
        budget(),
    )
    .expect("session");
    fs::write(
        session.dir.join("transcript.jsonl"),
        "{\"kind\":\"user\",\"text\":\"first prompt\"}\n",
    )
    .expect("transcript");
    let id = session.id.clone();
    drop(session);

    let dotted = converge::identify(&repo.path().join(".")).expect("dotted");
    let loaded = converge::load_unfinished(home.path(), &dotted)
        .expect("load")
        .expect("session");
    assert_eq!(loaded.id, id);
    assert_eq!(loaded.goal.as_deref(), Some("keep the goal"));
    let transcript = fs::read_to_string(loaded.dir.join("transcript.jsonl")).expect("transcript");
    assert!(transcript.contains("first prompt"));

    if let Some(alt) = case_variant(repo.path()) {
        let folded = converge::identify(&alt).expect("folded");
        assert_eq!(folded.repo_key, workspace.repo_key);
        let again = converge::load_unfinished(home.path(), &folded)
            .expect("load")
            .expect("session");
        assert_eq!(again.id, id);
    }
}

#[test]
fn resume_skips_a_different_toplevel_and_a_finished_session() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).expect("repo");
    let older = converge::create_session(
        home.path(),
        &workspace,
        Some("older goal".to_string()),
        budget(),
    )
    .expect("older");
    let newer = converge::create_session(
        home.path(),
        &workspace,
        Some("newer goal".to_string()),
        budget(),
    )
    .expect("newer");
    fs::write(
        newer.dir.join("transcript.jsonl"),
        "{\"kind\":\"test\",\"status\":0}\n",
    )
    .expect("passed");
    let sibling = newer.dir.parent().expect("sessions").join("sibling");
    fs::create_dir_all(&sibling).expect("sibling");
    fs::write(
        sibling.join("session.json"),
        r#"{
  "session_id": "sibling",
  "workspace": "/no/such/converge/toplevel",
  "work_mode": "single-agent",
  "permission_mode": "dontAsk",
  "goal": "somewhere else",
  "budget": {"steps": null, "tokens": null, "seconds": null}
}"#,
    )
    .expect("sibling record");
    stamp(&older.dir.join("session.json"), "202001010101.01");
    stamp(&newer.dir.join("session.json"), "202001010101.02");
    stamp(&sibling.join("session.json"), "203001010101.03");

    let loaded = converge::load_unfinished(home.path(), &workspace)
        .expect("load")
        .expect("older session");
    assert_eq!(loaded.id, older.id);
    assert_eq!(loaded.goal.as_deref(), Some("older goal"));

    fs::write(
        older.dir.join("transcript.jsonl"),
        "{\"kind\":\"test\",\"status\":0}\n",
    )
    .expect("older passed");
    assert!(converge::load_unfinished(home.path(), &workspace)
        .expect("load")
        .is_none());
    let created = converge::create_session(home.path(), &workspace, None, budget()).expect("new");
    assert_ne!(created.id, older.id);
    assert_ne!(created.id, newer.id);
}

#[test]
fn resume_keeps_a_stored_goal_until_the_user_replaces_it() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).expect("repo");
    let mut session = converge::create_session(
        home.path(),
        &workspace,
        Some("keep the goal".to_string()),
        budget(),
    )
    .expect("session");
    let loaded = converge::load_unfinished(home.path(), &workspace)
        .expect("load")
        .expect("session");
    assert_eq!(loaded.goal.as_deref(), Some("keep the goal"));
    converge::set_goal(&mut session, "user replaced it".to_string()).expect("goal");
    let replaced = converge::load_unfinished(home.path(), &workspace)
        .expect("load")
        .expect("session");
    assert_eq!(replaced.goal.as_deref(), Some("user replaced it"));
    assert!(!repo.path().join("session.json").exists());
}

#[test]
fn exhausted_resume_exits_nonzero_without_a_new_session_or_lesson() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user-home");
    let home = user_home.path().join(".converge");
    fs::create_dir_all(&home).expect("converge home");
    let workspace = converge::identify(repo.path()).expect("repo");
    let mut session = converge::create_session(
        &home,
        &workspace,
        Some("keep me".to_string()),
        converge::Budget {
            steps: Some(1),
            tokens: None,
            seconds: None,
        },
    )
    .expect("session");
    session.used.steps = 1;
    converge::save_session(&session).expect("used");
    let loaded = converge::load_unfinished(&home, &workspace)
        .expect("load")
        .expect("stored session");
    assert_eq!(loaded.used.steps, 1);
    assert_eq!(loaded.budget.steps, Some(1));
    fs::write(
        session.dir.join("transcript.jsonl"),
        "{\"kind\":\"user\",\"text\":\"first prompt\"}\n",
    )
    .expect("transcript");
    let id = session.id.clone();

    let status = Command::new(env!("CARGO_BIN_EXE_converge"))
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("USERPROFILE", user_home.path())
        .env("CONVERGE_BASE_URL", "http://127.0.0.1:1")
        .env("CONVERGE_API_KEY", "test-key")
        .env("CONVERGE_MODEL", "test-model")
        .args(["-p", "again", "--test", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run");
    assert!(!status.success());

    let sessions = home
        .join("projects")
        .join(&workspace.repo_key)
        .join("sessions");
    let ids: Vec<_> = fs::read_dir(&sessions)
        .expect("sessions")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(ids.len(), 1);
    let record = fs::read_to_string(sessions.join(&id).join("session.json")).expect("record");
    assert!(record.contains("keep me"));
    assert!(record.contains(&id));
    let transcript =
        fs::read_to_string(sessions.join(&id).join("transcript.jsonl")).expect("transcript");
    assert!(transcript.contains("first prompt"));
    assert!(transcript.contains("again"));
    assert!(!contains_lesson(user_home.path()));
}

#[cfg(windows)]
#[test]
fn windows_shell_runs_the_selected_shell() {
    let repo = git_repo();
    let echoed = converge::run_shell(repo.path(), "echo converge-ok").expect("echo");
    assert_eq!(echoed.status, 0);
    assert!(echoed.stdout.contains("converge-ok"));
    let failed = converge::run_shell(repo.path(), "exit 1").expect("exit");
    assert_ne!(failed.status, 0);
    assert!(!converge::stop_allowed(failed.status));
}

#[cfg(windows)]
#[test]
fn edit_waits_out_an_exclusive_windows_lock() {
    let repo = git_repo();
    let file = repo.path().join("hello.txt");
    fs::write(&file, "hello").expect("seed");
    let ready = repo.path().join("lock-ready");
    // The hold sits inside the edit retry window, so the write lands on the same path.
    let mut child = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "$fs = [System.IO.File]::Open($env:CONVERGE_LOCK_PATH, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None); Set-Content -LiteralPath $env:CONVERGE_LOCK_READY -Value ready; Start-Sleep -Milliseconds 400; $fs.Close()",
        ])
        .env("CONVERGE_LOCK_PATH", &file)
        .env("CONVERGE_LOCK_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("powershell");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "exclusive lock did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    converge::edit_file(repo.path(), "hello.txt", "hello", "hello converge").expect("edit");
    let _ = child.wait();
    assert_eq!(fs::read_to_string(&file).expect("read"), "hello converge");
}
