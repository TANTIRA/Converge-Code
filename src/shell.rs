use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::home;
use crate::provider;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_CAPTURE: usize = 1024 * 1024;
// A background child can hold the pipes open for as long as it lives, so wait only briefly.
const PIPE_GRACE: Duration = Duration::from_millis(500);

pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(cwd: &Path, command: &str) -> Result<CommandOutput> {
    run_timeout(cwd, command, DEFAULT_TIMEOUT)
}

pub fn run_timeout(cwd: &Path, command: &str, timeout: Duration) -> Result<CommandOutput> {
    if command.is_empty() {
        return Err(Error::new("command is empty"));
    }
    let (program, prefix) = shell_invocation();
    let mut process = Command::new(&program);
    prepare(&mut process, cwd);
    remove_provider_keys(&mut process);
    process.args(&prefix).arg(command);
    finish(&program, process, timeout)
}

pub fn run_argv(
    cwd: &Path,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut process = match find_on_windows_path(program) {
        Some(found) => Command::new(found),
        None => Command::new(program),
    };
    prepare(&mut process, cwd);
    process.args(args);
    finish(program, process, timeout)
}

fn prepare(process: &mut Command, cwd: &Path) {
    process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
}

// Only the model's shell loses the keys. A backend CLI is the user's and needs its own.
fn remove_provider_keys(process: &mut Command) {
    process.env_remove("CONVERGE_API_KEY");
    if let Ok(home) = home::converge_home() {
        for name in provider::secret_envs(&home) {
            process.env_remove(name);
        }
    }
}

fn finish(program: &str, mut process: Command, timeout: Duration) -> Result<CommandOutput> {
    if timeout.is_zero() {
        return Err(Error::new("command timed out"));
    }
    let mut child = process.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::new(format!("{program} is not installed"))
        } else {
            Error::new(format!("{program} failed to start: {error}"))
        }
    })?;
    let stdout = child.stdout.take().map(capture);
    let stderr = child.stderr.take().map(capture);
    let started = Instant::now();
    let status = loop {
        if started.elapsed() >= timeout || stop_requested() {
            let cancelled = stop_requested();
            kill_tree(&mut child);
            let _ = child.wait();
            if cancelled {
                return Err(Error::new("command cancelled"));
            }
            return Err(Error::new("command timed out"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                kill_tree(&mut child);
                let _ = child.wait();
                return Err(Error::new(format!("{program} failed: {error}")));
            }
        }
    };
    let stdout = collect(stdout);
    let stderr = collect(stderr);
    Ok(CommandOutput {
        status: status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

static STOP: AtomicBool = AtomicBool::new(false);

pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

pub fn clear_stop() {
    STOP.store(false, Ordering::SeqCst);
}

pub(crate) fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

struct Capture {
    bytes: Arc<Mutex<Vec<u8>>>,
    eof: mpsc::Receiver<()>,
}

// Keeps draining past the cap, so the writer does not die on a closed pipe.
fn capture(mut pipe: impl Read + Send + 'static) -> Capture {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let (done, eof) = mpsc::channel::<()>();
    let shared = Arc::clone(&bytes);
    thread::spawn(move || {
        let _done = done;
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    let mut kept = shared.lock().expect("capture");
                    let room = MAX_CAPTURE.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..count.min(room)]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    Capture { bytes, eof }
}

fn collect(capture: Option<Capture>) -> Vec<u8> {
    let Some(capture) = capture else {
        return Vec::new();
    };
    let _ = capture.eof.recv_timeout(PIPE_GRACE);
    let bytes = std::mem::take(&mut *capture.bytes.lock().expect("capture"));
    bytes
}

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        kill(-(child.id() as i32), 9);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn shell_invocation() -> (String, Vec<String>) {
    if !cfg!(windows) {
        return ("bash".to_string(), vec!["-lc".to_string()]);
    }
    match find_on_windows_path("bash") {
        Some(bash) => (bash.to_string_lossy().into_owned(), vec!["-lc".to_string()]),
        None => (
            "powershell".to_string(),
            vec!["-NoProfile".to_string(), "-Command".to_string()],
        ),
    }
}

// Windows process creation appends only .exe and checks System32 first, where bash.exe only starts WSL.
fn find_on_windows_path(name: &str) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| {
            let dir = dir.to_string_lossy().to_ascii_lowercase();
            !dir.ends_with("system32") && !dir.ends_with("windowsapps")
        })
        .flat_map(|dir| ["exe", "cmd", "bat"].map(|ext| dir.join(name).with_extension(ext)))
        .find(|candidate| candidate.is_file())
}
