use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::home;
use crate::provider;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

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
    process.args(&prefix).arg(command);
    finish(&program, process, timeout)
}

pub fn run_argv(
    cwd: &Path,
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut process = Command::new(program);
    prepare(&mut process, cwd);
    process.args(args);
    finish(program, process, timeout)
}

fn prepare(process: &mut Command, cwd: &Path) {
    process
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("CONVERGE_API_KEY");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
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
    let stdout = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(move || read_pipe(pipe)));
    let stderr = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || read_pipe(pipe)));
    let started = Instant::now();
    let status = loop {
        if started.elapsed() >= timeout || stop_requested() {
            let cancelled = stop_requested();
            kill_tree(&mut child);
            let _ = child.wait();
            let _ = stdout.map(|handle| handle.join());
            let _ = stderr.map(|handle| handle.join());
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
    let stdout = join_pipe(stdout);
    let stderr = join_pipe(stderr);
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

fn read_pipe(mut pipe: impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf);
    buf
}

fn join_pipe(handle: Option<thread::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
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
    if cfg!(windows) && !bash_available() {
        (
            "powershell".to_string(),
            vec!["-NoProfile".to_string(), "-Command".to_string()],
        )
    } else {
        ("bash".to_string(), vec!["-lc".to_string()])
    }
}

fn bash_available() -> bool {
    Command::new("bash")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
