use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::cli::Args;
use crate::edit;
use crate::error::Result;
use crate::git;
use crate::home;
use crate::instructions;
use crate::mcp;
use crate::model::{self, Model};
use crate::modes;
use crate::session::{self, Budget, Session};
use crate::shell;

const MAX_OUTPUT: usize = 100_000;

pub fn stop_allowed(test_status: i32) -> bool {
    test_status == 0
}

pub fn run_print(cwd: &Path, args: &Args) -> Result<i32> {
    let workspace = git::identify(cwd)?;
    let mut model = model::OpenAiCompatible::from_env()?;
    let home = home::converge_home()?;
    let instructions = instructions::load_scoped(
        &workspace.toplevel,
        Some(&instructions::organization_instructions()),
        Some(&instructions::user_instructions(&home)),
    )?;
    let mut session = match session::load_unfinished(&home, &workspace)? {
        Some(mut existing) => {
            if let Some(goal) = args.goal.clone() {
                session::set_goal(&mut existing, goal)?;
            }
            existing
        }
        None => {
            let budget = session::budget_from_env()?;
            session::create(&home, &workspace, args.goal.clone(), budget)?
        }
    };
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
    execute(
        &mut model,
        &mut session,
        &home,
        &mut messages,
        &args.test_command,
    )
}

fn execute(
    model: &mut dyn Model,
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
            session::append(session, &json!({"kind": "budget", "exhausted": true}))?;
            return Ok(1);
        }
        let turn = model.complete(messages, &tools)?;
        steps += 1;
        tokens = tokens.saturating_add(turn.total_tokens);
        session.used = session::Usage {
            steps,
            tokens,
            seconds: base_seconds.saturating_add(started.elapsed().as_secs()),
        };
        session::store_used(session)?;
        messages.push(turn.assistant_message.clone());
        session::append(
            session,
            &json!({"kind": "assistant", "message": turn.assistant_message}),
        )?;
        if over_token_budget(session.budget.tokens, tokens) {
            session::append(session, &json!({"kind": "budget", "exhausted": true}))?;
            return Ok(1);
        }
        if turn.tool_calls.is_empty() {
            let output = shell::run(&session.workspace, test_command)?;
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
            let text = format!(
                "Completion command exited with status {}.\nstdout:\n{}\nstderr:\n{}",
                output.status,
                clip(&output.stdout),
                clip(&output.stderr)
            );
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
            let result = dispatch(session, &call.name, &call.arguments);
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": &result,
            }));
            session::append(
                session,
                &json!({"kind": "tool", "name": call.name, "text": result}),
            )?;
        }
    }
}

fn dispatch(session: &Session, name: &str, arguments: &str) -> String {
    if let Err(error) = modes::admit(session, name, arguments, false) {
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
            match shell::run(root, command) {
                Ok(output) => format!(
                    "status: {}\nstdout:\n{}\nstderr:\n{}",
                    output.status,
                    clip(&output.stdout),
                    clip(&output.stderr)
                ),
                Err(error) => error.message,
            }
        }
        _ => format!("mcp tool {name}"),
    }
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

fn exhausted(budget: &Budget, steps: u64, tokens: u64, seconds: u64) -> bool {
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
