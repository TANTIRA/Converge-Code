use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::worktree;

const MAX_LINES: usize = 200;
const MAX_BYTES: usize = 25 * 1024;

pub fn write_lesson(
    home: &Path,
    repo_key: &str,
    name: &str,
    symptom: &str,
    rule: &str,
    test_name: &str,
    test_status: i32,
) -> Result<PathBuf> {
    if test_status != 0 {
        return Err(Error::new(
            "a lesson is written only after the completion command exits 0",
        ));
    }
    let dir = memory_dir(home, repo_key)?;
    let topic = dir.join(format!("{name}.md"));
    let body = format!("# {name}\n\nsymptom: {symptom}\n\nrule: {rule}\n\ntest: {test_name}\n");
    fs::write(&topic, body)
        .map_err(|error| Error::new(format!("write {}: {error}", topic.display())))?;
    let index = dir.join("MEMORY.md");
    let mut text = fs::read_to_string(&index).unwrap_or_default();
    if text.is_empty() {
        text.push_str("# Memory\n");
    }
    text.push_str(&format!(
        "- lesson: {name} | {symptom} | {rule} | {test_name}\n"
    ));
    fs::write(&index, text)
        .map_err(|error| Error::new(format!("write {}: {error}", index.display())))?;
    Ok(index)
}

// The write-lesson action stages the note. It becomes memory only when the completion
// command of this session exits 0, so a failed run writes no lesson.
pub fn write_session_lesson(
    session: &crate::session::Session,
    name: &str,
    symptom: &str,
    rule: &str,
    test_name: &str,
) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::new("a lesson needs a name"));
    }
    if session.last_test_status == Some(0) {
        let home = crate::home::converge_home()?;
        let repo_key = crate::git::identify(&session.workspace)?.repo_key;
        let path = write_lesson(&home, &repo_key, name, symptom, rule, test_name, 0)?;
        return Ok(format!("lesson written to {}", path.display()));
    }
    let dir = staged_dir(&session.dir)?;
    let path = dir.join(format!("{name}.json"));
    let record = serde_json::json!({
        "name": name,
        "symptom": symptom,
        "rule": rule,
        "test": test_name,
    });
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| Error::new(format!("encode lesson: {error}")))?;
    fs::write(&path, bytes)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    Ok(
        "lesson staged. It becomes memory only when the completion command exits with code 0."
            .to_string(),
    )
}

// Called after the completion command exits 0.
pub fn flush_lessons(session: &crate::session::Session) -> Result<Vec<String>> {
    if session.last_test_status != Some(0) {
        return Ok(Vec::new());
    }
    let dir = session.dir.join("lessons");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(Error::new(format!("read {}: {error}", dir.display()))),
    };
    let home = crate::home::converge_home()?;
    let repo_key = crate::git::identify(&session.workspace)?.repo_key;
    let mut written = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(note) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let field = |key: &str| note.get(key).and_then(|value| value.as_str()).unwrap_or("");
        write_lesson(
            &home,
            &repo_key,
            field("name"),
            field("symptom"),
            field("rule"),
            field("test"),
            0,
        )?;
        let _ = fs::remove_file(&path);
        written.push(field("name").to_string());
    }
    Ok(written)
}

// A run that did not pass writes no lesson.
pub fn discard_lessons(session: &crate::session::Session) {
    let _ = fs::remove_dir_all(session.dir.join("lessons"));
}

fn staged_dir(session_dir: &Path) -> Result<PathBuf> {
    let dir = session_dir.join("lessons");
    fs::create_dir_all(&dir)
        .map_err(|error| Error::new(format!("create {}: {error}", dir.display())))?;
    Ok(dir)
}

pub fn load_index(home: &Path, repo_key: &str) -> Result<String> {
    let path = memory_dir(home, repo_key)?.join("MEMORY.md");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(Error::new(format!("read {}: {error}", path.display()))),
    };
    let mut kept = String::new();
    for (index, line) in text.lines().enumerate() {
        if index >= MAX_LINES || kept.len() + line.len() + 1 > MAX_BYTES {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    Ok(kept)
}

pub fn revert_lesson(home: &Path, repo_key: &str, name: &str) -> Result<()> {
    let dir = memory_dir(home, repo_key)?;
    let index = dir.join("MEMORY.md");
    let text = fs::read_to_string(&index)
        .map_err(|error| Error::new(format!("read {}: {error}", index.display())))?;
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| !line.contains(&format!("lesson: {name} ")))
        .collect();
    fs::write(&index, format!("{}\n", kept.join("\n")))
        .map_err(|error| Error::new(format!("write {}: {error}", index.display())))?;
    let topic = dir.join(format!("{name}.md"));
    if topic.exists() {
        fs::remove_file(&topic)
            .map_err(|error| Error::new(format!("remove {}: {error}", topic.display())))?;
    }
    Ok(())
}

pub fn write_learning_skill(repo: &Path, depth: u32, text: &str) -> Result<PathBuf> {
    if depth > 3 {
        return Err(Error::new("learning depth stops at 3"));
    }
    if !worktree::in_worktree(repo)? {
        return Err(Error::new(
            "the lesson-writing skill changes only in a child worktree",
        ));
    }
    let path = repo.join("skill-lesson.md");
    fs::write(&path, text)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    Ok(path)
}

fn memory_dir(home: &Path, repo_key: &str) -> Result<PathBuf> {
    let dir = home.join("projects").join(repo_key).join("memory");
    fs::create_dir_all(&dir)
        .map_err(|error| Error::new(format!("create {}: {error}", dir.display())))?;
    Ok(dir)
}
