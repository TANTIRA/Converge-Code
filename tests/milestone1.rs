use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("git init");
    assert!(status.success());
    dir
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

#[test]
fn repo_key_is_stable_for_two_spellings() {
    let repo = git_repo();
    let direct = converge::identify(repo.path()).expect("repo");
    let dotted = converge::identify(&repo.path().join(".")).expect("dotted");
    assert_eq!(direct.repo_key, dotted.repo_key);
    assert_eq!(direct.toplevel, dotted.toplevel);

    fs::create_dir(repo.path().join("nested")).expect("nested");
    let through_parent =
        converge::identify(&repo.path().join("nested").join("..")).expect("parent");
    assert_eq!(direct.repo_key, through_parent.repo_key);
    assert_eq!(direct.toplevel, through_parent.toplevel);
}

#[test]
fn edit_and_shell_use_real_files_and_processes() {
    let repo = git_repo();
    converge::edit_file(repo.path(), "hello.txt", "", "hello").expect("create");
    assert_eq!(
        fs::read_to_string(repo.path().join("hello.txt")).expect("read"),
        "hello"
    );
    converge::edit_file(repo.path(), "hello.txt", "hello", "hello converge").expect("replace");
    assert_eq!(
        fs::read_to_string(repo.path().join("hello.txt")).expect("read"),
        "hello converge"
    );
    assert!(converge::edit_file(repo.path(), "../outside.txt", "", "no").is_err());

    let echoed = converge::run_shell(repo.path(), "echo converge-ok").expect("echo");
    assert_eq!(echoed.status, 0);
    assert!(echoed.stdout.contains("converge-ok"));

    let failed = converge::run_shell(repo.path(), "exit 1").expect("exit");
    assert_ne!(failed.status, 0);
    assert!(!converge::stop_allowed(failed.status));
    assert!(converge::stop_allowed(0));
}

#[test]
fn project_instructions_prefer_converge_md() {
    let repo = git_repo();
    fs::write(repo.path().join("AGENTS.md"), "from-agents").expect("agents");
    fs::write(repo.path().join("CONVERGE.md"), "from-converge").expect("converge");
    fs::write(repo.path().join("CONVERGE.local.md"), "from-local").expect("local");
    let text = converge::load_once(repo.path()).expect("instructions");
    assert!(text.contains("from-converge"));
    assert!(text.contains("from-local"));
    assert!(!text.contains("from-agents"));
}

#[test]
fn session_record_stays_outside_the_repo() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let workspace = converge::identify(repo.path()).expect("repo");
    let session = converge::create_session(
        home.path(),
        &workspace,
        Some("keep the tests green".to_string()),
        converge::Budget {
            steps: None,
            tokens: Some(10),
            seconds: None,
        },
    )
    .expect("session");
    let record = fs::read_to_string(session.dir.join("session.json")).expect("record");
    assert!(record.contains("single-agent"));
    assert!(record.contains("dontAsk"));
    assert!(record.contains("keep the tests green"));
    assert!(record.contains(&workspace.toplevel.display().to_string()));
    assert!(!repo.path().join("session.json").exists());
    assert!(!contains_lesson(home.path()));
}

#[test]
fn print_outside_a_git_repo_exits_nonzero() {
    let dir = TempDir::new("converge-not-git");
    let home = TempDir::new("converge-home");
    let status = Command::new(env!("CARGO_BIN_EXE_converge"))
        .current_dir(dir.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .args(["-p", "fix it", "--test", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run");
    assert!(!status.success());
}

#[test]
fn print_without_model_config_exits_nonzero_and_writes_no_lesson() {
    let repo = git_repo();
    let home = TempDir::new("converge-home");
    let status = Command::new(env!("CARGO_BIN_EXE_converge"))
        .current_dir(repo.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env_remove("CONVERGE_BASE_URL")
        .env_remove("CONVERGE_API_KEY")
        .env_remove("CONVERGE_MODEL")
        .args(["-p", "fix it", "--test", "true"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("run");
    assert!(!status.success());
    assert!(!contains_lesson(home.path()));
    assert!(!home.path().join("projects").exists());
}
