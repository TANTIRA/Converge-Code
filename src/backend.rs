use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::git;
use crate::modes;
use crate::session::{self, Session};
use crate::shell;

const NAMES: &[&str] = &["converge", "hermes", "codex", "grokbot", "claude"];

pub fn selected(home: &Path) -> Result<String> {
    let path = home.join("backend.json");
    if !path.is_file() {
        return Ok("converge".to_string());
    }
    let bytes =
        fs::read(&path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    known(&name)?;
    Ok(name)
}

pub fn use_backend(home: &Path, name: &str) -> Result<()> {
    known(name)?;
    fs::create_dir_all(home)
        .map_err(|error| Error::new(format!("create {}: {error}", home.display())))?;
    let path = home.join("backend.json");
    let bytes = serde_json::to_vec_pretty(&json!({"name": name}))
        .map_err(|error| Error::new(format!("encode backend: {error}")))?;
    fs::write(&path, bytes)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

pub fn reply(home: &Path, cwd: &Path, prompt: &str) -> Result<String> {
    let name = selected(home)?;
    if name == "converge" {
        return Err(Error::new("backend converge uses the loop"));
    }
    let output = shell::run_argv(
        cwd,
        program(&name),
        &arguments(&name, prompt),
        shell::DEFAULT_TIMEOUT,
    )
    .map_err(|error| {
        if error.message.contains("is not installed") {
            Error::new(format!("{name} is not installed"))
        } else {
            error
        }
    })?;
    if output.status != 0 {
        let detail = format!("{}{}", output.stdout, output.stderr);
        return Err(Error::new(format!(
            "{name} exited with status {}\n{detail}",
            output.status
        )));
    }
    Ok(output.stdout)
}

pub fn run_external(
    cwd: &Path,
    home: &Path,
    prompt: &str,
    test_command: &str,
    goal: Option<String>,
) -> Result<i32> {
    let name = selected(home)?;
    let text = reply(home, cwd, prompt).map_err(|error| {
        if error.message.contains("is not installed") {
            Error::new(format!("{name} is not installed"))
        } else {
            error
        }
    })?;
    let workspace = git::identify(cwd)?;
    let mut session = match session::load_unfinished(home, &workspace)? {
        Some(mut existing) => {
            if let Some(goal) = goal {
                session::set_goal(&mut existing, goal)?;
            }
            existing
        }
        None => session::create(home, &workspace, goal, session::budget_from_env()?)?,
    };
    session::append(&session, &json!({"kind": "user", "text": prompt}))?;
    let message = json!({"role": "assistant", "content": text});
    session::append(&session, &json!({"kind": "assistant", "message": message}))?;
    finish_test(&mut session, test_command)
}

fn finish_test(session: &mut Session, test_command: &str) -> Result<i32> {
    let output = modes::run_session_shell(session, test_command)?;
    session::append(
        session,
        &json!({
            "kind": "test",
            "status": output.status,
            "stdout": output.stdout,
            "stderr": output.stderr,
        }),
    )?;
    if output.status == 0 {
        Ok(0)
    } else {
        Ok(output.status)
    }
}

fn known(name: &str) -> Result<()> {
    if NAMES.contains(&name) {
        Ok(())
    } else {
        Err(Error::new(format!("backend {name} is not configured")))
    }
}

fn program(name: &str) -> &str {
    match name {
        "grokbot" => "grok",
        other => other,
    }
}

fn arguments(name: &str, prompt: &str) -> Vec<String> {
    match name {
        "hermes" => vec!["-z".to_string(), prompt.to_string()],
        "claude" | "grokbot" => vec!["-p".to_string(), prompt.to_string()],
        "codex" => vec!["exec".to_string(), prompt.to_string()],
        _ => Vec::new(),
    }
}
