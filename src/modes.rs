use std::fs;

use crate::error::{Error, Result};
use crate::session::{self, Session};
use crate::shell;

pub fn cycle_permission(current: &str) -> &'static str {
    match current {
        "ask-each" => "accept-edits",
        "accept-edits" => "plan",
        _ => "ask-each",
    }
}

pub fn approve_plan(session: &mut Session) -> Result<()> {
    let included = session.dir.join("workflow.json").is_file();
    session.work_mode = if included {
        "dynamic-workflow".to_string()
    } else {
        "single-agent".to_string()
    };
    session::save(session)
}

pub fn reject_plan(session: &mut Session) -> Result<()> {
    session.work_mode = "plan".to_string();
    session::save(session)
}

pub fn write_plan(session: &Session, text: &str) -> Result<()> {
    if session.work_mode != "plan" {
        return Err(Error::new("a plan is written only in plan mode"));
    }
    let path = session.dir.join("plan.md");
    fs::write(&path, text).map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

pub fn compact_window(goal: Option<&str>, instructions: &str, transcript: &str) -> String {
    let mut kept = String::new();
    if let Some(goal) = goal {
        kept.push_str("goal:\n");
        kept.push_str(goal);
        kept.push('\n');
    }
    if let Some(test) = transcript
        .lines()
        .rev()
        .find(|line| line.contains("\"kind\":\"test\"") || line.contains("\"kind\": \"test\""))
    {
        kept.push_str("test:\n");
        kept.push_str(test);
        kept.push('\n');
    }
    if !instructions.is_empty() {
        kept.push_str("instructions:\n");
        kept.push_str(instructions);
        kept.push('\n');
    }
    kept
}

pub fn admit(session: &Session, name: &str, arguments: &str, tty: bool) -> Result<()> {
    if session.permission_mode == "bypass" {
        return crate::windows_isolation::laptop_bypass().map(|_| ());
    }
    if session.work_mode == "debug" && session.source_closed && name == "edit" {
        return Err(Error::new("debug source writes are closed"));
    }
    if session.work_mode == "ask" && name == "edit" {
        return Err(Error::new("ask mode does not edit source"));
    }
    if session.work_mode == "plan" && name == "edit" {
        return Err(Error::new("plan mode does not edit source"));
    }
    if name == "shell" {
        let command = command_argument(arguments);
        if session.work_mode == "ask" && !read_only(&command) {
            return Err(Error::new("ask mode does not change state"));
        }
        if shell_targets_session(&command) {
            return Err(Error::new("the shell cannot change the session goal"));
        }
        if session.workspace.join("weights").is_dir() && writes_weights(&command) {
            return Err(Error::new(
                "weight bytes cannot be written outside PromoteWeightEdit",
            ));
        }
    }
    if !tty && would_ask(session, name) {
        return Err(Error::new("permission denied without a tty"));
    }
    Ok(())
}

pub fn run_admitted_shell(session: &Session, command: &str) -> Result<shell::CommandOutput> {
    let arguments = format!(
        "{{\"command\":{}}}",
        serde_json::to_string(command).unwrap_or_default()
    );
    admit(session, "shell", &arguments, false)?;
    shell::run(&session.workspace, command)
}

fn would_ask(session: &Session, name: &str) -> bool {
    match session.permission_mode.as_str() {
        "dontAsk" | "auto" => false,
        "accept-edits" => name == "shell",
        _ => name == "edit" || name == "shell",
    }
}

fn read_only(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || has_shell_syntax(trimmed) {
        return false;
    }
    trimmed == "pwd"
        || whole_command(trimmed, "ls")
        || whole_command(trimmed, "git status")
        || whole_command(trimmed, "git diff")
        || whole_command(trimmed, "git log")
        || whole_command(trimmed, "cat")
}

fn whole_command(command: &str, name: &str) -> bool {
    command == name || command.starts_with(&format!("{name} "))
}

fn has_shell_syntax(command: &str) -> bool {
    command.chars().any(|ch| {
        matches!(
            ch,
            '\n' | '\r' | '`' | '$' | ';' | '&' | '|' | '<' | '>' | '(' | ')' | '\\' | '!' | '#'
        )
    })
}

fn shell_targets_session(command: &str) -> bool {
    command.contains(".converge")
}

fn writes_weights(command: &str) -> bool {
    if !(command.contains("weights/") || command.contains("weights\\")) {
        return false;
    }
    !read_only(command)
}

fn command_argument(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("command")
                .and_then(|command| command.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

pub fn set_goal_text(session: &mut Session, goal: Option<String>) -> Result<()> {
    session.goal = goal;
    session::save(session)
}
