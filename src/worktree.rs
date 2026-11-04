use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use crate::error::{Error, Result};
use crate::session::Session;

pub fn add(repo: &Path, destination: &Path, branch: &str) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Error::new(format!("create {}: {error}", parent.display())))?;
    }
    let status = Command::new("git")
        .args([
            "worktree",
            "add",
            "-b",
            branch,
            &destination.to_string_lossy(),
        ])
        .current_dir(repo)
        .status()
        .map_err(|error| Error::new(format!("git worktree failed to start: {error}")))?;
    if !status.success() {
        return Err(Error::new("git worktree add failed"));
    }
    Ok(())
}

pub fn write_delegation(dir: &Path, goal: &str, worktree: &Path) -> Result<PathBuf> {
    let path = dir.join("delegation.json");
    let record = json!({
        "goal": goal,
        "worktree": worktree,
    });
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| Error::new(format!("encode delegation: {error}")))?;
    fs::write(&path, bytes)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    Ok(path)
}

pub fn spawn(session: &Session, repo: &Path, destination: &Path, branch: &str) -> Result<()> {
    if matches!(session.work_mode.as_str(), "single-agent" | "ask" | "plan") {
        return Err(Error::new("spawn is rejected"));
    }
    add(repo, destination, branch)?;
    write_delegation(
        &session.dir,
        session.goal.as_deref().unwrap_or(""),
        destination,
    )?;
    Ok(())
}

pub fn parent_status(repo: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo)
        .output()
        .map_err(|error| Error::new(format!("git status failed to start: {error}")))?;
    if !output.status.success() {
        return Err(Error::new("git status failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn in_worktree(repo: &Path) -> Result<bool> {
    let git_dir = canonical_git(repo, "--git-dir")?;
    let common = canonical_git(repo, "--git-common-dir")?;
    Ok(git_dir != common)
}

fn canonical_git(repo: &Path, flag: &str) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", flag])
        .current_dir(repo)
        .output()
        .map_err(|error| Error::new(format!("git failed to start: {error}")))?;
    if !output.status.success() {
        return Err(Error::new("not a git repository"));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = PathBuf::from(&raw);
    let absolute = if path.is_absolute() {
        path
    } else {
        repo.join(path)
    };
    absolute.canonicalize().map_err(|error| {
        Error::new(format!(
            "cannot canonicalize {}: {error}",
            absolute.display()
        ))
    })
}
