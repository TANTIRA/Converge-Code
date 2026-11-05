#![cfg(unix)]

use std::ffi::CStr;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

unsafe extern "C" {
    fn posix_openpt(flags: i32) -> i32;
    fn grantpt(fd: i32) -> i32;
    fn unlockpt(fd: i32) -> i32;
    fn ptsname(fd: i32) -> *const std::ffi::c_char;
}

const O_RDWR: i32 = 2;

fn open_terminal() -> (File, File) {
    unsafe {
        let master = posix_openpt(O_RDWR);
        assert!(master >= 0);
        assert_eq!(grantpt(master), 0);
        assert_eq!(unlockpt(master), 0);
        let name = CStr::from_ptr(ptsname(master))
            .to_str()
            .unwrap()
            .to_string();
        let replica = File::options().read(true).write(true).open(name).unwrap();
        (File::from_raw_fd(master), replica)
    }
}

fn find(root: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        }
    }
    None
}

fn eventually(what: &str, check: impl Fn() -> bool) {
    let started = Instant::now();
    while !check() {
        assert!(started.elapsed() < Duration::from_secs(5), "{what}");
        thread::sleep(Duration::from_millis(20));
    }
}

// A real terminal delivers an escape sequence or a paste as one burst of bytes.
#[test]
fn keys_that_arrive_together_are_all_read() {
    let base = std::env::temp_dir().join(format!("converge-pty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let home = base.join("home");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let (mut master, replica) = open_terminal();
    let mut drain = master.try_clone().unwrap();
    thread::spawn(move || {
        let mut sink = [0u8; 4096];
        while matches!(drain.read(&mut sink), Ok(count) if count > 0) {}
    });
    let stdin = Stdio::from(OwnedFd::from(replica.try_clone().unwrap()));
    let stdout = Stdio::from(OwnedFd::from(replica));
    let mut chat = Command::new(env!("CARGO_BIN_EXE_converge"))
        .current_dir(&repo)
        .env("HOME", &home)
        .env_remove("CONVERGE_BASE_URL")
        .env_remove("CONVERGE_PROVIDER")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let record = || find(&home, "session.json");
    eventually("session starts", || record().is_some());
    let mode = || fs::read_to_string(record().unwrap()).unwrap_or_default();
    eventually("chat asks each", || mode().contains("ask-each"));
    let users = || -> Vec<String> {
        let Some(path) = find(&home, "transcript.jsonl") else {
            return Vec::new();
        };
        fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| value["kind"] == "user")
            .map(|value| value["text"].as_str().unwrap_or("").to_string())
            .collect()
    };

    thread::sleep(Duration::from_millis(150));
    master.write_all(b"\x1b[Z").unwrap();
    eventually("shift+tab cycles the mode", || {
        mode().contains("accept-edits")
    });

    master.write_all(b"hello ").unwrap();
    thread::sleep(Duration::from_millis(50));
    master.write_all(b"\x1b[A").unwrap();
    thread::sleep(Duration::from_millis(50));
    master.write_all(b"world\r").unwrap();
    eventually("an arrow key keeps the line", || {
        users().contains(&"hello world".to_string())
    });

    master
        .write_all("caf\u{e9} \u{65e5}\u{672c}\r".as_bytes())
        .unwrap();
    eventually("text is not limited to ASCII", || {
        users().contains(&"caf\u{e9} \u{65e5}\u{672c}".to_string())
    });

    master.write_all(b"typo").unwrap();
    thread::sleep(Duration::from_millis(50));
    master.write_all(b"\x1b").unwrap();
    thread::sleep(Duration::from_millis(80));
    master.write_all(b"pasted\r").unwrap();
    eventually("a lone escape clears, a paste arrives whole", || {
        users().contains(&"pasted".to_string())
    });

    master.write_all(b"/quit\r").unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = chat.try_wait().unwrap() {
            break status;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "/quit");
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
    let _ = fs::remove_dir_all(&base);
}

// The background child uses a backend CLI, so this never calls a real model.
#[test]
fn a_background_task_runs_a_child_and_reports_on_a_later_turn() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!("converge-pty-task-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let home = base.join("home");
    let bin = base.join("bin");
    for dir in [&repo, &home, &bin] {
        fs::create_dir_all(dir).unwrap();
    }
    let git = |args: &[&str]| {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
            .status
            .success());
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    converge::use_backend(&home.join(".converge"), "hermes").unwrap();
    let script = bin.join("hermes");
    fs::write(&script, "#!/bin/sh\nprintf child > child.txt\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let command = |stdin: Stdio, stdout: Stdio| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_converge"));
        command
            .current_dir(&repo)
            .env("HOME", &home)
            .env("PATH", &path)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::null());
        command
    };
    assert!(command(Stdio::null(), Stdio::null())
        .args(["mode", "work", "multi-task"])
        .status()
        .unwrap()
        .success());

    let (mut master, replica) = open_terminal();
    let mut drain = master.try_clone().unwrap();
    thread::spawn(move || {
        let mut sink = [0u8; 4096];
        while matches!(drain.read(&mut sink), Ok(count) if count > 0) {}
    });
    let stdin = Stdio::from(OwnedFd::from(replica.try_clone().unwrap()));
    let stdout = Stdio::from(OwnedFd::from(replica));
    let mut chat = command(stdin, stdout).spawn().unwrap();
    thread::sleep(Duration::from_millis(300));

    master
        .write_all(b"/task say hi --test test -f child.txt\r")
        .unwrap();
    let tasks = || {
        find(&home, "tasks.json")
            .and_then(|path| fs::read_to_string(path).ok())
            .unwrap_or_default()
    };
    eventually("the task starts", || tasks().contains("say hi"));
    let started = Instant::now();
    while !tasks().contains("\"done\"") {
        assert!(started.elapsed() < Duration::from_secs(20), "{}", tasks());
        thread::sleep(Duration::from_millis(50));
    }
    assert!(!repo.join("child.txt").exists());

    master.write_all(b"next message\r").unwrap();
    // The child has its own session, so read the transcript beside the parent's tasks.json.
    let parent = find(&home, "tasks.json")
        .unwrap()
        .with_file_name("transcript.jsonl");
    eventually("the result enters a later turn", || {
        fs::read_to_string(&parent)
            .unwrap_or_default()
            .contains("Background task task-1 finished with status done")
    });
    master.write_all(b"/quit\r").unwrap();
    let started = Instant::now();
    while chat.try_wait().unwrap().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "/quit");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(&base);
}

// A real chat answers a question written the way a child writes it.
#[test]
fn chat_shows_a_child_question_with_its_command_and_answers_it() {
    let base = std::env::temp_dir().join(format!("converge-pty-ask-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let home = base.join("home");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&home).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let (mut master, replica) = open_terminal();
    let mut output = master.try_clone().unwrap();
    let screen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&screen);
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(count) = output.read(&mut chunk) {
            if count == 0 {
                break;
            }
            seen.lock().unwrap().extend_from_slice(&chunk[..count]);
        }
    });
    let stdin = Stdio::from(OwnedFd::from(replica.try_clone().unwrap()));
    let stdout = Stdio::from(OwnedFd::from(replica));
    let mut chat = Command::new(env!("CARGO_BIN_EXE_converge"))
        .current_dir(&repo)
        .env("HOME", &home)
        .env_remove("CONVERGE_BASE_URL")
        .env_remove("CONVERGE_PROVIDER")
        .stdin(stdin)
        .stdout(stdout)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    eventually("session starts", || find(&home, "session.json").is_some());
    let asks = converge::asks_dir(find(&home, "session.json").unwrap().parent().unwrap());
    thread::sleep(Duration::from_millis(200));

    let ask = |reply: &'static [u8], master: &mut File| {
        let dir = asks.clone();
        let child = thread::spawn(move || {
            converge::child_ask(&dir, "alpha", "shell", r#"{"command":"rm -rf build"}"#).unwrap()
        });
        let shown = || String::from_utf8_lossy(&screen.lock().unwrap()).contains("allow shell?");
        eventually("the prompt appears", shown);
        master.write_all(reply).unwrap();
        thread::sleep(Duration::from_millis(100));
        master.write_all(b"\r").unwrap();
        let answer = child.join().unwrap();
        let text = String::from_utf8_lossy(&screen.lock().unwrap()).into_owned();
        assert!(text.contains("child alpha"), "{text}");
        assert!(text.contains("rm -rf build"), "{text}");
        screen.lock().unwrap().clear();
        answer
    };
    assert_eq!(ask(b"y", &mut master), converge::Answer::Allow);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(ask(b"n", &mut master), converge::Answer::Deny);

    master.write_all(b"/quit\r").unwrap();
    let started = Instant::now();
    while chat.try_wait().unwrap().is_none() {
        assert!(started.elapsed() < Duration::from_secs(5), "/quit");
        thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(&base);
}
