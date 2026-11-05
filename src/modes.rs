use std::fs;
use std::time::Duration;

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
        if session.work_mode == "plan" && !read_only(&command) {
            return Err(Error::new("plan mode does not change state"));
        }
        if skips_commit_hook(&command) {
            return Err(Error::new("the shell cannot skip the commit hook"));
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
    if session.permission_mode == "auto" {
        crate::checker::allow(&session.workspace, name, arguments)?;
    }
    if !tty && would_ask(session, name) {
        return Err(Error::new("permission denied without a tty"));
    }
    Ok(())
}

pub fn unattended_exit(session: &Session, name: &str, arguments: &str) -> Option<i32> {
    let denied = would_ask(session, name)
        || (session.permission_mode == "auto" && admit(session, name, arguments, false).is_err());
    denied.then_some(1)
}

pub fn run_admitted_shell(session: &Session, command: &str) -> Result<shell::CommandOutput> {
    let arguments = format!(
        "{{\"command\":{}}}",
        serde_json::to_string(command).unwrap_or_default()
    );
    admit(session, "shell", &arguments, false)?;
    run_session_shell(session, command)
}

pub(crate) fn run_session_shell(session: &Session, command: &str) -> Result<shell::CommandOutput> {
    let path = session.dir.join("session.json");
    let snapshot = fs::read(&path).ok();
    let result = shell::run_timeout(&session.workspace, command, shell_timeout(session));
    if let Some(snapshot) = snapshot {
        if record_changed(&path, &snapshot) {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&path, snapshot)
                .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
            return Err(Error::new("the shell cannot change the session goal"));
        }
    }
    result
}

fn shell_timeout(session: &Session) -> Duration {
    match session.budget.seconds {
        Some(limit) => Duration::from_secs(limit.saturating_sub(session.used.seconds)),
        None => shell::DEFAULT_TIMEOUT,
    }
}

fn record_changed(path: &std::path::Path, snapshot: &[u8]) -> bool {
    match fs::read(path) {
        Ok(now) => now != snapshot,
        Err(_) => true,
    }
}

pub(crate) fn would_ask(session: &Session, name: &str) -> bool {
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
    // git accepts abbreviated options: --outp writes a file, --ext runs a configured program.
    if trimmed
        .split_whitespace()
        .any(|word| word.starts_with("--ou") || word.starts_with("--ext"))
    {
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

// shortcut: these checks read the command text, so a glob or a script can get past them; upgrade to an OS sandbox over ~/.converge and weights/.
fn shell_targets_session(command: &str) -> bool {
    command.contains(".converge")
}

fn writes_weights(command: &str) -> bool {
    command.to_ascii_lowercase().contains("weights") && !read_only(command)
}

fn skips_commit_hook(command: &str) -> bool {
    if command.contains("CONVERGE_LINT=") {
        return true;
    }
    if command.contains("core.hooksPath") && command.split_whitespace().any(|word| word == "git") {
        return true;
    }
    without_quoted_text(command)
        .split([';', '&', '|', '\n', '(', ')'])
        .any(|part| {
            let words: Vec<&str> = part.split_whitespace().collect();
            let flags = git_subcommand_args(&words, "commit");
            flags.is_some_and(|flags| flags.iter().any(|word| disables_verify(word)))
        })
}

fn without_quoted_text(command: &str) -> String {
    let mut quote = None;
    command
        .chars()
        .filter(|ch| match quote {
            Some(open) => {
                if *ch == open {
                    quote = None;
                }
                false
            }
            None if matches!(ch, '"' | '\'') => {
                quote = Some(*ch);
                false
            }
            None => true,
        })
        .collect()
}

// The arguments after `git <subcommand>`, past global options such as `-c name=value`.
fn git_subcommand_args<'a>(words: &'a [&'a str], subcommand: &str) -> Option<&'a [&'a str]> {
    let git = words.iter().position(|word| *word == "git")?;
    let mut index = git + 1;
    while let Some(word) = words.get(index) {
        if matches!(*word, "-c" | "-C") {
            index += 2;
        } else if word.starts_with('-') {
            index += 1;
        } else {
            break;
        }
    }
    (words.get(index) == Some(&subcommand)).then(|| &words[index + 1..])
}

fn disables_verify(word: &str) -> bool {
    if word.starts_with("--") {
        return word.starts_with("--no-v");
    }
    // In `-anm`, the letters after m are the message, so only the ones before it are flags.
    word.starts_with('-')
        && word[1..]
            .chars()
            .take_while(|ch| *ch != 'm')
            .any(|ch| ch == 'n')
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
