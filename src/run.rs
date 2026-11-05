use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli::Args;
use crate::edit;
use crate::error::Result;
use crate::git;
use crate::home;
use crate::instructions;
use crate::mcp;
use crate::model::{self, OpenAiCompatible};
use crate::modes;
use crate::session::{self, Budget, Session};

const MAX_OUTPUT: usize = 100_000;

pub fn stop_allowed(test_status: i32) -> bool {
    test_status == 0
}

pub fn run_print(cwd: &Path, args: &Args) -> Result<i32> {
    exit_on_interrupt();
    let result = print_session(cwd, args);
    if crate::shell::stop_requested() {
        return Ok(130);
    }
    result
}

fn print_session(cwd: &Path, args: &Args) -> Result<i32> {
    let home = home::converge_home()?;
    if crate::backend::selected(&home)? != "converge" {
        return crate::backend::run_external(
            cwd,
            &home,
            &args.prompt,
            &args.test_command,
            args.goal.clone(),
        );
    }
    let workspace = git::identify(cwd)?;
    let mut model = model::OpenAiCompatible::open(&home)?;
    let instructions = instructions::load_scoped(
        &workspace.toplevel,
        Some(&instructions::organization_instructions()),
        Some(&instructions::user_instructions(&home)),
    )?;
    let mut session = session::open_for_print(&home, &workspace, args.goal.clone())?;
    let mut messages = vec![
        json!({
            "role": "system",
            "content": system_prompt(
                &instructions,
                &args.test_command,
                session.goal.as_deref(),
            ),
        }),
        json!({"role": "user", "content": args.prompt}),
    ];
    if let Ok(server) = std::env::var("CONVERGE_MCP_SERVER") {
        if !server.is_empty() {
            session.mcp_server = Some(server);
            session::save(&session)?;
        }
    }
    session::append(&session, &json!({"kind": "user", "text": args.prompt}))?;
    let _clients = crate::uhp::serve(&home, &session, &workspace.toplevel)?;
    crate::uhp::publish(
        "session/status",
        json!({
            "work_mode": session.work_mode,
            "permission_mode": session.permission_mode,
        }),
    );
    crate::uhp::publish("session/stream", json!({"text": args.prompt}));
    execute(
        &mut model,
        &mut session,
        &home,
        &mut messages,
        &args.test_command,
    )
}

fn execute(
    model: &mut OpenAiCompatible,
    session: &mut Session,
    home: &Path,
    messages: &mut Vec<Value>,
    test_command: &str,
) -> Result<i32> {
    let mut tools = model::admitted_tools();
    let mut mcp_names = Vec::new();
    if let Some(server) = session.mcp_server.clone() {
        let ask = session.work_mode == "ask";
        let extra = mcp::initial_tools(home, &server, ask)?;
        mcp_names = extra.iter().map(|tool| tool.name.clone()).collect();
        tools.extend(extra);
    }
    let started = Instant::now();
    let base_seconds = session.used.seconds;
    let mut steps = session.used.steps;
    let mut tokens = session.used.tokens;
    loop {
        let seconds = base_seconds.saturating_add(started.elapsed().as_secs());
        if exhausted(&session.budget, steps, tokens, seconds) {
            return out_of_budget(session);
        }
        let turn = model.complete(messages, &tools)?;
        steps += 1;
        tokens = tokens.saturating_add(turn.total_tokens);
        session.used = session::Usage {
            steps,
            tokens,
            seconds: base_seconds.saturating_add(started.elapsed().as_secs()),
        };
        session::save(session)?;
        messages.push(turn.assistant_message.clone());
        session::append(
            session,
            &json!({"kind": "assistant", "message": turn.assistant_message}),
        )?;
        let spoken = turn
            .assistant_message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !spoken.is_empty() {
            crate::uhp::publish("session/stream", json!({"text": spoken}));
        }
        if over_token_budget(session.budget.tokens, tokens) {
            return out_of_budget(session);
        }
        if turn.tool_calls.is_empty() {
            let text = match modes::run_session_shell(session, test_command) {
                Ok(output) => {
                    session::append(
                        session,
                        &json!({
                            "kind": "test",
                            "status": output.status,
                            "stdout": clip(&output.stdout),
                            "stderr": clip(&output.stderr),
                        }),
                    )?;
                    if stop_allowed(output.status) {
                        if session.work_mode == "debug" {
                            session.source_closed = true;
                            session::save(session)?;
                        }
                        return Ok(0);
                    }
                    format!(
                        "Completion command exited with status {}.\nstdout:\n{}\nstderr:\n{}",
                        output.status,
                        clip(&output.stdout),
                        clip(&output.stderr)
                    )
                }
                Err(error) => {
                    session::append(session, &json!({"kind": "test", "error": error.message}))?;
                    format!("Completion command did not finish: {}", error.message)
                }
            };
            messages.push(json!({"role": "user", "content": text}));
            session::append(
                session,
                &json!({"kind": "user", "text": "completion command failed"}),
            )?;
            continue;
        }
        if let Some(denied) = turn.tool_calls.iter().find(|call| {
            !model::is_admitted(&call.name) && !mcp_names.iter().any(|name| name == &call.name)
        }) {
            session::append(session, &json!({"kind": "denied", "name": denied.name}))?;
            return Ok(1);
        }
        for call in &turn.tool_calls {
            if let Some(code) = modes::unattended_exit(session, &call.name, &call.arguments) {
                session::append(session, &json!({"kind": "denied", "name": call.name}))?;
                eprintln!(
                    "{} needs permission, and there is no terminal to ask (permission mode {}). Change it with converge mode permission <mode>.",
                    call.name, session.permission_mode
                );
                return Ok(code);
            }
            let result = dispatch(session, &call.name, &call.arguments, false);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": &result,
            }));
            session::append(
                session,
                &json!({"kind": "tool", "name": call.name, "tool_call_id": call.id, "text": &result}),
            )?;
            crate::uhp::publish("session/tool", json!({"name": call.name, "text": &result}));
            if call.name == "edit" {
                crate::uhp::publish_diff(&session.workspace);
            }
        }
    }
}

pub(crate) fn dispatch(session: &Session, name: &str, arguments: &str, tty: bool) -> String {
    if let Err(error) = modes::admit(session, name, arguments, tty) {
        return error.message;
    }
    let root = &session.workspace;
    let parsed: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    match name {
        "edit" => {
            let path = parsed.get("path").and_then(Value::as_str).unwrap_or("");
            let old = parsed.get("old").and_then(Value::as_str).unwrap_or("");
            let new = parsed.get("new").and_then(Value::as_str).unwrap_or("");
            match edit::apply(root, path, old, new) {
                Ok(message) => message,
                Err(error) => error.message,
            }
        }
        "shell" => {
            let command = parsed.get("command").and_then(Value::as_str).unwrap_or("");
            match modes::run_session_shell(session, command) {
                Ok(output) => format!(
                    "status: {}\nstdout:\n{}\nstderr:\n{}",
                    output.status,
                    clip(&output.stdout),
                    clip(&output.stderr)
                ),
                Err(error) => error.message,
            }
        }
        // No MCP client exists yet, so say that nothing ran rather than report a success.
        _ => format!("error: {name} is not connected, and nothing was run"),
    }
}

// A command runs in its own process group, so Ctrl-C reaches only Converge. The shell
// loop kills that group on stop, and this exit also ends a blocked model request.
fn exit_on_interrupt() {
    crate::tty::install_cancel_handler();
    thread::spawn(|| loop {
        if crate::shell::stop_requested() {
            thread::sleep(Duration::from_millis(200));
            std::process::exit(130);
        }
        thread::sleep(Duration::from_millis(50));
    });
}

fn system_prompt(instructions: &str, test_command: &str, goal: Option<&str>) -> String {
    let mut prompt = String::from(
        "You are Converge. Edit files with the edit tool and run commands with the shell tool. The task is complete only when the completion command exits with code 0.\n",
    );
    if let Some(goal) = goal {
        prompt.push_str("\nGoal:\n");
        prompt.push_str(goal);
        prompt.push('\n');
    }
    prompt.push_str("\nCompletion command:\n");
    prompt.push_str(test_command);
    if !instructions.is_empty() {
        prompt.push_str("\n\nProject instructions:\n");
        prompt.push_str(instructions);
    }
    prompt
}

fn out_of_budget(session: &Session) -> Result<i32> {
    session::append(session, &json!({"kind": "budget", "exhausted": true}))?;
    eprintln!(
        "budget exhausted. Raise CONVERGE_BUDGET_STEPS, CONVERGE_BUDGET_TOKENS, or CONVERGE_BUDGET_SECONDS and run again to continue."
    );
    Ok(1)
}

pub(crate) fn exhausted(budget: &Budget, steps: u64, tokens: u64, seconds: u64) -> bool {
    if steps >= budget.steps.unwrap_or(session::DEFAULT_STEP_BUDGET) {
        return true;
    }
    if over_token_budget(budget.tokens, tokens) {
        return true;
    }
    if let Some(limit) = budget.seconds {
        if seconds >= limit {
            return true;
        }
    }
    false
}

fn over_token_budget(limit: Option<u64>, tokens: u64) -> bool {
    limit.is_some_and(|limit| tokens >= limit)
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(MAX_OUTPUT).collect();
    clipped.push_str("\n[output truncated]");
    clipped
}
