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

#[cfg(unix)]
fn install_backend(user_home: &Path, script: &str) -> std::ffi::OsString {
    use std::os::unix::fs::PermissionsExt;
    let converge_home = user_home.join(".converge");
    fs::create_dir_all(&converge_home).unwrap();
    converge::use_backend(&converge_home, "hermes").unwrap();
    let bin = user_home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let path = bin.join("hermes");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let mut search = std::ffi::OsString::from(&bin);
    search.push(":");
    search.push(std::env::var_os("PATH").unwrap_or_default());
    search
}

#[cfg(unix)]
#[test]
fn a_backend_keeps_its_api_key_and_the_shell_does_not() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let search = install_backend(
        user_home.path(),
        "#!/bin/sh\nprintf 'key=%s\\n' \"$ANTHROPIC_API_KEY\"\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["-p", "hi", "--test", "test -z \"$ANTHROPIC_API_KEY\""])
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("PATH", &search)
        .env("ANTHROPIC_API_KEY", "backend-secret")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transcript = find_transcript(user_home.path());
    assert!(fs::read_to_string(transcript)
        .unwrap()
        .contains("key=backend-secret"));
}

#[cfg(unix)]
fn find_transcript(root: &Path) -> PathBuf {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.ends_with("transcript.jsonl") {
                return path;
            }
        }
    }
    panic!("no transcript under {}", root.display());
}

#[cfg(unix)]
#[test]
fn ctrl_c_on_a_print_run_stops_the_running_command() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let marker = format!("{}", 40000 + std::process::id() % 20000);
    let search = install_backend(user_home.path(), &format!("#!/bin/sh\nsleep {marker}\n"));
    let mut run = Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["-p", "hi", "--test", "true"])
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("PATH", &search)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let running = || {
        Command::new("pgrep")
            .args(["-f", &format!("sleep {marker}")])
            .output()
            .is_ok_and(|output| output.status.success())
    };
    let started = std::time::Instant::now();
    while !running() {
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(Command::new("kill")
        .args(["-INT", &run.id().to_string()])
        .status()
        .unwrap()
        .success());
    assert_eq!(run.wait().unwrap().code(), Some(130));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!running());
}

#[cfg(unix)]
#[test]
fn converge_home_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let repo = git_repo();
    let base = TempDir::new("converge-user");
    let home = base.path().join(".converge");
    let workspace = converge::identify(repo.path()).unwrap();
    converge::create_session(&home, &workspace, None, budget()).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
    converge::load_unfinished(&home, &workspace).unwrap();
    let mode = fs::metadata(&home).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[cfg(unix)]
#[test]
fn a_client_that_stops_reading_does_not_stall_the_others() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    let repo = git_repo();
    let base = PathBuf::from(format!("/tmp/cvs{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let home = base.join(".converge");
    let workspace = converge::identify(repo.path()).unwrap();
    let session = converge::create_session(&home, &workspace, None, budget()).unwrap();
    let server = converge::serve(&home, &session, repo.path()).unwrap();
    let _stuck = UnixStream::connect(&server.socket).unwrap();
    let active = UnixStream::connect(&server.socket).unwrap();
    let mut writer = active.try_clone().unwrap();
    active
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut replies = BufReader::new(active);
    std::thread::sleep(std::time::Duration::from_millis(100));

    let text = "x".repeat(256 * 1024);
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": 7, "method": "session/input", "params": {"text": text},
    });
    writeln!(writer, "{request}").unwrap();
    let started = std::time::Instant::now();
    let mut reply = String::new();
    while !reply.contains("\"id\":7") {
        reply.clear();
        replies.read_line(&mut reply).unwrap();
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(5));

    // A reply goes only to the client that asked.
    let other = UnixStream::connect(&server.socket).unwrap();
    other
        .set_read_timeout(Some(std::time::Duration::from_millis(300)))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    writeln!(
        writer,
        r#"{{"jsonrpc":"2.0","id":8,"method":"session/status","params":{{}}}}"#
    )
    .unwrap();
    reply.clear();
    replies.read_line(&mut reply).unwrap();
    assert!(reply.contains("\"id\":8"), "{reply}");
    let mut overheard = String::new();
    assert!(BufReader::new(other).read_line(&mut overheard).is_err());
    drop(server);
    let _ = fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn sandbox_keeps_commands_out_of_secrets_weights_hooks_and_the_socket() {
    if !converge::sandbox_available() {
        assert!(
            std::env::var_os("CONVERGE_EXPECT_SANDBOX").is_none(),
            "this CI job must have a sandbox"
        );
        return;
    }
    let repo = git_repo();
    fs::create_dir(repo.path().join("weights")).unwrap();
    fs::write(repo.path().join("weights/m.bin"), "weights").unwrap();
    // Short, because a socket path has a low length limit.
    let base = PathBuf::from(format!("/tmp/cvb{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let home = base.join(".converge");
    let workspace = converge::identify(repo.path()).unwrap();
    let session = converge::create_session(&home, &workspace, None, budget()).unwrap();
    fs::write(home.join(".env"), "KEY=sk-secret\n").unwrap();
    let server = converge::serve(&home, &session, repo.path()).unwrap();
    assert!(server.socket.exists());
    // Split, so these reach the sandbox rather than stop at the text check.
    let split = format!("p='{}/.conv'; q='erge'", base.display());

    let secret =
        converge::run_admitted_shell(&session, &format!("{split}; cat \"$p$q/.env\"")).unwrap();
    assert_ne!(secret.status, 0);
    assert!(!secret.stdout.contains("sk-secret"));

    let socket = server.socket.file_name().unwrap().to_string_lossy();
    let connect = format!(
        "{split}; python3 -c 'import socket,sys; socket.socket(socket.AF_UNIX).connect(sys.argv[1])' \"$p$q/run/{socket}\""
    );
    assert_ne!(
        converge::run_admitted_shell(&session, &connect)
            .unwrap()
            .status,
        0
    );

    let weights = converge::run_admitted_shell(&session, "cd wei*s; rm m.bin").unwrap();
    assert_ne!(weights.status, 0);
    assert!(repo.path().join("weights/m.bin").exists());

    let hook =
        converge::run_admitted_shell(&session, "cd .git/hooks; echo x > pre-commit").unwrap();
    assert_ne!(hook.status, 0);
    assert!(!repo.path().join(".git/hooks/pre-commit").exists());

    let normal =
        converge::run_admitted_shell(&session, "echo fine > normal.txt; git status").unwrap();
    assert_eq!(normal.status, 0, "{}", normal.stderr);
    assert!(repo.path().join("normal.txt").exists());

    // The commit hook still runs inside the sandbox, and still blocks on a failing lint.
    for setting in [["user.name", "t"], ["user.email", "t@example.com"]] {
        Command::new("git")
            .arg("config")
            .args(setting)
            .current_dir(repo.path())
            .status()
            .unwrap();
    }
    converge::install_hook(repo.path(), Path::new(env!("CARGO_BIN_EXE_converge"))).unwrap();
    let lint = repo.path().join("lint");
    fs::write(&lint, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&lint, fs::Permissions::from_mode(0o755)).unwrap();
    let commit = "git add -A && git commit -m agent";
    let blocked = converge::run_admitted_shell(&session, commit).unwrap();
    assert_ne!(blocked.status, 0);
    assert!(blocked.stderr.contains("lint failed"), "{}", blocked.stderr);
    fs::write(&lint, "#!/bin/sh\nexit 0\n").unwrap();
    let passed = converge::run_admitted_shell(&session, commit).unwrap();
    assert_eq!(passed.status, 0, "{}", passed.stderr);
    drop(server);
    let _ = fs::remove_dir_all(&base);
}
