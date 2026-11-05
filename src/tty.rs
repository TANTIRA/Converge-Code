use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::git;
use crate::home;
use crate::modes;
use crate::session::{self, Session};
#[cfg(unix)]
use crate::termios::{tcgetattr, tcsetattr, zero_termios, Termios};

static CANCEL: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_sigint(_: i32) {
    CANCEL.store(true, Ordering::SeqCst);
    crate::shell::request_stop();
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
    let mut session = session::open(&converge_home, &workspace)?;
    if session.permission_mode == "dontAsk" {
        session::set_permission_mode(&mut session, "ask-each")?;
    }
    let _clients = crate::uhp::serve(&converge_home, &session, &workspace.toplevel)?;
    crate::uhp::publish(
        "session/status",
        serde_json::json!({
            "work_mode": session.work_mode,
            "permission_mode": session.permission_mode,
        }),
    );
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    if stdin.is_terminal() && stdout.is_terminal() {
        return run_screen(&workspace, &converge_home, session);
    }
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
            crate::uhp::publish(
                "session/status",
                serde_json::json!({
                    "work_mode": session.work_mode,
                    "permission_mode": session.permission_mode,
                }),
            );
            writeln!(stdout, "{next}").map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("/goal") {
            let shown = goal_reply(&mut session, rest.trim())?;
            writeln!(stdout, "{shown}").map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if line == "/tasks" {
            writeln!(stdout, "{}", tasks_reply(&session))
                .map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("/plan ") {
            let shown = plan_reply(&mut session, rest.trim())?;
            writeln!(stdout, "{shown}").map_err(|error| Error::new(error.to_string()))?;
            continue;
        }
        if crate::backend::selected(&converge_home)? != "converge" {
            session::append(
                &session,
                &serde_json::json!({"kind": "user", "text": &line}),
            )?;
            crate::uhp::publish("session/stream", serde_json::json!({"text": &line}));
            let text = crate::backend::reply(&converge_home, &workspace.toplevel, &line)?;
            writeln!(stdout, "{text}").map_err(|error| Error::new(error.to_string()))?;
            crate::uhp::publish("session/stream", serde_json::json!({"text": &text}));
            let message = serde_json::json!({"role": "assistant", "content": text});
            session::append(
                &session,
                &serde_json::json!({"kind": "assistant", "message": message}),
            )?;
            continue;
        }
        let rendered = format_turn(&line, None, None);
        write!(stdout, "{rendered}").map_err(|error| Error::new(error.to_string()))?;
        session::append(
            &session,
            &serde_json::json!({"kind": "user", "text": &line}),
        )?;
        crate::uhp::publish("session/stream", serde_json::json!({"text": line}));
    }
    Ok(0)
}

fn goal_reply(session: &mut Session, rest: &str) -> Result<String> {
    if rest.is_empty() {
        return Ok(session.goal.clone().unwrap_or_default());
    }
    if rest == "clear" {
        modes::set_goal_text(session, None)?;
        return Ok("cleared".to_string());
    }
    modes::set_goal_text(session, Some(rest.to_string()))?;
    Ok(rest.to_string())
}

fn tasks_reply(session: &Session) -> String {
    crate::workflow::tasks_text(&session.dir).unwrap_or_default()
}

fn task_reply(session: &Session, rest: &str) -> String {
    let Some((goal, test)) = rest.split_once(" --test ") else {
        return "usage: /task <goal> --test <command>".to_string();
    };
    let started = std::env::current_exe()
        .map_err(|error| Error::new(error.to_string()))
        .and_then(|bin| {
            crate::workflow::start_background(
                session,
                &session.workspace,
                &bin,
                goal.trim(),
                test.trim(),
            )
        });
    match started {
        Ok(_) => format!("started in the background: {}", goal.trim()),
        Err(error) => error.message,
    }
}

fn merge_reply(session: &Session, rest: &str) -> String {
    let Some((id, test)) = rest.split_once(" --test ") else {
        return "usage: /merge <id> --test <command>".to_string();
    };
    let merged = home::converge_home().and_then(|home| {
        let workspace = git::identify(&session.workspace)?;
        crate::workflow::merge_child(&home, &workspace, id.trim(), test.trim())
    });
    match merged {
        Ok(text) => text,
        Err(error) => error.message,
    }
}

fn plan_reply(session: &mut Session, action: &str) -> Result<String> {
    match action {
        "approve" => modes::approve_plan(session)?,
        "reject" | "keep" => modes::reject_plan(session)?,
        _ => return Err(Error::new("plan action is approve, reject, or keep")),
    }
    Ok(session.work_mode.clone())
}

pub fn raw_console_modes(input: u32, output: u32) -> (u32, u32) {
    const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
    const ENABLE_LINE_INPUT: u32 = 0x0002;
    const ENABLE_ECHO_INPUT: u32 = 0x0004;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
    let input = (input & !ENABLE_PROCESSED_INPUT & !ENABLE_LINE_INPUT & !ENABLE_ECHO_INPUT)
        | ENABLE_VIRTUAL_TERMINAL_INPUT;
    let output = output | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
    (input, output)
}

pub fn render_chat_frame(
    rows: usize,
    cols: usize,
    transcript: &[String],
    input: &str,
    status: &str,
) -> String {
    let rows = rows.max(3);
    let cols = cols.max(20);
    let body_rows = rows - 2;
    let flat: Vec<String> = transcript
        .iter()
        .flat_map(|line| line.split('\n'))
        .flat_map(|line| wrap(line, cols))
        .collect();
    let start = flat.len().saturating_sub(body_rows);
    let mut lines = Vec::with_capacity(rows);
    for row in 0..body_rows {
        lines.push(flat.get(start + row).cloned().unwrap_or_default());
    }
    lines.push(fit(&format!("> {input}"), cols));
    lines.push(fit(status, cols));
    lines.join("\n")
}

fn fit(text: &str, cols: usize) -> String {
    printable(text).take(cols).collect()
}

fn wrap(text: &str, cols: usize) -> Vec<String> {
    let chars: Vec<char> = printable(text).collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(cols)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

pub(crate) fn printable_text(text: &str) -> String {
    text.split('\n')
        .map(|line| printable(line).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

// Model and tool text must not reach the terminal as control codes.
fn printable(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars()
        .map(|ch| if ch == '\t' { ' ' } else { ch })
        .filter(|ch| !ch.is_control())
}

fn run_screen(workspace: &crate::git::Workspace, home: &Path, mut session: Session) -> Result<i32> {
    #[cfg(unix)]
    let _raw = RawTerm::enable()?;
    #[cfg(windows)]
    let _console = windows_screen::Console::enable()?;
    let _screen = AltScreen::enter();
    let instructions = crate::instructions::load_scoped(
        &workspace.toplevel,
        Some(&crate::instructions::organization_instructions()),
        Some(&crate::instructions::user_instructions(home)),
    )?;
    let mut view = load_view(&session);
    view.extend(crate::sandbox::missing_warning().map(str::to_string));
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": chat_prompt(&instructions, session.goal.as_deref()),
    })];
    extend_messages(&mut messages, &session);
    let mut input = String::new();
    let mut model = crate::model::OpenAiCompatible::open(home).ok();
    let mut dirty = true;
    loop {
        if CANCEL.swap(false, Ordering::SeqCst) {
            return Ok(130);
        }
        if dirty {
            let status = status_line(&session, home, model.is_some());
            paint(&render_chat_frame(
                window_size().0,
                window_size().1,
                &view,
                &input,
                &status,
            ))?;
            dirty = false;
        }
        // shortcut: a child waits while the parent's own turn runs; upgrade by polling inside chat_reply too.
        if answer_children(&session, home, &mut view)? {
            dirty = true;
            continue;
        }
        if let Some(line) = crate::uhp::poll_input() {
            dirty = true;
            if let Some(code) = submit_line(
                &mut session,
                home,
                &mut model,
                &mut messages,
                &mut view,
                line,
                true,
            )? {
                return Ok(code);
            }
            continue;
        }
        if !wait_key(50)? {
            continue;
        }
        dirty = true;
        match read_key()? {
            Key::Char(ch) => input.push(ch),
            Key::Backspace => {
                input.pop();
            }
            Key::Esc => input.clear(),
            Key::Ignore => {}
            Key::ShiftTab => {
                let next = modes::cycle_permission(&session.permission_mode);
                session::set_permission_mode(&mut session, next)?;
                crate::uhp::publish(
                    "session/status",
                    serde_json::json!({
                        "work_mode": session.work_mode,
                        "permission_mode": session.permission_mode,
                    }),
                );
            }
            Key::CtrlC => return Ok(130),
            Key::CtrlD => {
                if input.is_empty() {
                    return Ok(0);
                }
            }
            Key::Enter => {
                let line = std::mem::take(&mut input);
                if let Some(code) = submit_line(
                    &mut session,
                    home,
                    &mut model,
                    &mut messages,
                    &mut view,
                    line,
                    false,
                )? {
                    return Ok(code);
                }
            }
        }
    }
}

fn answer_children(session: &Session, home: &Path, view: &mut Vec<String>) -> Result<bool> {
    let dir = crate::child_ask::asks_dir(&session.dir);
    let asks = crate::child_ask::pending(&dir);
    for ask in &asks {
        view.push(format!("child {}:", ask.child));
        let allowed = confirm_tool(&ask.name, &ask.arguments, view, session, home)?;
        let answer = if allowed {
            crate::child_ask::Answer::Allow
        } else {
            crate::child_ask::Answer::Deny
        };
        view.push(format!(
            "{} {}",
            ask.name,
            if allowed { "allowed" } else { "denied" }
        ));
        crate::child_ask::answer(&dir, &ask.id, answer)?;
    }
    Ok(!asks.is_empty())
}

fn submit_line(
    session: &mut Session,
    home: &Path,
    model: &mut Option<crate::model::OpenAiCompatible>,
    messages: &mut Vec<serde_json::Value>,
    view: &mut Vec<String>,
    line: String,
    recorded: bool,
) -> Result<Option<i32>> {
    if line == "/quit" {
        return Ok(Some(0));
    }
    if line.is_empty() {
        return Ok(None);
    }
    if let Some(shown) = command_line(session, &line)? {
        view.push(shown.clone());
        crate::uhp::publish(
            "session/status",
            serde_json::json!({
                "work_mode": session.work_mode,
                "permission_mode": session.permission_mode,
                "text": shown,
            }),
        );
        return Ok(None);
    }
    for finished in crate::workflow::take_finished(&session.dir) {
        view.push(finished.clone());
        session::append(
            session,
            &serde_json::json!({"kind": "user", "text": &finished}),
        )?;
        messages.push(serde_json::json!({"role": "user", "content": finished}));
    }
    view.push(format!("you  {line}"));
    if !recorded {
        session::append(session, &serde_json::json!({"kind": "user", "text": &line}))?;
        crate::uhp::publish("session/stream", serde_json::json!({"text": &line}));
    }
    messages.push(serde_json::json!({"role": "user", "content": line}));
    let reply = chat_reply(session, home, model, messages, view)?;
    if reply == 130 {
        let exit = CANCEL.swap(false, Ordering::SeqCst);
        crate::shell::clear_stop();
        if exit {
            return Ok(Some(130));
        }
    }
    Ok(None)
}

fn command_line(session: &mut Session, line: &str) -> Result<Option<String>> {
    if let Some(rest) = line.strip_prefix("/goal") {
        return goal_reply(session, rest.trim()).map(Some);
    }
    if line == "/tasks" {
        return Ok(Some(tasks_reply(session)));
    }
    if let Some(rest) = line.strip_prefix("/merge ") {
        return Ok(Some(merge_reply(session, rest)));
    }
    if let Some(rest) = line.strip_prefix("/task ") {
        return Ok(Some(task_reply(session, rest)));
    }
    if let Some(rest) = line.strip_prefix("/plan ") {
        return plan_reply(session, rest.trim()).map(Some);
    }
    Ok(None)
}

fn chat_prompt(instructions: &str, goal: Option<&str>) -> String {
    let mut prompt = String::from(
        "You are Converge. Edit files with the edit tool and run commands with the shell tool. A chat turn is not complete because you say so.\n",
    );
    if let Some(goal) = goal {
        prompt.push_str("\nGoal:\n");
        prompt.push_str(goal);
        prompt.push('\n');
    }
    if !instructions.is_empty() {
        prompt.push_str("\nProject instructions:\n");
        prompt.push_str(instructions);
    }
    prompt
}

fn status_line(session: &Session, home: &Path, ready: bool) -> String {
    let provider = crate::provider::resolve(home)
        .map(|resolved| format!("{} {}", resolved.profile.name, resolved.profile.model))
        .unwrap_or_else(|_| "no provider".to_string());
    let mark = if ready {
        provider
    } else {
        "no provider".to_string()
    };
    format!(
        "{}  {}  {mark}  /quit",
        session.work_mode, session.permission_mode
    )
}

fn read_transcript(session: &Session) -> String {
    let bytes = std::fs::read(session.dir.join("transcript.jsonl")).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn load_view(session: &Session) -> Vec<String> {
    let text = read_transcript(session);
    let mut lines = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("kind").and_then(|kind| kind.as_str()) {
            Some("user") => lines.push(format!(
                "you  {}",
                value
                    .get("text")
                    .and_then(|text| text.as_str())
                    .unwrap_or("")
            )),
            Some("assistant") => {
                let content = value
                    .pointer("/message/content")
                    .map(value_text)
                    .unwrap_or_default();
                if !content.is_empty() {
                    lines.push(content);
                }
            }
            Some("tool") => lines.push(format!(
                "tool {}\n{}",
                value
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or(""),
                value
                    .get("text")
                    .and_then(|text| text.as_str())
                    .unwrap_or("")
            )),
            _ => {}
        }
    }
    lines
}

fn extend_messages(messages: &mut Vec<serde_json::Value>, session: &Session) {
    messages.extend(restore_messages(&read_transcript(session)));
}

pub fn restore_messages(transcript: &str) -> Vec<serde_json::Value> {
    let records: Vec<serde_json::Value> = transcript
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let kind = |record: &serde_json::Value| {
        record
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut messages = Vec::new();
    for (index, record) in records.iter().enumerate() {
        match kind(record).as_str() {
            "user" => messages.push(serde_json::json!({
                "role": "user",
                "content": record.get("text").and_then(|text| text.as_str()).unwrap_or("")
            })),
            "assistant" => {
                let Some(message) = record.get("message") else {
                    continue;
                };
                messages.push(message.clone());
                let calls = message.get("tool_calls").and_then(|calls| calls.as_array());
                for call in calls.into_iter().flatten() {
                    let id = call.get("id").and_then(|id| id.as_str()).unwrap_or("");
                    // Some providers reuse ids between turns, so stop at the next model turn.
                    let answer = records[index + 1..]
                        .iter()
                        .take_while(|later| kind(later) != "assistant")
                        .find(|later| {
                            later.get("tool_call_id").and_then(|id| id.as_str()) == Some(id)
                        })
                        .and_then(|later| later.get("text"))
                        .and_then(|text| text.as_str())
                        .unwrap_or("no result was recorded");
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": answer,
                    }));
                }
            }
            _ => {}
        }
    }
    messages
}

fn last_user_text(messages: &[serde_json::Value]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(|role| role.as_str()) == Some("user"))
        .and_then(|message| message.get("content").and_then(|content| content.as_str()))
        .unwrap_or("")
        .to_string()
}

fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn watching<T>(body: impl FnOnce() -> T) -> T {
    #[cfg(unix)]
    let _watch = WatchKeys::start();
    body()
}

#[cfg(unix)]
static WATCH: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
struct WatchKeys;

#[cfg(unix)]
impl WatchKeys {
    fn start() -> Self {
        WATCH.store(true, Ordering::SeqCst);
        thread::spawn(|| {
            while WATCH.load(Ordering::SeqCst) {
                if CANCEL.load(Ordering::SeqCst) {
                    crate::shell::request_stop();
                    break;
                }
                if poll_stdin(50).unwrap_or(false) {
                    let Ok(key) = read_key() else {
                        break;
                    };
                    match key {
                        Key::Esc => crate::shell::request_stop(),
                        Key::CtrlC => {
                            CANCEL.store(true, Ordering::SeqCst);
                            crate::shell::request_stop();
                        }
                        _ => {}
                    }
                }
            }
        });
        Self
    }
}

#[cfg(unix)]
impl Drop for WatchKeys {
    fn drop(&mut self) {
        WATCH.store(false, Ordering::SeqCst);
    }
}

pub fn wait_for_turn<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<Option<T>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(work());
    });
    loop {
        if crate::shell::stop_requested() || CANCEL.load(Ordering::SeqCst) {
            return Ok(None);
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(value) => {
                if crate::shell::stop_requested() || CANCEL.load(Ordering::SeqCst) {
                    return Ok(None);
                }
                return Ok(Some(value));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Error::new("model request failed"));
            }
        }
    }
}

fn chat_reply(
    session: &mut Session,
    home: &Path,
    model: &mut Option<crate::model::OpenAiCompatible>,
    messages: &mut Vec<serde_json::Value>,
    view: &mut Vec<String>,
) -> Result<i32> {
    if crate::backend::selected(home)? != "converge" {
        let prompt = last_user_text(messages);
        let text = crate::backend::reply(home, &session.workspace, &prompt)?;
        view.push(text.clone());
        crate::uhp::publish("session/stream", serde_json::json!({"text": &text}));
        let message = serde_json::json!({"role": "assistant", "content": text});
        messages.push(message.clone());
        session::append(
            session,
            &serde_json::json!({"kind": "assistant", "message": message}),
        )?;
        return Ok(0);
    }
    if model.is_none() {
        *model = crate::model::OpenAiCompatible::open(home).ok();
    }
    if model.is_none() {
        let text = "no provider is configured".to_string();
        view.push(text.clone());
        session::append(session, &serde_json::json!({"kind": "note", "text": text}))?;
        return Ok(0);
    }
    crate::shell::clear_stop();
    // Chat stays open for days, so its budget covers one message, not the whole session.
    let started = Instant::now();
    let mut steps = 0;
    let mut tokens = 0u64;
    loop {
        if CANCEL.load(Ordering::SeqCst) || crate::shell::stop_requested() {
            view.push("cancelled".to_string());
            return Ok(130);
        }
        if crate::run::exhausted(&session.budget, steps, tokens, started.elapsed().as_secs()) {
            view.push("budget exhausted for this message".to_string());
            session::append(
                session,
                &serde_json::json!({"kind": "budget", "exhausted": true}),
            )?;
            return Ok(0);
        }
        let Some(active) = model.take() else {
            let text = "no provider is configured".to_string();
            view.push(text.clone());
            session::append(session, &serde_json::json!({"kind": "note", "text": text}))?;
            return Ok(0);
        };
        let owned = messages.clone();
        let finished = watching(|| {
            wait_for_turn(move || {
                let mut active = active;
                let tools = crate::model::admitted_tools();
                let result = active.complete(&owned, &tools);
                (active, result)
            })
        })?;
        let Some((active, result)) = finished else {
            view.push("cancelled".to_string());
            return Ok(130);
        };
        *model = Some(active);
        let turn = match result {
            Ok(turn) => turn,
            Err(error) => {
                view.push(error.message.clone());
                let message = serde_json::json!({"role": "assistant", "content": error.message});
                messages.push(message.clone());
                session::append(
                    session,
                    &serde_json::json!({"kind": "assistant", "message": message}),
                )?;
                return Ok(0);
            }
        };
        steps += 1;
        tokens = tokens.saturating_add(turn.total_tokens);
        messages.push(turn.assistant_message.clone());
        session::append(
            session,
            &serde_json::json!({"kind": "assistant", "message": turn.assistant_message}),
        )?;
        let content = value_text(
            turn.assistant_message
                .get("content")
                .unwrap_or(&serde_json::Value::Null),
        );
        if !content.is_empty() {
            view.push(content.clone());
            crate::uhp::publish("session/stream", serde_json::json!({"text": content}));
        }
        if turn.tool_calls.is_empty() {
            return Ok(0);
        }
        for call in turn.tool_calls {
            if modes::would_ask(session, &call.name) {
                let allowed = confirm_tool(&call.name, &call.arguments, view, session, home)?;
                if !allowed {
                    let text = format!("{} denied", call.name);
                    view.push(text.clone());
                    crate::uhp::publish(
                        "session/tool",
                        serde_json::json!({"name": call.name, "text": &text}),
                    );
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": call.id,
                        "content": text,
                    }));
                    session::append(
                        session,
                        &serde_json::json!({
                            "kind": "denied",
                            "name": call.name,
                            "tool_call_id": call.id,
                            "text": text,
                        }),
                    )?;
                    continue;
                }
            }
            let result =
                watching(|| crate::run::dispatch(session, &call.name, &call.arguments, true));
            view.push(format!("tool {}\n{result}", call.name));
            crate::uhp::publish(
                "session/tool",
                serde_json::json!({"name": call.name, "text": &result}),
            );
            if call.name == "edit" {
                crate::uhp::publish_diff(&session.workspace);
            }
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": &result,
            }));
            session::append(
                session,
                &serde_json::json!({
                    "kind": "tool",
                    "name": call.name,
                    "tool_call_id": call.id,
                    "text": &result,
                }),
            )?;
        }
    }
}

// shortcut: a call taller than the screen shows only its end; upgrade when the view can scroll.
pub(crate) fn describe_call(name: &str, arguments: &str) -> String {
    let parsed: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let field = |key: &str| parsed.get(key).and_then(|value| value.as_str());
    match (name, field("command"), field("path")) {
        ("shell", Some(command), _) => command.to_string(),
        ("edit", _, Some(path)) => format!(
            "{path}\n- {}\n+ {}",
            field("old").unwrap_or(""),
            field("new").unwrap_or("")
        ),
        _ => arguments.to_string(),
    }
}

fn confirm_tool(
    name: &str,
    arguments: &str,
    view: &mut Vec<String>,
    session: &Session,
    home: &Path,
) -> Result<bool> {
    let call = describe_call(name, arguments);
    crate::uhp::publish_ask(name, &call);
    view.push(format!("{name} asks to run:\n{call}"));
    let prompt = format!("allow {name}? type y and Enter, anything else denies: ");
    let mut answer = String::new();
    let mut dirty = true;
    loop {
        if let Some(allow) = crate::uhp::poll_permission() {
            return Ok(allow);
        }
        if dirty {
            let (rows, cols) = window_size();
            paint(&render_chat_frame(
                rows,
                cols,
                view,
                &format!("{prompt}{answer}"),
                &status_line(session, home, true),
            ))?;
            dirty = false;
        }
        if !wait_key(50)? {
            continue;
        }
        dirty = true;
        match read_key()? {
            Key::Esc => return Ok(false),
            Key::CtrlC => {
                CANCEL.store(true, Ordering::SeqCst);
                return Ok(false);
            }
            Key::Backspace => {
                answer.pop();
            }
            Key::Char(ch) => answer.push(ch),
            Key::Enter => return Ok(answer.eq_ignore_ascii_case("y")),
            Key::ShiftTab | Key::CtrlD | Key::Ignore => {}
        }
    }
}

fn paint(frame: &str) -> Result<()> {
    let mut out = String::from("\u{1b}[H");
    for (index, line) in frame.lines().enumerate() {
        let row = index + 1;
        if index + 1 == frame.lines().count() {
            out.push_str(&format!("\u{1b}[{row};1H\u{1b}[2K\u{1b}[7m{line}\u{1b}[0m"));
        } else {
            out.push_str(&format!("\u{1b}[{row};1H\u{1b}[2K{line}"));
        }
    }
    let rows = frame.lines().count();
    let input = frame.lines().nth(rows.saturating_sub(2)).unwrap_or("");
    let column = input.chars().count() + 1;
    out.push_str(&format!("\u{1b}[{};{}H", rows.saturating_sub(1), column));
    let mut stdout = io::stdout();
    write!(stdout, "{out}").map_err(|error| Error::new(error.to_string()))?;
    stdout
        .flush()
        .map_err(|error| Error::new(error.to_string()))?;
    Ok(())
}

enum Key {
    Char(char),
    Enter,
    Backspace,
    Esc,
    ShiftTab,
    CtrlC,
    CtrlD,
    Ignore,
}

// `next` returns the next input byte if one arrives within the timeout in milliseconds.
fn decode_key(first: u8, next: &mut dyn FnMut(i32) -> Result<Option<u8>>) -> Result<Key> {
    match first {
        3 => Ok(Key::CtrlC),
        4 => Ok(Key::CtrlD),
        b'\r' | b'\n' => Ok(Key::Enter),
        0x7f | 0x08 => Ok(Key::Backspace),
        0x1b => decode_escape(next),
        0x20..=0x7e => Ok(Key::Char(first as char)),
        0xc2..=0xf4 => decode_utf8(first, next),
        _ => Ok(Key::Ignore),
    }
}

fn decode_escape(next: &mut dyn FnMut(i32) -> Result<Option<u8>>) -> Result<Key> {
    match next(20)? {
        None => Ok(Key::Esc),
        Some(b'[') => {
            let mut parameters = false;
            while let Some(byte) = next(20)? {
                if (0x40..=0x7e).contains(&byte) {
                    return Ok(if byte == b'Z' && !parameters {
                        Key::ShiftTab
                    } else {
                        Key::Ignore
                    });
                }
                parameters = true;
            }
            Ok(Key::Ignore)
        }
        Some(b'O') => {
            next(20)?;
            Ok(Key::Ignore)
        }
        Some(_) => Ok(Key::Ignore),
    }
}

fn decode_utf8(first: u8, next: &mut dyn FnMut(i32) -> Result<Option<u8>>) -> Result<Key> {
    let length = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    let mut bytes = vec![first];
    while bytes.len() < length {
        match next(20)? {
            Some(byte) => bytes.push(byte),
            None => return Ok(Key::Ignore),
        }
    }
    Ok(std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.chars().next())
        .map_or(Key::Ignore, Key::Char))
}

#[cfg(unix)]
fn read_key() -> Result<Key> {
    let first = read_byte()?;
    decode_key(first, &mut |millis| {
        if poll_stdin(millis)? {
            read_byte().map(Some)
        } else {
            Ok(None)
        }
    })
}

// Reads the descriptor directly: io::stdin() buffers bytes that poll() can no longer see.
#[cfg(unix)]
fn read_byte() -> Result<u8> {
    use std::os::unix::io::AsRawFd;
    let mut byte = 0u8;
    loop {
        match unsafe { read(io::stdin().as_raw_fd(), &mut byte, 1) } {
            1 => return Ok(byte),
            0 => return Err(Error::new("stdin: end of input")),
            _ => {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(Error::new(format!("stdin: {error}")));
                }
            }
        }
    }
}

fn wait_key(timeout_ms: i32) -> Result<bool> {
    #[cfg(unix)]
    {
        poll_stdin(timeout_ms)
    }
    #[cfg(windows)]
    {
        Ok(windows_screen::pending(timeout_ms as u32))
    }
}

#[cfg(unix)]
fn poll_stdin(timeout_ms: i32) -> Result<bool> {
    use std::os::unix::io::AsRawFd;
    let mut fd = PollFd {
        fd: io::stdin().as_raw_fd(),
        events: 1,
        revents: 0,
    };
    let rc = unsafe { poll(&mut fd, 1, timeout_ms) };
    if rc < 0 {
        if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(Error::new("could not read the terminal"));
    }
    Ok(rc > 0)
}

#[cfg(windows)]
fn read_key() -> Result<Key> {
    windows_screen::read_key()
}

#[cfg(windows)]
fn window_size() -> (usize, usize) {
    windows_screen::window_size()
}

#[cfg(unix)]
fn window_size() -> (usize, usize) {
    use std::os::unix::io::AsRawFd;
    let mut size = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe { ioctl(io::stdout().as_raw_fd(), window_request(), &mut size) };
    if rc == -1 || size.ws_row < 3 || size.ws_col < 20 {
        (24, 80)
    } else {
        (size.ws_row as usize, size.ws_col as usize)
    }
}

#[cfg(all(unix, target_os = "macos"))]
fn window_request() -> u64 {
    0x4008_7468
}

#[cfg(all(unix, not(target_os = "macos")))]
fn window_request() -> u64 {
    0x5413
}

#[cfg(unix)]
#[repr(C)]
#[allow(dead_code)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg(unix)]
extern "C" {
    fn ioctl(fd: i32, request: u64, winsize: *mut Winsize) -> i32;
    fn poll(fds: *mut PollFd, nfds: u32, timeout: i32) -> i32;
    fn read(fd: i32, buffer: *mut u8, count: usize) -> isize;
}

#[cfg(unix)]
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(unix)]
struct RawTerm {
    fd: i32,
    original: Termios,
}

#[cfg(unix)]
impl RawTerm {
    fn enable() -> Result<Self> {
        use std::os::unix::io::AsRawFd;
        let fd = io::stdin().as_raw_fd();
        let mut original = zero_termios();
        if unsafe { tcgetattr(fd, &mut original) } != 0 {
            return Err(Error::new("could not read the terminal"));
        }
        let mut raw = original;
        raw.c_lflag &= !echo_canon();
        set_raw_cc(&mut raw);
        if unsafe { tcsetattr(fd, 0, &raw) } != 0 {
            return Err(Error::new("could not set the terminal"));
        }
        Ok(Self { fd, original })
    }
}

#[cfg(unix)]
impl Drop for RawTerm {
    fn drop(&mut self) {
        unsafe {
            tcsetattr(self.fd, 0, &self.original);
        }
    }
}

struct AltScreen;

impl AltScreen {
    fn enter() -> Self {
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\u{1b}[?1049h\u{1b}[?25h");
        let _ = stdout.flush();
        Self
    }
}

impl Drop for AltScreen {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\u{1b}[?1049l\u{1b}[?25h");
        let _ = stdout.flush();
    }
}

#[cfg(all(unix, target_os = "macos"))]
fn echo_canon() -> u64 {
    0x0000_0008 | 0x0000_0100
}

#[cfg(all(unix, not(target_os = "macos")))]
fn echo_canon() -> u32 {
    0o000_010 | 0o000_002
}

#[cfg(all(unix, target_os = "macos"))]
fn set_raw_cc(termios: &mut Termios) {
    termios.c_cc[16] = 1;
    termios.c_cc[17] = 0;
}

#[cfg(all(unix, not(target_os = "macos")))]
fn set_raw_cc(termios: &mut Termios) {
    termios.c_cc[6] = 1;
    termios.c_cc[5] = 0;
}

#[cfg(windows)]
mod windows_screen {
    use std::ffi::c_void;

    use super::{decode_key, raw_console_modes, Key};
    use crate::error::{Error, Result};

    const STD_INPUT: u32 = 0xFFFF_FFF6;
    const STD_OUTPUT: u32 = 0xFFFF_FFF5;

    pub(super) struct Console {
        input: *mut c_void,
        output: *mut c_void,
        input_mode: u32,
        output_mode: u32,
    }

    impl Console {
        pub(super) fn enable() -> Result<Self> {
            let input = handle(STD_INPUT)?;
            let output = handle(STD_OUTPUT)?;
            let input_mode = mode(input)?;
            let output_mode = mode(output)?;
            let (next_input, next_output) = raw_console_modes(input_mode, output_mode);
            if unsafe { SetConsoleMode(output, next_output) } == 0
                || unsafe { SetConsoleMode(input, next_input) } == 0
            {
                unsafe {
                    SetConsoleMode(input, input_mode);
                    SetConsoleMode(output, output_mode);
                }
                return Err(Error::new("the console does not support a raw screen"));
            }
            Ok(Self {
                input,
                output,
                input_mode,
                output_mode,
            })
        }
    }

    impl Drop for Console {
        fn drop(&mut self) {
            unsafe {
                SetConsoleMode(self.input, self.input_mode);
                SetConsoleMode(self.output, self.output_mode);
            }
        }
    }

    fn handle(kind: u32) -> Result<*mut c_void> {
        let handle = unsafe { GetStdHandle(kind) };
        if handle.is_null() || handle == (-1isize as *mut c_void) {
            return Err(Error::new("the console does not support a raw screen"));
        }
        Ok(handle)
    }

    fn mode(handle: *mut c_void) -> Result<u32> {
        let mut mode = 0u32;
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            return Err(Error::new("the console does not support a raw screen"));
        }
        Ok(mode)
    }

    pub(super) fn window_size() -> (usize, usize) {
        let output = unsafe { GetStdHandle(STD_OUTPUT) };
        let mut info = ConsoleScreenBufferInfo {
            size: Coord { x: 0, y: 0 },
            cursor: Coord { x: 0, y: 0 },
            attributes: 0,
            window: SmallRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            maximum: Coord { x: 0, y: 0 },
        };
        if unsafe { GetConsoleScreenBufferInfo(output, &mut info) } == 0 {
            return (24, 80);
        }
        let rows = (info.window.bottom - info.window.top + 1).max(1) as usize;
        let cols = (info.window.right - info.window.left + 1).max(1) as usize;
        (rows, cols)
    }

    pub(super) fn read_key() -> Result<Key> {
        let first = read_byte()?;
        decode_key(first, &mut |millis| {
            if pending(millis as u32) {
                read_byte().map(Some)
            } else {
                Ok(None)
            }
        })
    }

    pub(super) fn pending(millis: u32) -> bool {
        let input = unsafe { GetStdHandle(STD_INPUT) };
        unsafe { WaitForSingleObject(input, millis) == 0 }
    }

    fn read_byte() -> Result<u8> {
        let input = handle(STD_INPUT)?;
        let mut byte = [0u8; 1];
        let mut read = 0u32;
        let ok = unsafe { ReadFile(input, byte.as_mut_ptr(), 1, &mut read, std::ptr::null_mut()) };
        if ok == 0 || read == 0 {
            return Err(Error::new("stdin: console read failed"));
        }
        Ok(byte[0])
    }

    #[repr(C)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    #[repr(C)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor: Coord,
        attributes: u16,
        window: SmallRect,
        maximum: Coord,
    }

    const _: () = assert!(std::mem::size_of::<ConsoleScreenBufferInfo>() == 22);

    unsafe extern "system" {
        fn GetStdHandle(nstd: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
        fn GetConsoleScreenBufferInfo(
            handle: *mut c_void,
            info: *mut ConsoleScreenBufferInfo,
        ) -> i32;
        fn ReadFile(
            handle: *mut c_void,
            buffer: *mut u8,
            to_read: u32,
            read: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn WaitForSingleObject(handle: *mut c_void, millis: u32) -> u32;
    }
}
