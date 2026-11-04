use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};
use crate::git;
use crate::home;
use crate::modes;
use crate::session::{self, Budget, Session};

static CANCEL: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_sigint(_: i32) {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn install_cancel_handler() {
    #[cfg(unix)]
    libc_signal(2, handle_sigint);
}

#[cfg(unix)]
fn libc_signal(sig: i32, handler: extern "C" fn(i32)) {
    unsafe extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> extern "C" fn(i32);
    }
    unsafe {
        signal(sig, handler);
    }
}

pub fn format_turn(user: &str, tool_name: Option<&str>, tool_text: Option<&str>) -> String {
    let mut out = String::new();
    if !user.is_empty() {
        out.push_str(user);
        out.push('\n');
    }
    if let (Some(name), Some(text)) = (tool_name, tool_text) {
        out.push_str("tool ");
        out.push_str(name);
        out.push('\n');
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub fn run_chat(cwd: &Path) -> Result<i32> {
    install_cancel_handler();
    let workspace = git::identify(cwd)?;
    let converge_home = home::converge_home()?;
    let mut session = match session::load_unfinished(&converge_home, &workspace)? {
        Some(session) => session,
        None => session::create(
            &converge_home,
            &workspace,
            None,
            Budget {
                steps: None,
                tokens: None,
                seconds: None,
            },
        )?,
    };
    if session.permission_mode == "dontAsk" {
        session::set_permission_mode(&mut session, "ask-each")?;
    }
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        if CANCEL.swap(false, Ordering::SeqCst) {
            return Ok(130);
        }
        let line = line.map_err(|error| Error::new(format!("stdin: {error}")))?;
        if line == "/quit" {
            return Ok(0);
        }
        if line == "\u{1b}" {
            writeln!(stdout, "cancelled").map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if line == "\u{1b}[Z" {
            let next = modes::cycle_permission(&session.permission_mode);
            session::set_permission_mode(&mut session, next)?;
            writeln!(stdout, "{next}").map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("/goal") {
            handle_goal(&mut session, rest.trim(), &mut stdout)?;
            continue;
        }
        if line == "/tasks" {
            let text = crate::workflow::tasks_text(&session.dir).unwrap_or_default();
            writeln!(stdout, "{text}").map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("/plan ") {
            handle_plan(&mut session, rest.trim(), &mut stdout)?;
            continue;
        }
        let rendered = format_turn(&line, None, None);
        write!(stdout, "{rendered}").map_err(|error| Error::new(error.to_string()))?;
        session::append(&session, &serde_json::json!({"kind": "user", "text": line}))?;
    }
    Ok(0)
}

fn handle_goal(session: &mut Session, rest: &str, stdout: &mut impl Write) -> Result<()> {
    if rest.is_empty() {
        writeln!(stdout, "{}", session.goal.as_deref().unwrap_or(""))
            .map_err(|error| Error::new(error.to_string()))?;
        return Ok(());
    }
    if rest == "clear" {
        modes::set_goal_text(session, None)?;
        writeln!(stdout, "cleared").map_err(|error| Error::new(error.to_string()))?;
        return Ok(());
    }
    modes::set_goal_text(session, Some(rest.to_string()))?;
    writeln!(stdout, "{rest}").map_err(|error| Error::new(error.to_string()))?;
    Ok(())
}

fn handle_plan(session: &mut Session, action: &str, stdout: &mut impl Write) -> Result<()> {
    match action {
        "approve" => modes::approve_plan(session)?,
        "reject" | "keep" => modes::reject_plan(session)?,
        _ => return Err(Error::new("plan action is approve, reject, or keep")),
    }
    writeln!(stdout, "{}", session.work_mode).map_err(|error| Error::new(error.to_string()))?;
    Ok(())
}
