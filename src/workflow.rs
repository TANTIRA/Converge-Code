use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::git::{self, Workspace};
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
    Ok(all_worktrees(&git::identify(repo)?).join(&session.id))
}

fn all_worktrees(workspace: &Workspace) -> PathBuf {
    // canonicalize returns \\?\ paths on Windows, and git cannot use them.
    let common = workspace.common_dir.to_string_lossy();
    let plain = common.strip_prefix(r"\\?\").unwrap_or(&common);
    PathBuf::from(plain).join("converge-worktrees")
}

// Merges a finished child into the parent's current branch. The test runs on the merged
// tree before the merge commit exists, so a failure leaves the parent as it was.
pub fn merge_child(
    home: &Path,
    workspace: &Workspace,
    id: &str,
    test_command: &str,
) -> Result<String> {
    let (session, worktree) = find_child(home, workspace, id)?;
    if matches!(session.work_mode.as_str(), "ask" | "plan") {
        return Err(Error::new(format!(
            "{} mode does not change source",
            session.work_mode
        )));
    }
    let goal = child_goal(&session, id)?;
    let repo = &workspace.toplevel;
    if !git_output(repo, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err(Error::new(
            "the parent has uncommitted changes. Commit or stash them before a merge",
        ));
    }
    git_output(&worktree, &["add", "-A"])?;
    if !git_output(&worktree, &["diff", "--cached", "--name-only"])?.is_empty() {
        git_output(&worktree, &["commit", "-m", &goal])?;
    }
    let branch = branch_name(&session.id, id);
    git_output(repo, &["merge", "--no-commit", "--no-ff", &branch])
        .inspect_err(|_| abort_merge(repo))?;
    if git_output(repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).is_ok() {
        let checked = crate::modes::run_session_shell(&session, test_command).and_then(|output| {
            if output.status == 0 {
                Ok(())
            } else {
                Err(Error::new(format!(
                    "the completion command exited with status {} on the merged tree\n{}{}",
                    output.status, output.stdout, output.stderr
                )))
            }
        });
        let committed = checked
            .and_then(|_| git_output(repo, &["commit", "-m", &format!("Merge {id}: {goal}")]));
        if let Err(error) = committed {
            abort_merge(repo);
            return Err(Error::new(format!(
                "merge undone, and the child's work is still on {branch}: {}",
                error.message
            )));
        }
    }
    mark_merged(&session.dir, id)?;
    crate::session::append(
        &session,
        &json!({"kind": "merge", "id": id, "branch": branch}),
    )?;
    git_output(repo, &["worktree", "remove", &worktree.to_string_lossy()])?;
    git_output(repo, &["branch", "-d", &branch])?;
    Ok(format!("merged {id}"))
}

fn abort_merge(repo: &Path) {
    let _ = git_output(repo, &["merge", "--abort"]);
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| Error::new(format!("git failed to start: {error}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        return Ok(stdout);
    }
    Err(Error::new(format!(
        "git {} failed: {stdout} {}",
        args.first().unwrap_or(&""),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

// A finished workflow session is no longer the one that resumes, so search every session.
fn find_child(home: &Path, workspace: &Workspace, id: &str) -> Result<(Session, PathBuf)> {
    let safe = !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if !safe {
        return Err(Error::new(format!("invalid child id: {id}")));
    }
    let entries = fs::read_dir(all_worktrees(workspace));
    let mut found: Vec<PathBuf> = entries
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join(id))
        .filter(|path| path.exists())
        .collect();
    let worktree = match (found.pop(), found.is_empty()) {
        (Some(worktree), true) => worktree,
        (Some(_), false) => {
            return Err(Error::new(format!(
                "more than one session has a child {id}"
            )));
        }
        (None, _) => return Err(Error::new(format!("no child worktree named {id}"))),
    };
    let session_id = worktree
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok((
        crate::session::load_by_id(home, workspace, &session_id)?,
        worktree,
    ))
}

fn child_goal(session: &Session, id: &str) -> Result<String> {
    let _guard = TASKS.lock().expect("tasks");
    let tasks = read_tasks(&session.dir.join("tasks.json"))?;
    let task = tasks.iter().find(|task| task["id"] == id).cloned();
    let document = read_workflow(&session.dir)?;
    let step = document
        .get("steps")
        .and_then(Value::as_array)
        .and_then(|steps| find_agent(steps, id));
    let child = task
        .or_else(|| step.cloned())
        .ok_or_else(|| Error::new(format!("no child named {id} in this session")))?;
    let status = child["status"].as_str().unwrap_or("not started");
    if status != "done" {
        return Err(Error::new(format!(
            "{id} is {status}. Only a child whose completion command passed can merge"
        )));
    }
    Ok(child["goal"].as_str().unwrap_or(id).to_string())
}

fn find_agent<'a>(steps: &'a [Value], id: &str) -> Option<&'a Value> {
    steps.iter().find_map(|step| {
        if step["op"] == "agent" && step["id"] == id {
            return Some(step);
        }
        find_agent(step.get("children")?.as_array()?, id)
    })
}

fn mark_agent_merged(steps: &mut [Value], id: &str) {
    for step in steps {
        if step["op"] == "agent" && step["id"] == id {
            step["status"] = json!("merged");
        }
        if let Some(children) = step.get_mut("children").and_then(Value::as_array_mut) {
            mark_agent_merged(children, id);
        }
    }
}

fn mark_merged(session_dir: &Path, id: &str) -> Result<()> {
    let _guard = TASKS.lock().expect("tasks");
    let tasks_path = session_dir.join("tasks.json");
    let mut tasks = read_tasks(&tasks_path)?;
    if let Some(task) = tasks.iter_mut().find(|task| task["id"] == id) {
        task["status"] = json!("merged");
        return write_json(&tasks_path, &Value::Array(tasks));
    }
    let mut document = read_workflow(session_dir)?;
    if let Some(steps) = document.get_mut("steps").and_then(Value::as_array_mut) {
        mark_agent_merged(steps, id);
    }
    write_json(&session_dir.join("workflow.json"), &document)
}

fn read_workflow(session_dir: &Path) -> Result<Value> {
    let path = session_dir.join("workflow.json");
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| Error::new(format!("read {}: {error}", path.display()))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(error) => Err(Error::new(format!("read {}: {error}", path.display()))),
    }
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
