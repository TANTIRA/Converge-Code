use std::path::{Component, Path, PathBuf};

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
    let resolved = resolve_inside(workspace, &path)?;
    if resolved.starts_with(workspace.join("weights")) {
        return Err(rejected());
    }
    Ok(())
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
        let path = Path::new(&token);
        if path.is_absolute() && !normalize(path).starts_with(&workspace) {
            return Err(rejected());
        }
    }
    Ok(())
}

fn resolve_inside(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() {
        return Err(rejected());
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return Err(rejected());
    }
    let mut resolved = root.to_path_buf();
    for component in relative_path.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() || !resolved.starts_with(root) {
                    return Err(rejected());
                }
            }
            Component::RootDir | Component::Prefix(_) => return Err(rejected()),
        }
    }
    if !resolved.starts_with(root) {
        return Err(rejected());
    }
    Ok(resolved)
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
