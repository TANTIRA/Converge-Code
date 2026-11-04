use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    dir
}

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git")
}

fn commit_all(repo: &Path, message: &str) -> bool {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message]).status.success()
}

fn head_count(repo: &Path) -> usize {
    let output = git(repo, &["rev-list", "--count", "HEAD"]);
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

fn budget() -> converge::Budget {
    converge::Budget {
        steps: None,
        tokens: None,
        seconds: None,
    }
}

#[test]
fn commit_hook_blocks_user_and_agent_when_lint_fails() {
    let repo = git_repo();
    fs::write(repo.path().join("file.txt"), "one").unwrap();
    assert!(commit_all(repo.path(), "first"));
    assert_eq!(head_count(repo.path()), 1);
    fs::write(repo.path().join("lint"), "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(repo.path().join("lint"))
            .unwrap()
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(repo.path().join("lint"), permissions).unwrap();
    }
    converge::install_hook(repo.path(), Path::new(env!("CARGO_BIN_EXE_converge"))).unwrap();
    fs::write(repo.path().join("file.txt"), "two").unwrap();
    assert!(!commit_all(repo.path(), "user"));
    assert_eq!(head_count(repo.path()), 1);
    let agent = converge::run_shell(repo.path(), "git add -A && git commit -m agent").unwrap();
    assert_ne!(agent.status, 0);
    assert_eq!(head_count(repo.path()), 1);
}

#[test]
fn child_worktree_keeps_the_parent_clean_and_single_agent_rejects_spawn() {
    let repo = git_repo();
    fs::write(repo.path().join("file.txt"), "one").unwrap();
    assert!(commit_all(repo.path(), "first"));
    let dest = repo.path().parent().unwrap().join("child-worktree");
    let _ = fs::remove_dir_all(&dest);
    converge::add_worktree(repo.path(), &dest, "child-branch").unwrap();
    fs::write(dest.join("child.txt"), "from the child").unwrap();
    assert_eq!(converge::parent_status(repo.path()).unwrap().trim(), "");
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let session =
        converge::create_session(home.path(), &workspace, Some("goal".into()), budget()).unwrap();
    assert!(converge::spawn_child(&session, repo.path(), &dest.join("nope"), "other").is_err());
    let child = converge::identify(&dest).unwrap();
    assert!(converge::load_unfinished(home.path(), &child)
        .unwrap()
        .is_none());
    assert!(converge::in_worktree(&dest).unwrap());
    let _ = fs::remove_dir_all(&dest);
}

#[test]
fn chat_keeps_turns_and_the_goal_until_slash_goal() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let mut child = Command::new(env!("CARGO_BIN_EXE_converge"))
        .arg("chat")
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("USERPROFILE", user_home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"/goal keep this\nchange the goal\n\x1b\n\x1b[Z\n/quit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("keep this"));
    assert!(stdout.contains("change the goal"));
    assert!(stdout.contains("cancelled"));
    assert!(stdout.contains("accept-edits"));
    let sessions = user_home.path().join(".converge").join("projects");
    let record = find_file(&sessions, "session.json");
    let text = fs::read_to_string(record).unwrap();
    assert!(text.contains("keep this"));
    assert!(text.contains("accept-edits"));
    let shell = converge::run_shell(repo.path(), "echo converge-ok").unwrap();
    let rendered = converge::format_turn("change the goal", Some("shell"), Some(&shell.stdout));
    assert!(rendered.contains("change the goal"));
    assert!(rendered.contains("tool shell"));
    assert!(rendered.contains("converge-ok"));
}

#[cfg(unix)]
#[test]
fn two_clients_share_one_session_socket() {
    let repo = git_repo();
    fs::write(repo.path().join("file.txt"), "changed\n").unwrap();
    let home = PathBuf::from(format!(
        "/tmp/c{}{}",
        std::process::id(),
        COUNTER.load(Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join(".converge")).unwrap();
    let workspace = converge::identify(repo.path()).unwrap();
    let session =
        converge::create_session(&home.join(".converge"), &workspace, None, budget()).unwrap();
    let server = converge::serve(&home.join(".converge"), &session, repo.path()).unwrap();
    thread::sleep(Duration::from_millis(50));
    let mut first = UnixStream::connect(&server.socket).unwrap();
    let mut second = UnixStream::connect(&server.socket).unwrap();
    thread::sleep(Duration::from_millis(50));
    first
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        first,
        "{}\n",
        r#"{"jsonrpc":"2.0","id":1,"method":"session/input","params":{"text":"hello from a"}}"#
    )
    .unwrap();
    let left = read_line(&mut first);
    let right = read_line(&mut second);
    assert!(left.contains("hello from a") || right.contains("hello from a"));
    let transcript = fs::read_to_string(session.dir.join("transcript.jsonl")).unwrap();
    assert!(transcript.contains("hello from a"));
    write!(
        second,
        "{}\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"session/diff","params":{}}"#
    )
    .unwrap();
    let diff = read_until(&mut second, "diff");
    assert!(diff.contains("diff"));
    write!(
        first,
        "{}\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"kernel/weight","params":{}}"#
    )
    .unwrap();
    let unknown = read_until(&mut first, "unknown");
    assert!(unknown.contains("unknown method"));
    assert!(converge::allowed_methods().contains(&"session/attach"));
    assert!(!converge::allowed_methods().contains(&"kernel/weight"));
    drop(server);
    let _ = fs::remove_dir_all(&home);
}

#[test]
fn modes_goal_plan_and_instructions() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let mut session =
        converge::create_session(home.path(), &workspace, Some("stay".into()), budget()).unwrap();
    converge::set_work_mode(&mut session, "ask").unwrap();
    assert!(converge::admit(
        &session,
        "edit",
        r#"{"path":"a","old":"","new":"b"}"#,
        false
    )
    .is_err());
    assert!(converge::run_admitted_shell(&session, "touch file.txt").is_err());
    assert!(converge::run_admitted_shell(&session, "git status").is_ok());
    converge::set_work_mode(&mut session, "plan").unwrap();
    let permission = session.permission_mode.clone();
    converge::write_plan(&session, "the plan").unwrap();
    assert!(session.dir.join("plan.md").is_file());
    assert!(!repo.path().join("plan.md").exists());
    converge::approve_plan(&mut session).unwrap();
    assert_eq!(session.work_mode, "single-agent");
    assert_eq!(session.permission_mode, permission);
    fs::write(
        session.dir.join("workflow.json"),
        r#"{"version":1,"steps":[]}"#,
    )
    .unwrap();
    converge::set_work_mode(&mut session, "plan").unwrap();
    converge::approve_plan(&mut session).unwrap();
    assert_eq!(session.work_mode, "dynamic-workflow");
    assert_eq!(session.permission_mode, permission);
    converge::reject_plan(&mut session).unwrap();
    assert_eq!(session.work_mode, "plan");
    session.source_closed = true;
    session.work_mode = "debug".to_string();
    assert!(converge::admit(
        &session,
        "edit",
        r#"{"path":"a","old":"","new":"b"}"#,
        false
    )
    .is_err());
    let window = converge::compact_window(
        Some("stay"),
        "project instructions",
        "ancient\n{\"kind\":\"test\",\"status\":1}\n",
    );
    assert!(window.contains("stay"));
    assert!(window.contains("project instructions"));
    assert!(window.contains("\"kind\":\"test\""));
    assert!(!window.contains("ancient"));
    let org = home.path().join("org.md");
    let user = home.path().join("user.md");
    fs::write(&org, "from-org").unwrap();
    fs::write(&user, "from-user").unwrap();
    fs::write(repo.path().join("AGENTS.md"), "from-agents").unwrap();
    fs::write(repo.path().join("CONVERGE.md"), "from-converge").unwrap();
    fs::write(repo.path().join("CONVERGE.local.md"), "from-local").unwrap();
    let text = converge::load_scoped(repo.path(), Some(&org), Some(&user)).unwrap();
    assert!(text.find("from-org").unwrap() < text.find("from-user").unwrap());
    assert!(text.find("from-user").unwrap() < text.find("from-converge").unwrap());
    assert!(text.find("from-converge").unwrap() < text.find("from-local").unwrap());
    assert!(!text.contains("from-agents"));
    session.permission_mode = "ask-each".to_string();
    assert!(converge::admit(
        &session,
        "edit",
        r#"{"path":"a","old":"","new":"b"}"#,
        false
    )
    .is_err());
}

#[test]
fn one_mcp_server_enters_the_initial_context() {
    let home = TempDir::new("converge-home");
    fs::write(
        home.path().join("mcp.json"),
        r#"{
            "servers": [
                {"name": "alpha", "tools": [{"name": "alpha.lookup", "description": "find", "read_only": true}, {"name": "bash", "description": "no", "read_only": true}]},
                {"name": "beta", "tools": [{"name": "beta.write", "description": "write", "read_only": false}]}
            ]
        }"#,
    )
    .unwrap();
    let names = converge::context_tool_names(home.path(), "alpha", false).unwrap();
    assert_eq!(names, vec!["alpha.lookup".to_string()]);
    let ask = converge::context_tool_names(home.path(), "beta", true).unwrap();
    assert!(ask.is_empty());
}

#[test]
fn lessons_write_only_after_success_and_revert_keeps_code() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    fs::write(repo.path().join("kept.txt"), "stay").unwrap();
    assert!(converge::write_lesson(
        home.path(),
        &workspace.repo_key,
        "lint",
        "failed",
        "run lint",
        "test",
        1
    )
    .is_err());
    converge::write_lesson(
        home.path(),
        &workspace.repo_key,
        "lint",
        "failed",
        "run lint",
        "test",
        0,
    )
    .unwrap();
    let loaded = converge::load_index(home.path(), &workspace.repo_key).unwrap();
    assert!(loaded.contains("lint"));
    converge::revert_lesson(home.path(), &workspace.repo_key, "lint").unwrap();
    let gone = converge::load_index(home.path(), &workspace.repo_key).unwrap();
    assert!(!gone.contains("lesson: lint"));
    assert_eq!(
        fs::read_to_string(repo.path().join("kept.txt")).unwrap(),
        "stay"
    );
    let mut big = String::from("# Memory\n");
    for index in 0..210 {
        big.push_str(&format!("- lesson: n{index} | s | r | t\n"));
    }
    let index = home
        .path()
        .join("projects")
        .join(&workspace.repo_key)
        .join("memory")
        .join("MEMORY.md");
    fs::write(&index, big).unwrap();
    let capped = converge::load_index(home.path(), &workspace.repo_key).unwrap();
    assert!(capped.lines().count() <= 200);
    assert!(converge::write_learning_skill(repo.path(), 3, "skill").is_err());
}

#[test]
fn workflow_creates_worktrees_and_tasks_stay_off_the_parent() {
    let repo = git_repo();
    fs::write(repo.path().join("file.txt"), "one").unwrap();
    assert!(commit_all(repo.path(), "first"));
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let mut session =
        converge::create_session(home.path(), &workspace, Some("goal".into()), budget()).unwrap();
    assert!(converge::spawn_child(&session, repo.path(), &home.path().join("no"), "nope").is_err());
    fs::write(
        session.dir.join("workflow.json"),
        r#"{"version":1,"steps":[{"op":"parallel","id":"group","goal":"g","model":null,"worktree":true,"children":[{"op":"agent","id":"alpha","goal":"a","model":null,"worktree":true},{"op":"agent","id":"beta","goal":"b","model":null,"worktree":true}]}]}"#,
    )
    .unwrap();
    converge::set_work_mode(&mut session, "ask").unwrap();
    assert!(converge::execute_workflow(&session, repo.path()).is_err());
    converge::set_work_mode(&mut session, "dynamic-workflow").unwrap();
    converge::execute_workflow(&session, repo.path()).unwrap();
    assert!(
        session
            .dir
            .join("worktrees")
            .join("alpha")
            .join(".git")
            .exists()
            || session.dir.join("worktrees").join("alpha").exists()
    );
    assert!(session.dir.join("worktrees").join("beta").exists());
    assert_eq!(converge::parent_status(repo.path()).unwrap().trim(), "");
    let log = fs::read_to_string(session.dir.join("workflow.log")).unwrap();
    assert!(
        log.find("agent alpha").unwrap() < log.find("agent beta").unwrap()
            || log.contains("agent alpha")
    );
    let dest = home.path().join("async-child");
    converge::set_work_mode(&mut session, "multi-task").unwrap();
    let handle =
        converge::start_background(&session, repo.path(), &dest, "async-branch", "background")
            .unwrap();
    assert_eq!(converge::parent_status(repo.path()).unwrap().trim(), "");
    fs::write(dest.join("release"), "go").unwrap();
    handle.join().unwrap();
    let tasks = converge::tasks_text(&session.dir).unwrap();
    assert!(tasks.contains("done"));
    assert!(tasks.contains("background"));
}

#[test]
fn schedule_is_the_same_cli_and_bypass_stays_off_the_laptop() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let status = Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["schedule", "-p", "fix it", "--test", "true"])
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("USERPROFILE", user_home.path())
        .env_remove("CONVERGE_BASE_URL")
        .env_remove("CONVERGE_API_KEY")
        .env_remove("CONVERGE_MODEL")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    assert!(!walk_has(&user_home.path(), "lesson"));
    let bypass = Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["mode", "permission", "bypass"])
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("USERPROFILE", user_home.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!bypass.success());
    assert!(converge::laptop_bypass().is_err());
    if cfg!(target_os = "linux") {
        assert!(converge::isolated_admit().is_ok());
    } else {
        assert!(converge::isolated_admit().is_err());
    }
    if cfg!(windows) {
        assert!(converge::windows_isolation_admit().is_ok());
    } else {
        assert!(converge::windows_isolation_admit().is_err());
    }
    let isolated = Command::new(env!("CARGO_BIN_EXE_converge-isolated"))
        .status()
        .unwrap();
    if cfg!(target_os = "linux") {
        assert!(isolated.success());
    } else {
        assert!(!isolated.success());
    }
    let devin = fs::read_to_string("clients/devin/client.js").unwrap();
    assert!(devin.contains("session/attach"));
    assert!(!devin.contains("PromoteWeightEdit"));
    let editor = fs::read_to_string("clients/vscode/extension.js").unwrap();
    assert!(editor.contains("session/attach"));
    let jetbrains =
        fs::read_to_string("clients/jetbrains/src/main/kotlin/converge/UhpClient.kt").unwrap();
    assert!(jetbrains.contains("session/attach"));
}

#[test]
fn weights_and_mmdp_stay_behind_their_actions() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    fs::create_dir(repo.path().join("weights")).unwrap();
    let path = converge::promote_weight(
        home.path(),
        &workspace,
        &converge::WeightEdit {
            parent: "parent".into(),
            child: "child".into(),
            parent_classification: "restricted".into(),
            child_classification: "restricted".into(),
            uri: "weights/child".into(),
        },
    )
    .unwrap();
    let record = fs::read_to_string(&path).unwrap();
    assert!(record.contains("child"));
    assert!(record.contains("passed"));
    assert!(record.contains("weights/child"));
    assert!(record.contains("lineage"));
    assert!(converge::promote_weight(
        home.path(),
        &workspace,
        &converge::WeightEdit {
            parent: "parent".into(),
            child: "wide".into(),
            parent_classification: "restricted".into(),
            child_classification: "public".into(),
            uri: "weights/wide".into(),
        },
    )
    .is_err());
    converge::abort_weight(&path).unwrap();
    assert!(fs::read_to_string(&path).unwrap().contains("aborted"));
    assert!(converge::edit_file(repo.path(), "weights/model.bin", "", "bytes").is_err());
    let session = converge::create_session(home.path(), &workspace, None, budget()).unwrap();
    assert!(converge::run_admitted_shell(&session, "echo bytes > weights/model.bin").is_err());
    let catalog = home.path().join("catalog");
    assert!(converge::mmdp_put_file(false, &catalog, "table.parquet", b"rows").is_err());
    assert!(!catalog.exists());
    converge::mmdp_put_file(true, &catalog, "table.parquet", b"rows").unwrap();
    assert_eq!(fs::read(catalog.join("table.parquet")).unwrap(), b"rows");
}

#[test]
fn ask_mode_rejects_chained_shell_and_bypass_stays_off_the_session() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).unwrap();
    let mut session = converge::create_session(home.path(), &workspace, None, budget()).unwrap();
    converge::set_work_mode(&mut session, "ask").unwrap();
    for command in ["git status", "git status --short", "pwd"] {
        let arguments = shell_args(command);
        assert!(
            converge::admit(&session, "shell", &arguments, false).is_ok(),
            "{command}"
        );
    }
    for command in [
        "ls; rm -rf .",
        "cat file && true",
        "pwd$(true)",
        "git statusfoo",
    ] {
        let arguments = shell_args(command);
        assert!(
            converge::admit(&session, "shell", &arguments, false).is_err(),
            "{command}"
        );
    }
    let before = fs::read_to_string(session.dir.join("session.json")).unwrap();
    let rejected = converge::set_permission_mode(&mut session, "bypass").unwrap_err();
    assert!(rejected.to_string().contains("bypass is rejected"));
    assert_eq!(session.permission_mode, "dontAsk");
    assert_eq!(
        fs::read_to_string(session.dir.join("session.json")).unwrap(),
        before
    );
    session.permission_mode = "bypass".to_string();
    let denied = converge::admit(&session, "shell", &shell_args("pwd"), true).unwrap_err();
    assert!(denied.to_string().contains("bypass is rejected"));
}

#[test]
fn missing_step_budget_defaults_to_32() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let status = Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["-p", "fix it", "--test", "true"])
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("USERPROFILE", user_home.path())
        .env("CONVERGE_BASE_URL", "http://127.0.0.1:1")
        .env("CONVERGE_API_KEY", "test-key")
        .env("CONVERGE_MODEL", "test-model")
        .env_remove("CONVERGE_BUDGET_STEPS")
        .env_remove("CONVERGE_MCP_SERVER")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    let record = fs::read_to_string(find_file(
        &user_home.path().join(".converge"),
        "session.json",
    ))
    .unwrap();
    assert!(record.contains("\"steps\": 32"));
}

#[test]
fn unknown_mcp_server_is_an_error() {
    let repo = git_repo();
    let user_home = TempDir::new("converge-user");
    let converge_home = user_home.path().join(".converge");
    fs::create_dir_all(&converge_home).unwrap();
    fs::write(
        converge_home.join("mcp.json"),
        r#"{"servers":[{"name":"alpha","tools":[]}]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_converge"))
        .args(["-p", "fix it", "--test", "true"])
        .current_dir(repo.path())
        .env("HOME", user_home.path())
        .env("USERPROFILE", user_home.path())
        .env("CONVERGE_BASE_URL", "http://127.0.0.1:1")
        .env("CONVERGE_API_KEY", "test-key")
        .env("CONVERGE_MODEL", "test-model")
        .env("CONVERGE_MCP_SERVER", "missing")
        .env_remove("CONVERGE_BUDGET_STEPS")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("mcp server missing"));
}

fn shell_args(command: &str) -> String {
    format!(
        "{{\"command\":{}}}",
        serde_json::to_string(command).unwrap()
    )
}

fn find_file(root: &Path, name: &str) -> PathBuf {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|item| item.to_str()) == Some(name) {
                return path;
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    panic!("missing {name}");
}

fn walk_has(root: &Path, needle: &str) -> bool {
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
                .and_then(|item| item.to_str())
                .unwrap_or("");
            if name.to_ascii_lowercase().contains(needle) {
                return true;
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    false
}

#[cfg(unix)]
fn read_line(stream: &mut UnixStream) -> String {
    let mut buf = [0_u8; 4096];
    let count = stream.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..count]).into_owned()
}

#[cfg(unix)]
fn read_until(stream: &mut UnixStream, needle: &str) -> String {
    let mut all = String::new();
    for _ in 0..4 {
        all.push_str(&read_line(stream));
        if all.contains(needle) {
            break;
        }
    }
    all
}
