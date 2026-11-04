use std::fs;
use std::path::Path;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::session::Session;
use crate::worktree;

pub fn execute(session: &Session, repo: &Path) -> Result<()> {
    if matches!(session.work_mode.as_str(), "ask" | "plan" | "single-agent") {
        return Err(Error::new("spawn is rejected"));
    }
    let path = session.dir.join("workflow.json");
    let bytes =
        fs::read(&path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let mut document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    if document.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(Error::new("workflow.json schema version must be 1"));
    }
    let steps = document
        .get_mut("steps")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| Error::new("workflow.json steps must be an array"))?;
    let mut order = Vec::new();
    run_steps(repo, &session.dir, steps, &mut order)?;
    let log = session.dir.join("workflow.log");
    fs::write(&log, order.join("\n"))
        .map_err(|error| Error::new(format!("write {}: {error}", log.display())))?;
    let path = session.dir.join("workflow.json");
    let bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| Error::new(format!("encode workflow: {error}")))?;
    fs::write(&path, bytes)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    Ok(())
}

pub fn tasks_text(session_dir: &Path) -> Result<String> {
    let path = session_dir.join("tasks.json");
    let bytes =
        fs::read(&path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let tasks: Vec<Value> = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let mut lines = Vec::new();
    for task in tasks {
        let id = task.get("id").and_then(Value::as_str).unwrap_or("");
        let status = task.get("status").and_then(Value::as_str).unwrap_or("");
        let goal = task.get("goal").and_then(Value::as_str).unwrap_or("");
        lines.push(format!("{id}\t{status}\t{goal}"));
    }
    Ok(lines.join("\n"))
}

pub fn start_background(
    session: &Session,
    repo: &Path,
    destination: &Path,
    branch: &str,
    goal: &str,
) -> Result<JoinHandle<()>> {
    if session.work_mode != "multi-task" && session.work_mode != "dynamic-workflow" {
        return Err(Error::new("spawn is rejected"));
    }
    worktree::add(repo, destination, branch)?;
    let release = destination.join("release");
    let dest = destination.to_path_buf();
    let tasks = session.dir.join("tasks.json");
    write_tasks(
        &tasks,
        &json!([{
            "id": branch,
            "goal": goal,
            "status": "running",
            "worktree": dest,
        }]),
    )?;
    let tasks_done = tasks.clone();
    let branch_done = branch.to_string();
    let goal_done = goal.to_string();
    let handle = thread::spawn(move || {
        for _ in 0..50 {
            if release.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = fs::write(dest.join("child.txt"), "done");
        let _ = write_tasks(
            &tasks_done,
            &json!([{
                "id": branch_done,
                "goal": goal_done,
                "status": "done",
                "worktree": dest,
            }]),
        );
    });
    Ok(handle)
}

fn run_steps(
    repo: &Path,
    session_dir: &Path,
    steps: &mut [Value],
    order: &mut Vec<String>,
) -> Result<()> {
    for step in steps.iter_mut() {
        let op = step
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let id = step
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match op.as_str() {
            "agent" => {
                if step.get("children").is_some() {
                    return Err(Error::new("agent has no children"));
                }
                let dest = session_dir.join("worktrees").join(&id);
                worktree::add(repo, &dest, &id)?;
                order.push(format!("agent {id}"));
                step["status"] = json!("done");
            }
            "parallel" => {
                let children = step
                    .get_mut("children")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| Error::new("parallel requires children"))?;
                if children.len() < 2 {
                    return Err(Error::new("parallel creates more than one worktree"));
                }
                run_steps(repo, session_dir, children, order)?;
                step["status"] = json!("done");
            }
            "pipeline" => {
                let children = step
                    .get_mut("children")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| Error::new("pipeline requires children"))?;
                run_steps(repo, session_dir, children, order)?;
                step["status"] = json!("done");
            }
            _ => return Err(Error::new(format!("unknown workflow op: {op}"))),
        }
    }
    Ok(())
}

fn write_tasks(path: &Path, tasks: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("create {}: {error}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(tasks)
        .map_err(|error| Error::new(format!("encode tasks: {error}")))?;
    fs::write(path, bytes).map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}
