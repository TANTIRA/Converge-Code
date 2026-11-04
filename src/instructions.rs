use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub fn load_once(root: &Path) -> Result<String> {
    load_scoped(root, None, None)
}

pub fn load_scoped(
    root: &Path,
    organization: Option<&Path>,
    user: Option<&Path>,
) -> Result<String> {
    let mut parts = Vec::new();
    if let Some(path) = organization {
        if let Some(text) = read_optional(path)? {
            parts.push(text);
        }
    }
    if let Some(path) = user {
        if let Some(text) = read_optional(path)? {
            parts.push(text);
        }
    }
    if let Some(text) = read_optional(&root.join("CONVERGE.md"))? {
        parts.push(text);
    } else if let Some(text) = read_optional(&root.join("AGENTS.md"))? {
        parts.push(text);
    }
    if let Some(text) = read_optional(&root.join("CONVERGE.local.md"))? {
        parts.push(text);
    }
    Ok(parts.join("\n\n"))
}

pub fn organization_instructions() -> PathBuf {
    if cfg!(windows) {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
        PathBuf::from(base).join("Converge").join("instructions.md")
    } else if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/Converge/instructions.md")
    } else {
        PathBuf::from("/etc/converge/instructions.md")
    }
}

pub fn user_instructions(home: &Path) -> PathBuf {
    home.join("instructions.md")
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::new(format!("read {}: {error}", path.display()))),
    }
}
