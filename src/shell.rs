use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(cwd: &Path, command: &str) -> Result<CommandOutput> {
    if command.is_empty() {
        return Err(Error::new("command is empty"));
    }
    let (program, prefix) = shell_invocation();
    let mut process = Command::new(&program);
    process
        .args(&prefix)
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .env_remove("CONVERGE_API_KEY");
    let output = process
        .output()
        .map_err(|error| Error::new(format!("{program} failed to start: {error}")))?;
    Ok(CommandOutput {
        status: output.status.code().unwrap_or(1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
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
