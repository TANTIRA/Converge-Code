use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::git;
use crate::session::Session;
use crate::shell;
use crate::worktree;

const CHILD_TIMEOUT: Duration = Duration::from_secs(3600);
static TASKS: Mutex<()> = Mutex::new(());

struct Children<'a> {
    repo: &'a Path,
    root: PathBuf,
    session_id: &'a str,
    bin: &'a Path,
    test_command: &'a str,
}

// Returns whether every child's completion command passed.
pub fn execute(session: &Session, repo: &Path, bin: &Path, test_command: &str) -> Result<bool> {
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
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("workflow.json steps must be an array"))?;
    validate(steps)?;
    let count = steps.len();
    let children = Children {
        repo,
        root: worktree_root(session, repo)?,
        session_id: &session.id,
        bin,
        test_command,
    };
    let mut order = Vec::new();
    let mut passed = true;
    // shortcut: progress is saved per top-level step, so an interrupted group reruns its unfinished children; upgrade if groups grow long.
    for index in 0..count {
        passed = run_step(&children, &mut document["steps"][index], &mut order)?;
        write_json(&path, &document)?;
        if !passed {
            break;
        }
    }
    let log = session.dir.join("workflow.log");
    fs::write(&log, order.join("\n"))
        .map_err(|error| Error::new(format!("write {}: {error}", log.display())))?;
    Ok(passed)
}

pub fn tasks_text(session_dir: &Path) -> Result<String> {
    let _guard = TASKS.lock().expect("tasks");
    let mut lines = Vec::new();
    for task in read_tasks(&session_dir.join("tasks.json"))? {
        let field = |name: &str| task.get(name).and_then(Value::as_str).unwrap_or("");
        lines.push(format!(
            "{}\t{}\t{}",
            field("id"),
            field("status"),
            field("goal")
        ));
    }
    Ok(lines.join("\n"))
}

// shortcut: a task whose parent exits stays "running" in tasks.json while its child finishes alone; upgrade by recording the child's pid.
pub fn start_background(
    session: &Session,
    repo: &Path,
    bin: &Path,
    goal: &str,
    test_command: &str,
) -> Result<JoinHandle<()>> {
    if session.work_mode != "multi-task" && session.work_mode != "dynamic-workflow" {
        return Err(Error::new("spawn is rejected"));
    }
    let tasks = session.dir.join("tasks.json");
    let id = {
        let _guard = TASKS.lock().expect("tasks");
        format!("task-{}", read_tasks(&tasks)?.len() + 1)
    };
    let dest = worktree_root(session, repo)?.join(&id);
    worktree::add(repo, &dest, &branch_name(&session.id, &id))?;
    {
        let _guard = TASKS.lock().expect("tasks");
        let mut all = read_tasks(&tasks)?;
        all.push(json!({
            "id": id,
            "goal": goal,
            "status": "running",
            "worktree": dest,
            "reported": false,
        }));
        write_json(&tasks, &Value::Array(all))?;
    }
    let bin = bin.to_path_buf();
    let goal = goal.to_string();
    let test_command = test_command.to_string();
    Ok(thread::spawn(move || {
        // Esc cancels the parent's turn, not a task the user sent to the background.
        let passed = run_child(&bin, &dest, &goal, &test_command, None, false);
        let _guard = TASKS.lock().expect("tasks");
        let Ok(mut all) = read_tasks(&tasks) else {
            return;
        };
        for task in &mut all {
            if task["id"] == id.as_str() {
                task["status"] = json!(if passed { "done" } else { "failed" });
            }
        }
        let _ = write_json(&tasks, &Value::Array(all));
    }))
}

// Each finished task is returned once, so its result enters the transcript on a later turn.
pub(crate) fn take_finished(session_dir: &Path) -> Vec<String> {
    let _guard = TASKS.lock().expect("tasks");
    let path = session_dir.join("tasks.json");
    let Ok(mut all) = read_tasks(&path) else {
        return Vec::new();
    };
    let mut finished = Vec::new();
    for task in &mut all {
        if task["status"] != "running" && task["reported"] == false {
            finished.push(format!(
                "Background task {} finished with status {}. Its work is in {} and is not merged.",
                task["id"].as_str().unwrap_or(""),
                task["status"].as_str().unwrap_or(""),
                task["worktree"].as_str().unwrap_or("")
            ));
            task["reported"] = json!(true);
        }
    }
    if !finished.is_empty() {
        let _ = write_json(&path, &Value::Array(all));
    }
    finished
}

// Inside the git common dir, so the parent stays clean, and outside ~/.converge, which the sandbox closes.
fn worktree_root(session: &Session, repo: &Path) -> Result<PathBuf> {
    let common = git::identify(repo)?.common_dir;
    // canonicalize returns \\\\?\\ paths on Windows, and git cannot use them.
    let common = common.to_string_lossy();
    let plain = common.strip_prefix(r"\\?\").unwrap_or(&common);
    Ok(PathBuf::from(plain)
        .join("converge-worktrees")
        .join(&session.id))
}

fn branch_name(session_id: &str, id: &str) -> String {
    let short: String = session_id.chars().take(8).collect();
    format!("converge/{short}/{id}")
}

// Runs before any child starts, because a step id becomes a path and a branch name.
fn validate(steps: &[Value]) -> Result<()> {
    for step in steps {
        let op = step.get("op").and_then(Value::as_str).unwrap_or("");
        let children = step.get("children").and_then(Value::as_array);
        match op {
            "agent" => {
                if step.get("children").is_some() {
                    return Err(Error::new("agent has no children"));
                }
                let id = step.get("id").and_then(Value::as_str).unwrap_or("");
                let safe = |ch: char| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_';
                if id.is_empty() || !id.chars().all(safe) {
                    return Err(Error::new(format!("invalid agent id: {id}")));
                }
                if step
                    .get("goal")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Err(Error::new(format!("agent {id} has no goal")));
                }
            }
            "parallel" => {
                let children = children.ok_or_else(|| Error::new("parallel requires children"))?;
                if children.len() < 2 {
                    return Err(Error::new("parallel creates more than one worktree"));
                }
                validate(children)?;
            }
            "pipeline" => {
                validate(children.ok_or_else(|| Error::new("pipeline requires children"))?)?;
            }
            _ => return Err(Error::new(format!("unknown workflow op: {op}"))),
        }
    }
    Ok(())
}

fn run_step(children: &Children, step: &mut Value, order: &mut Vec<String>) -> Result<bool> {
    if step["status"] == "done" {
        return Ok(true);
    }
    let passed = match step["op"].as_str().unwrap_or("") {
        "agent" => run_agent(children, step, order)?,
        "parallel" => {
            let results: Vec<Result<(bool, Vec<String>)>> = thread::scope(|scope| {
                let handles: Vec<_> = step["children"]
                    .as_array_mut()
                    .into_iter()
                    .flatten()
                    .map(|child| {
                        scope.spawn(move || {
                            let mut child_order = Vec::new();
                            let passed = run_step(children, child, &mut child_order)?;
                            Ok((passed, child_order))
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .unwrap_or_else(|_| Err(Error::new("parallel child failed")))
                    })
                    .collect()
            });
            let mut passed = true;
            for result in results {
                let (child_passed, child_order) = result?;
                passed &= child_passed;
                order.extend(child_order);
            }
            passed
        }
        _ => {
            let mut passed = true;
            for child in step["children"].as_array_mut().into_iter().flatten() {
                if !run_step(children, child, order)? {
                    passed = false;
                    break;
                }
            }
            passed
        }
    };
    step["status"] = json!(if passed { "done" } else { "failed" });
    Ok(passed)
}

fn run_agent(children: &Children, step: &Value, order: &mut Vec<String>) -> Result<bool> {
    let id = step["id"].as_str().unwrap_or("");
    let dest = children.root.join(id);
    if !dest.exists() {
        worktree::add(children.repo, &dest, &branch_name(children.session_id, id))?;
    }
    order.push(format!("agent {id}"));
    Ok(run_child(
        children.bin,
        &dest,
        step["goal"].as_str().unwrap_or(""),
        children.test_command,
        step["model"].as_str(),
        true,
    ))
}

// The child is a whole session. Only its completion command can make it pass.
fn run_child(
    bin: &Path,
    worktree: &Path,
    goal: &str,
    test_command: &str,
    model: Option<&str>,
    cancellable: bool,
) -> bool {
    let arguments = ["-p", goal, "--goal", goal, "--test", test_command].map(str::to_string);
    shell::run_child(worktree, bin, &arguments, model, CHILD_TIMEOUT, cancellable)
        .is_ok_and(|output| output.status == 0)
}

fn read_tasks(path: &Path) -> Result<Vec<Value>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::new(format!("read {}: {error}", path.display()))),
    };
    serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("create {}: {error}", parent.display())))?;
    }
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| Error::new(format!("encode {}: {error}", path.display())))?;
    fs::write(path, bytes).map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}
