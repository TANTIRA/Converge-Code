use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};

pub fn apply(root: &Path, relative: &str, old: &str, new: &str) -> Result<String> {
    let path = resolve_inside(root, relative)?;
    if !path.exists() {
        if !old.is_empty() {
            return Err(Error::new(format!("file not found: {relative}")));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| Error::new(format!("create {}: {error}", parent.display())))?;
        }
        retry(|| fs::write(&path, new.as_bytes()), "write", &path)?;
        return Ok(format!("created {relative}"));
    }
    if old.is_empty() {
        return Err(Error::new(format!(
            "old string is empty and {relative} already exists"
        )));
    }
    let current = retry(|| fs::read_to_string(&path), "read", &path)?;
    let count = current.matches(old).count();
    if count != 1 {
        return Err(Error::new(format!(
            "old string matched {count} times in {relative}"
        )));
    }
    let updated = current.replacen(old, new, 1);
    retry(|| fs::write(&path, updated.as_bytes()), "write", &path)?;
    Ok(format!("edited {relative}"))
}

pub(crate) fn resolve_inside(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() {
        return Err(Error::new("path is empty"));
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute() {
        return Err(Error::new("path must stay inside the workspace"));
    }
    let mut resolved = root.to_path_buf();
    for component in relative_path.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() || !resolved.starts_with(root) {
                    return Err(Error::new("path must stay inside the workspace"));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::new("path must stay inside the workspace"));
            }
        }
    }
    if !resolved.starts_with(root) {
        return Err(Error::new("path must stay inside the workspace"));
    }
    check_real_path(root, &resolved)?;
    Ok(resolved)
}

// Judges the path after symlinks, and without case, as macOS and Windows open it.
fn check_real_path(root: &Path, path: &Path) -> Result<()> {
    let outside = || Error::new("path must stay inside the workspace");
    let real_root = root.canonicalize().map_err(|_| outside())?;
    let mut existing = path;
    while fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().ok_or_else(outside)?;
    }
    let real = existing.canonicalize().map_err(|_| outside())?;
    let missing = path.strip_prefix(existing).map_err(|_| outside())?;
    let inside = real
        .strip_prefix(&real_root)
        .map_err(|_| outside())?
        .join(missing);
    if inside.iter().any(|part| part.eq_ignore_ascii_case(".git")) {
        return Err(Error::new("the edit tool does not write inside .git"));
    }
    let weights = inside
        .iter()
        .next()
        .is_some_and(|part| part.eq_ignore_ascii_case("weights"));
    if weights && root.join("weights").is_dir() {
        return Err(Error::new(
            "weight bytes cannot be written outside PromoteWeightEdit",
        ));
    }
    Ok(())
}

fn retry<T>(op: impl Fn() -> std::io::Result<T>, action: &str, path: &Path) -> Result<T> {
    let mut attempt = 0;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) if is_lock(&error) && attempt + 1 < 8 => {
                attempt += 1;
                thread::sleep(Duration::from_millis(25 * attempt));
            }
            Err(error) => {
                return Err(Error::new(format!("{action} {}: {error}", path.display())));
            }
        }
    }
}

fn is_lock(error: &std::io::Error) -> bool {
    // Windows returns ERROR_SHARING_VIOLATION (32) while an editor holds the file.
    error.kind() == ErrorKind::PermissionDenied || error.raw_os_error() == Some(32)
}
