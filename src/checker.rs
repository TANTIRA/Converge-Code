use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub fn allow(workspace: &Path, name: &str, arguments: &str) -> Result<()> {
    match name {
        "edit" => allow_edit(workspace, arguments),
        "shell" => allow_shell(workspace, arguments),
        _ => Err(rejected()),
    }
}

fn allow_edit(workspace: &Path, arguments: &str) -> Result<()> {
    let path = argument(arguments, "path");
    crate::edit::resolve_inside(workspace, &path)
        .map(|_| ())
        .map_err(|_| rejected())
}

fn allow_shell(workspace: &Path, arguments: &str) -> Result<()> {
    let command = argument(arguments, "command");
    if command.is_empty() {
        return Err(rejected());
    }
    let workspace = normalize(workspace);
    for token in tokens(&command) {
        if token.split(['/', '\\']).any(|part| part == "..") {
            return Err(rejected());
        }
        if token.starts_with('~') || token.contains("$HOME") || token.contains("${HOME") {
            return Err(rejected());
        }
        let path = Path::new(&token);
        if path.is_absolute() && !normalize(path).starts_with(&workspace) {
            return Err(rejected());
        }
    }
    Ok(())
}

fn tokens(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch| matches!(ch, '"' | '\'' | '`'))
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .collect()
}

fn normalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn argument(arguments: &str, name: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get(name)
                .and_then(|item| item.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn rejected() -> Error {
    Error::new("auto rejected the action")
}
