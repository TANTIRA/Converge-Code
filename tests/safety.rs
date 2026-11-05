use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        Self(path.canonicalize().expect("canonical"))
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
    let dir = TempDir::new("converge-safety");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .expect("git");
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

fn shell(command: &str) -> String {
    serde_json::json!({ "command": command }).to_string()
}

#[cfg(unix)]
#[test]
fn a_background_child_does_not_outlive_the_time_limit() {
    let dir = TempDir::new("converge-shell");
    let started = std::time::Instant::now();
    let output = converge::run_shell_timeout(
        dir.path(),
        "echo before; sleep 20 & exit 0",
        std::time::Duration::from_secs(5),
    )
    .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    assert_eq!(output.stdout, "before\n");
}

#[cfg(unix)]
#[test]
fn captured_output_is_capped() {
    let dir = TempDir::new("converge-shell");
    let output = converge::run_shell(dir.path(), "head -c 3000000 /dev/zero | tr '\\0' a").unwrap();
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.len(), 1024 * 1024);
}

#[cfg(unix)]
#[test]
fn edit_does_not_follow_a_symlink_out_of_the_workspace() {
    let repo = git_repo();
    let outside = TempDir::new("converge-outside");
    std::os::unix::fs::symlink(outside.path(), repo.path().join("link")).unwrap();
    assert!(converge::edit_file(repo.path(), "link/new", "", "x").is_err());
    assert!(!outside.path().join("new").exists());
    std::os::unix::fs::symlink(outside.path().join("gone"), repo.path().join("dangling")).unwrap();
    assert!(converge::edit_file(repo.path(), "dangling", "", "x").is_err());
    assert!(!outside.path().join("gone").exists());
}

#[test]
fn edit_stays_out_of_git_and_weights_in_any_case() {
    let repo = git_repo();
    fs::create_dir(repo.path().join("weights")).unwrap();
    for path in [
        ".git/hooks/pre-commit",
        ".GIT/config",
        "sub/.git/config",
        "weights/m.bin",
        "Weights/m.bin",
    ] {
        assert!(
            converge::edit_file(repo.path(), path, "", "x").is_err(),
            "{path}"
        );
    }
    assert!(converge::edit_file(repo.path(), "src/new.txt", "", "x").is_ok());
}

#[test]
fn shell_guards_close_the_known_holes() {
    let repo = git_repo();
    fs::create_dir(repo.path().join("weights")).unwrap();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let mut session = converge::create_session(home.path(), &workspace, None, budget()).unwrap();
    for command in [
        "cd weights && rm m.bin",
        "git commit --no-verify -m x",
        "git commit -n -m x",
        "git commit -anm x",
        "git -c a=b commit -n -m x",
        "git add . && git commit --no-ver -m x",
        "CONVERGE_LINT=true git commit -m x",
        "git -c core.hooksPath=/tmp commit -m x",
    ] {
        assert!(
            converge::admit(&session, "shell", &shell(command), false).is_err(),
            "{command}"
        );
    }
    for command in [
        "git commit -m x",
        "git commit -m note",
        "git commit -m 'add -n and --no-verify flags'",
        "git commit -m x && ls -n",
        "grep -rn commit src",
        "git log -n 5 --grep commit",
        "grep CONVERGE_LINT src",
        "grep core.hooksPath docs",
        "cargo test",
    ] {
        assert!(
            converge::admit(&session, "shell", &shell(command), false).is_ok(),
            "{command}"
        );
    }

    converge::set_work_mode(&mut session, "ask").unwrap();
    for command in [
        "git log --output=a",
        "git diff --outp=a",
        "git diff --ext-diff",
    ] {
        assert!(
            converge::admit(&session, "shell", &shell(command), false).is_err(),
            "{command}"
        );
    }
    assert!(converge::admit(&session, "shell", &shell("git log --oneline"), false).is_ok());

    converge::set_work_mode(&mut session, "plan").unwrap();
    assert!(converge::admit(&session, "shell", &shell("sed -i s/a/b/ f"), false).is_err());
    assert!(converge::admit(&session, "shell", &shell("git status"), false).is_ok());

    converge::set_work_mode(&mut session, "single-agent").unwrap();
    converge::set_permission_mode(&mut session, "auto").unwrap();
    for command in ["cat ~/.ssh/id_rsa", "cat $HOME/.ssh/id_rsa"] {
        assert!(
            converge::admit(&session, "shell", &shell(command), false).is_err(),
            "{command}"
        );
    }
}

#[test]
fn unattended_run_stops_on_anything_that_would_ask() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let mut session = converge::create_session(home.path(), &workspace, None, budget()).unwrap();
    let edit = r#"{"path":"a","old":"","new":"b"}"#;
    assert_eq!(
        converge::unattended_exit(&session, "shell", &shell("pwd")),
        None
    );
    converge::set_permission_mode(&mut session, "accept-edits").unwrap();
    assert_eq!(converge::unattended_exit(&session, "edit", edit), None);
    assert_eq!(
        converge::unattended_exit(&session, "shell", &shell("pwd")),
        Some(1)
    );
    converge::set_permission_mode(&mut session, "ask-each").unwrap();
    assert_eq!(converge::unattended_exit(&session, "edit", edit), Some(1));
}

#[test]
fn hook_install_keeps_a_hook_it_did_not_write() {
    let repo = git_repo();
    let bin = Path::new(env!("CARGO_BIN_EXE_converge"));
    let hook = converge::install_hook(repo.path(), bin).unwrap();
    assert!(converge::install_hook(repo.path(), bin).is_ok());
    fs::write(&hook, "#!/bin/sh\necho mine\n").unwrap();
    assert!(converge::install_hook(repo.path(), bin).is_err());
    assert_eq!(fs::read_to_string(&hook).unwrap(), "#!/bin/sh\necho mine\n");
    let binary = [0x7f, b'E', b'L', b'F', 0xff, 0xfe];
    fs::write(&hook, binary).unwrap();
    assert!(converge::install_hook(repo.path(), bin).is_err());
    assert_eq!(fs::read(&hook).unwrap(), binary);
    let config = Command::new("git")
        .args(["config", "core.hooksPath"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(config.stdout.is_empty());
}

#[test]
fn damaged_session_files_do_not_lock_the_repo() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let damaged = converge::create_session(home.path(), &workspace, None, budget()).unwrap();
    fs::write(damaged.dir.join("session.json"), "{\"session_id\":").unwrap();
    let kept = converge::create_session(home.path(), &workspace, None, budget()).unwrap();
    fs::write(
        kept.dir.join("transcript.jsonl"),
        "{\"kind\":\"user\",\"text\":\"hi\"}\n{\"kind\":\"tool\",\"na",
    )
    .unwrap();
    let loaded = converge::load_unfinished(home.path(), &workspace)
        .unwrap()
        .expect("the healthy session");
    assert_eq!(loaded.id, kept.id);
}

#[test]
fn a_bigger_budget_continues_an_exhausted_session() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let home = user_home.path().join(".converge");
    let workspace = converge::identify(repo.path()).unwrap();
    let limit = converge::Budget {
        steps: Some(1),
        tokens: None,
        seconds: None,
    };
    let mut session = converge::create_session(&home, &workspace, None, limit).unwrap();
    session.used.steps = 1;
    converge::save_session(&session).unwrap();
    let run = |steps: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_converge"));
        command
            .args(["-p", "again", "--test", "true"])
            .current_dir(repo.path())
            .env("HOME", user_home.path())
            .env("USERPROFILE", user_home.path())
            .env("CONVERGE_BASE_URL", "http://127.0.0.1:1")
            .env("CONVERGE_API_KEY", "test-key")
            .env("CONVERGE_MODEL", "test-model")
            .env_remove("CONVERGE_BUDGET_STEPS")
            .stdin(Stdio::null());
        if let Some(steps) = steps {
            command.env("CONVERGE_BUDGET_STEPS", steps);
        }
        String::from_utf8_lossy(&command.output().unwrap().stderr).into_owned()
    };
    assert!(run(None).contains("budget exhausted"));
    assert!(run(Some("5")).contains("model request failed"));
}

#[test]
fn restored_chat_answers_every_tool_call() {
    let call = |id: &str| serde_json::json!({"id": id, "type": "function", "function": {"name": "shell", "arguments": "{}"}});
    let assistant = |calls: Vec<serde_json::Value>| serde_json::json!({"kind": "assistant", "message": {"role": "assistant", "content": null, "tool_calls": calls}});
    let transcript = [
        serde_json::json!({"kind": "user", "text": "go"}),
        assistant(vec![call("same"), call("denied")]),
        serde_json::json!({"kind": "tool", "name": "shell", "tool_call_id": "same", "text": "first"}),
        serde_json::json!({"kind": "denied", "name": "shell", "tool_call_id": "denied", "text": "shell denied"}),
        assistant(vec![call("same")]),
        serde_json::json!({"kind": "tool", "name": "shell", "text": "old record without an id"}),
    ]
    .map(|record| record.to_string())
    .join("\n");
    let messages = converge::restore_messages(&format!("{transcript}\n{{\"kind\":\"to"));
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        ["user", "assistant", "tool", "tool", "assistant", "tool"]
    );
    assert_eq!(messages[2]["content"], "first");
    assert_eq!(messages[3]["content"], "shell denied");
    assert_eq!(messages[5]["tool_call_id"], "same");
    assert_eq!(messages[5]["content"], "no result was recorded");
}

#[test]
fn chat_frame_wraps_long_lines_and_strips_control_codes() {
    let frame = converge::render_chat_frame(
        8,
        20,
        &[
            "0123456789012345678901234 rm -rf".to_string(),
            "\u{1b}]52;c;x\u{7}safe\r".to_string(),
        ],
        "",
        "status",
    );
    assert!(frame.contains("rm -rf"));
    assert!(!frame.contains('\u{1b}'));
    assert!(!frame.contains('\r'));
    assert!(frame.contains("safe"));
}
