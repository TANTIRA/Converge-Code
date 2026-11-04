use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::git;
use crate::home;
use crate::model::Model;
use crate::modes;
use crate::session::{self, Budget, Session};

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
    #[cfg(unix)]
    if stdin.is_terminal() && stdout.is_terminal() {
        return run_screen(&workspace, &converge_home, session);
    }
    #[cfg(windows)]
    if stdin.is_terminal() && stdout.is_terminal() {
        return windows_screen::run(&workspace, &converge_home, session);
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
            session::append(&session, &serde_json::json!({"kind": "user", "text": line}))?;
            let text = crate::backend::reply(&converge_home, &workspace.toplevel, &line)?;
            writeln!(stdout, "{text}").map_err(|error| Error::new(error.to_string()))?;
            let message = serde_json::json!({"role": "assistant", "content": text});
            session::append(
                &session,
                &serde_json::json!({"kind": "assistant", "message": message}),
            )?;
            continue;
        }
        let rendered = format_turn(&line, None, None);
        write!(stdout, "{rendered}").map_err(|error| Error::new(error.to_string()))?;
        session::append(&session, &serde_json::json!({"kind": "user", "text": line}))?;
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
        .map(|line| fit(line, cols))
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
    text.chars().take(cols).collect()
}

#[cfg(unix)]
fn run_screen(workspace: &crate::git::Workspace, home: &Path, mut session: Session) -> Result<i32> {
    let _raw = RawTerm::enable()?;
    let _screen = AltScreen::enter();
    let instructions = crate::instructions::load_scoped(
        &workspace.toplevel,
        Some(&crate::instructions::organization_instructions()),
        Some(&crate::instructions::user_instructions(home)),
    )?;
    let mut view = load_view(&session);
    let mut messages = vec![serde_json::json!({
        "role": "system",
        "content": chat_prompt(&instructions, session.goal.as_deref()),
    })];
    extend_messages(&mut messages, &session);
    let mut input = String::new();
    let mut model = crate::model::OpenAiCompatible::open(home).ok();
    loop {
        if CANCEL.swap(false, Ordering::SeqCst) {
            return Ok(130);
        }
        let status = status_line(&session, home, model.is_some());
        paint(&render_chat_frame(
            window_size().0,
            window_size().1,
            &view,
            &input,
            &status,
        ))?;
        match read_key()? {
            Key::Char(ch) => input.push(ch),
            Key::Backspace => {
                input.pop();
            }
            Key::Esc => input.clear(),
            Key::ShiftTab => {
                let next = modes::cycle_permission(&session.permission_mode);
                session::set_permission_mode(&mut session, next)?;
            }
            Key::CtrlC => return Ok(130),
            Key::CtrlD => {
                if input.is_empty() {
                    return Ok(0);
                }
            }
            Key::Enter => {
                let line = std::mem::take(&mut input);
                if line == "/quit" {
                    return Ok(0);
                }
                if line.is_empty() {
                    continue;
                }
                if let Some(shown) = command_line(&mut session, &line)? {
                    view.push(shown);
                    continue;
                }
                view.push(format!("you  {line}"));
                session::append(&session, &serde_json::json!({"kind": "user", "text": line}))?;
                messages.push(serde_json::json!({"role": "user", "content": line}));
                let reply = chat_reply(&mut session, home, &mut model, &mut messages, &mut view)?;
                if reply == 130 {
                    let exit = CANCEL.swap(false, Ordering::SeqCst);
                    crate::shell::clear_stop();
                    if exit {
                        return Ok(130);
                    }
                    continue;
                }
            }
        }
    }
}

fn command_line(session: &mut Session, line: &str) -> Result<Option<String>> {
    if let Some(rest) = line.strip_prefix("/goal") {
        return goal_reply(session, rest.trim()).map(Some);
    }
    if line == "/tasks" {
        return Ok(Some(tasks_reply(session)));
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

fn load_view(session: &Session) -> Vec<String> {
    let path = session.dir.join("transcript.jsonl");
    let text = std::fs::read_to_string(path).unwrap_or_default();
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
    let path = session.dir.join("transcript.jsonl");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("kind").and_then(|kind| kind.as_str()) {
            Some("user") => messages.push(serde_json::json!({
                "role": "user",
                "content": value.get("text").and_then(|text| text.as_str()).unwrap_or("")
            })),
            Some("assistant") => {
                if let Some(message) = value.get("message") {
                    messages.push(message.clone());
                }
            }
            _ => {}
        }
    }
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
    loop {
        if CANCEL.load(Ordering::SeqCst) || crate::shell::stop_requested() {
            view.push("cancelled".to_string());
            return Ok(130);
        }
        if session.used.steps >= session.budget.steps.unwrap_or(session::DEFAULT_STEP_BUDGET) {
            view.push("budget exhausted".to_string());
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
        session.used.steps += 1;
        session.used.tokens = session.used.tokens.saturating_add(turn.total_tokens);
        session::store_used(session)?;
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
            view.push(content);
        }
        if turn.tool_calls.is_empty() {
            return Ok(0);
        }
        for call in turn.tool_calls {
            if modes::asks(session, &call.name) {
                let allowed = confirm_tool(&call.name, view, session, home)?;
                if !allowed {
                    let text = format!("{} denied", call.name);
                    view.push(text.clone());
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": call.id,
                        "content": text,
                    }));
                    session::append(
                        session,
                        &serde_json::json!({"kind": "denied", "name": call.name}),
                    )?;
                    continue;
                }
            }
            let result =
                watching(|| crate::run::dispatch(session, &call.name, &call.arguments, true));
            view.push(format!("tool {}\n{result}", call.name));
            messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": &result,
            }));
            session::append(
                session,
                &serde_json::json!({"kind": "tool", "name": call.name, "text": &result}),
            )?;
        }
    }
}

fn confirm_tool(
    name: &str,
    view: &mut Vec<String>,
    session: &Session,
    home: &Path,
) -> Result<bool> {
    let prompt = format!("allow {name}? [y/n] ");
    let mut answer = String::new();
    loop {
        let (rows, cols) = window_size();
        paint(&render_chat_frame(
            rows,
            cols,
            view,
            &format!("{prompt}{answer}"),
            &status_line(session, home, true),
        ))?;
        match read_key()? {
            Key::Char('y') | Key::Char('Y') => return Ok(true),
            Key::Char('n') | Key::Char('N') | Key::Esc => return Ok(false),
            Key::CtrlC => {
                CANCEL.store(true, Ordering::SeqCst);
                return Ok(false);
            }
            Key::Backspace => {
                answer.pop();
            }
            Key::Char(ch) => answer.push(ch),
            Key::Enter => return Ok(answer.eq_ignore_ascii_case("y")),
            Key::ShiftTab | Key::CtrlD => {}
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
}

#[cfg(unix)]
fn read_key() -> Result<Key> {
    let mut byte = [0u8; 1];
    io::stdin()
        .read_exact(&mut byte)
        .map_err(|error| Error::new(format!("stdin: {error}")))?;
    match byte[0] {
        3 => Ok(Key::CtrlC),
        4 => Ok(Key::CtrlD),
        b'\r' | b'\n' => Ok(Key::Enter),
        0x7f | 0x08 => Ok(Key::Backspace),
        0x1b => read_escape(),
        value if (0x20..0x7f).contains(&value) => Ok(Key::Char(value as char)),
        _ => read_key(),
    }
}

#[cfg(unix)]
fn read_escape() -> Result<Key> {
    let mut sequence = String::new();
    while poll_stdin(20)? {
        let mut byte = [0u8; 1];
        io::stdin()
            .read_exact(&mut byte)
            .map_err(|error| Error::new(format!("stdin: {error}")))?;
        sequence.push(byte[0] as char);
        if sequence.len() > 8 {
            break;
        }
    }
    if sequence == "[Z" {
        Ok(Key::ShiftTab)
    } else {
        Ok(Key::Esc)
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

#[cfg(unix)]
struct AltScreen;

#[cfg(unix)]
impl AltScreen {
    fn enter() -> Self {
        let mut stdout = io::stdout();
        let _ = write!(stdout, "\u{1b}[?1049h\u{1b}[?25h");
        let _ = stdout.flush();
        Self
    }
}

#[cfg(unix)]
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

#[cfg(all(unix, target_os = "macos"))]
fn zero_termios() -> Termios {
    Termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_cc: [0; 20],
        c_ispeed: 0,
        c_ospeed: 0,
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn zero_termios() -> Termios {
    Termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_line: 0,
        c_cc: [0; 32],
        c_ispeed: 0,
        c_ospeed: 0,
    }
}

#[cfg(unix)]
extern "C" {
    fn tcgetattr(fd: i32, termios: *mut Termios) -> i32;
    fn tcsetattr(fd: i32, optional_actions: i32, termios: *const Termios) -> i32;
}

#[cfg(all(unix, target_os = "macos"))]
#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u64,
    c_oflag: u64,
    c_cflag: u64,
    c_lflag: u64,
    c_cc: [u8; 20],
    c_ispeed: u64,
    c_ospeed: u64,
}

#[cfg(all(unix, target_os = "macos"))]
const _: () = assert!(std::mem::size_of::<Termios>() == 72);

#[cfg(all(unix, not(target_os = "macos")))]
#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

#[cfg(all(unix, not(target_os = "macos")))]
const _: () = assert!(std::mem::size_of::<Termios>() == 60);

#[cfg(windows)]
mod windows_screen {
    use std::ffi::c_void;
    use std::io::{self, Write};
    use std::path::Path;

    use super::{
        chat_prompt, chat_reply, command_line, extend_messages, load_view, paint,
        raw_console_modes, render_chat_frame, status_line, Key, CANCEL,
    };
    use crate::error::{Error, Result};
    use crate::session::{self, Session};

    const STD_INPUT: u32 = 0xFFFF_FFF6;
    const STD_OUTPUT: u32 = 0xFFFF_FFF5;

    struct Console {
        input: *mut c_void,
        output: *mut c_void,
        input_mode: u32,
        output_mode: u32,
    }

    impl Console {
        fn enable() -> Result<Self> {
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

    pub fn run(
        workspace: &crate::git::Workspace,
        home: &Path,
        mut session: Session,
    ) -> Result<i32> {
        let _console = Console::enable()?;
        let _screen = AltScreen::enter();
        let instructions = crate::instructions::load_scoped(
            &workspace.toplevel,
            Some(&crate::instructions::organization_instructions()),
            Some(&crate::instructions::user_instructions(home)),
        )?;
        let mut view = load_view(&session);
        let mut messages = vec![serde_json::json!({
            "role": "system",
            "content": chat_prompt(&instructions, session.goal.as_deref()),
        })];
        extend_messages(&mut messages, &session);
        let mut input = String::new();
        let mut model = crate::model::OpenAiCompatible::open(home).ok();
        loop {
            if CANCEL.swap(false, std::sync::atomic::Ordering::SeqCst) {
                return Ok(130);
            }
            let status = status_line(&session, home, model.is_some());
            let (rows, cols) = window_size();
            paint(&render_chat_frame(rows, cols, &view, &input, &status))?;
            match read_key()? {
                Key::Char(ch) => input.push(ch),
                Key::Backspace => {
                    input.pop();
                }
                Key::Esc => input.clear(),
                Key::ShiftTab => {
                    let next = crate::modes::cycle_permission(&session.permission_mode);
                    session::set_permission_mode(&mut session, next)?;
                }
                Key::CtrlC => return Ok(130),
                Key::CtrlD => {
                    if input.is_empty() {
                        return Ok(0);
                    }
                }
                Key::Enter => {
                    let line = std::mem::take(&mut input);
                    if line == "/quit" {
                        return Ok(0);
                    }
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(shown) = command_line(&mut session, &line)? {
                        view.push(shown);
                        continue;
                    }
                    view.push(format!("you  {line}"));
                    session::append(&session, &serde_json::json!({"kind": "user", "text": line}))?;
                    messages.push(serde_json::json!({"role": "user", "content": line}));
                    let reply =
                        chat_reply(&mut session, home, &mut model, &mut messages, &mut view)?;
                    if reply == 130 {
                        let exit = CANCEL.swap(false, std::sync::atomic::Ordering::SeqCst);
                        crate::shell::clear_stop();
                        if exit {
                            return Ok(130);
                        }
                        continue;
                    }
                }
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
        let byte = read_byte()?;
        if byte == 0x1b {
            return read_escape();
        }
        match byte {
            b'\r' | b'\n' => Ok(Key::Enter),
            0x7f | 0x08 => Ok(Key::Backspace),
            0x03 => Ok(Key::CtrlC),
            0x04 => Ok(Key::CtrlD),
            value if (0x20..0x7f).contains(&value) => Ok(Key::Char(value as char)),
            _ => Ok(Key::Char(' ')),
        }
    }

    fn read_escape() -> Result<Key> {
        if !pending(20) {
            return Ok(Key::Esc);
        }
        let second = read_byte()?;
        if second != b'[' || !pending(20) {
            return Ok(Key::Esc);
        }
        let third = read_byte()?;
        if third == b'Z' {
            Ok(Key::ShiftTab)
        } else {
            Ok(Key::Esc)
        }
    }

    fn pending(millis: u32) -> bool {
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
