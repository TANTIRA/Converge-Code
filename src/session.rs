use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::git::Workspace;

pub const DEFAULT_STEP_BUDGET: u64 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub steps: Option<u64>,
    pub tokens: Option<u64>,
    pub seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Usage {
    pub steps: u64,
    pub tokens: u64,
    pub seconds: u64,
}

#[derive(Debug)]
pub struct Session {
    pub id: String,
    pub dir: PathBuf,
    pub workspace: PathBuf,
    pub goal: Option<String>,
    pub budget: Budget,
    pub used: Usage,
    pub work_mode: String,
    pub permission_mode: String,
    pub source_closed: bool,
    pub mcp_server: Option<String>,
}

pub fn create(
    home: &Path,
    workspace: &Workspace,
    goal: Option<String>,
    budget: Budget,
) -> Result<Session> {
    let id = Uuid::new_v4().to_string();
    let dir = home
        .join("projects")
        .join(&workspace.repo_key)
        .join("sessions")
        .join(&id);
    fs::create_dir_all(&dir)
        .map_err(|error| Error::new(format!("create {}: {error}", dir.display())))?;
    crate::home::make_private(home)?;
    let session = Session {
        id,
        dir,
        workspace: workspace.toplevel.clone(),
        goal,
        budget,
        used: Usage::default(),
        work_mode: "single-agent".to_string(),
        permission_mode: "dontAsk".to_string(),
        source_closed: false,
        mcp_server: None,
    };
    write_record(&session)?;
    Ok(session)
}

pub fn open(home: &Path, workspace: &Workspace) -> Result<Session> {
    match load_unfinished(home, workspace)? {
        Some(session) => Ok(session),
        None => create(
            home,
            workspace,
            None,
            Budget {
                steps: None,
                tokens: None,
                seconds: None,
            },
        ),
    }
}

pub fn open_for_print(home: &Path, workspace: &Workspace, goal: Option<String>) -> Result<Session> {
    let Some(mut session) = load_unfinished(home, workspace)? else {
        return create(home, workspace, goal, budget_from_env()?);
    };
    if let Some(goal) = goal {
        set_goal(&mut session, goal)?;
    }
    apply_budget_env(&mut session)?;
    Ok(session)
}

pub fn load_unfinished(home: &Path, workspace: &Workspace) -> Result<Option<Session>> {
    let sessions = home
        .join("projects")
        .join(&workspace.repo_key)
        .join("sessions");
    let entries = match fs::read_dir(&sessions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::new(format!("read {}: {error}", sessions.display())));
        }
    };
    crate::home::make_private(home)?;
    let mut newest: Option<(SystemTime, Session)> = None;
    for entry in entries {
        let entry =
            entry.map_err(|error| Error::new(format!("read {}: {error}", sessions.display())))?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(mut session) = read_session(&dir)? else {
            continue;
        };
        if !same_workspace(&session.workspace, &workspace.toplevel)? {
            continue;
        }
        session.workspace = workspace.toplevel.clone();
        if transcript_passed(&dir)? {
            continue;
        }
        let modified = fs::metadata(dir.join("session.json"))
            .and_then(|metadata| metadata.modified())
            .map_err(|error| Error::new(format!("stat {}: {error}", dir.display())))?;
        let replace = newest
            .as_ref()
            .map(|(time, _)| modified >= *time)
            .unwrap_or(true);
        if replace {
            newest = Some((modified, session));
        }
    }
    Ok(newest.map(|(_, session)| session))
}

pub(crate) fn load_by_id(home: &Path, workspace: &Workspace, id: &str) -> Result<Session> {
    let dir = home
        .join("projects")
        .join(&workspace.repo_key)
        .join("sessions")
        .join(id);
    let mut session =
        read_session(&dir)?.ok_or_else(|| Error::new(format!("no session named {id}")))?;
    session.workspace = workspace.toplevel.clone();
    Ok(session)
}

pub fn set_goal(session: &mut Session, goal: String) -> Result<()> {
    session.goal = Some(goal);
    write_record(session)
}

pub fn save(session: &Session) -> Result<()> {
    write_record(session)
}

pub fn set_work_mode(session: &mut Session, mode: &str) -> Result<()> {
    match mode {
        "ask" | "plan" | "debug" | "single-agent" | "dynamic-workflow" | "multi-task" => {}
        _ => return Err(Error::new(format!("unknown work mode: {mode}"))),
    }
    session.work_mode = mode.to_string();
    save(session)
}

pub fn set_permission_mode(session: &mut Session, mode: &str) -> Result<()> {
    if mode == "bypass" {
        return crate::windows_isolation::laptop_bypass().map(|_| ());
    }
    match mode {
        "ask-each" | "accept-edits" | "auto" | "dontAsk" | "plan" => {}
        _ => return Err(Error::new(format!("unknown permission mode: {mode}"))),
    }
    session.permission_mode = mode.to_string();
    save(session)
}

pub fn append(session: &Session, value: &Value) -> Result<()> {
    append_record(&session.dir, value)
}

// One write call per line, so a client thread cannot interleave with the agent loop.
pub(crate) fn append_record(dir: &Path, value: &Value) -> Result<()> {
    let path = dir.join("transcript.jsonl");
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| Error::new(format!("open {}: {error}", path.display())))?;
    file.write_all(format!("{value}\n").as_bytes())
        .map_err(|error| Error::new(format!("write transcript: {error}")))
}

fn apply_budget_env(session: &mut Session) -> Result<()> {
    let steps = optional_u64("CONVERGE_BUDGET_STEPS")?;
    let tokens = optional_u64("CONVERGE_BUDGET_TOKENS")?;
    let seconds = optional_u64("CONVERGE_BUDGET_SECONDS")?;
    if steps.is_none() && tokens.is_none() && seconds.is_none() {
        return Ok(());
    }
    session.budget.steps = steps.or(session.budget.steps);
    session.budget.tokens = tokens.or(session.budget.tokens);
    session.budget.seconds = seconds.or(session.budget.seconds);
    save(session)
}

pub fn budget_from_env() -> Result<Budget> {
    Ok(Budget {
        steps: Some(optional_u64("CONVERGE_BUDGET_STEPS")?.unwrap_or(DEFAULT_STEP_BUDGET)),
        tokens: optional_u64("CONVERGE_BUDGET_TOKENS")?,
        seconds: optional_u64("CONVERGE_BUDGET_SECONDS")?,
    })
}

fn optional_u64(name: &str) -> Result<Option<u64>> {
    match std::env::var(name) {
        Err(_) => Ok(None),
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|_| Error::new(format!("{name} must be an integer"))),
    }
}

fn same_workspace(stored: &Path, toplevel: &Path) -> Result<bool> {
    match stored.canonicalize() {
        Ok(canonical) => Ok(canonical == toplevel),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(Error::new(format!(
            "cannot canonicalize {}: {error}",
            stored.display()
        ))),
    }
}

fn transcript_passed(dir: &Path) -> Result<bool> {
    let path = dir.join("transcript.jsonl");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::new(format!("read {}: {error}", path.display()))),
    };
    for line in String::from_utf8_lossy(&bytes).lines() {
        // A crash can leave a partial last line. That must not lock the whole repo.
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let test = value.get("kind").and_then(Value::as_str) == Some("test");
        let passed = value.get("status").and_then(Value::as_i64) == Some(0);
        if test && passed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn read_session(dir: &Path) -> Result<Option<Session>> {
    let path = dir.join("session.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::new(format!("read {}: {error}", path.display()))),
    };
    let record: StoredSession = match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        Err(error) => {
            eprintln!("skipping damaged session {}: {error}", path.display());
            return Ok(None);
        }
    };
    Ok(Some(Session {
        id: record.session_id,
        dir: dir.to_path_buf(),
        workspace: PathBuf::from(record.workspace),
        goal: record.goal,
        budget: record.budget,
        used: record.used,
        work_mode: record.work_mode,
        permission_mode: record.permission_mode,
        source_closed: record.source_closed,
        mcp_server: record.mcp_server,
    }))
}

fn write_record(session: &Session) -> Result<()> {
    let record = StoredSession {
        session_id: session.id.clone(),
        workspace: session.workspace.to_string_lossy().into_owned(),
        work_mode: session.work_mode.clone(),
        permission_mode: session.permission_mode.clone(),
        source_closed: session.source_closed,
        mcp_server: session.mcp_server.clone(),
        goal: session.goal.clone(),
        budget: session.budget,
        used: session.used,
    };
    let path = session.dir.join("session.json");
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| Error::new(format!("encode session: {error}")))?;
    let temporary = session.dir.join("session.json.tmp");
    fs::write(&temporary, bytes)
        .and_then(|_| fs::rename(&temporary, &path))
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}

#[derive(Serialize, Deserialize)]
struct StoredSession {
    session_id: String,
    workspace: String,
    work_mode: String,
    permission_mode: String,
    #[serde(default)]
    source_closed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_server: Option<String>,
    #[serde(default)]
    goal: Option<String>,
    budget: Budget,
    #[serde(default)]
    used: Usage,
}
