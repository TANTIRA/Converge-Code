use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub common_dir: PathBuf,
    pub toplevel: PathBuf,
    pub repo_key: String,
}

pub fn identify(cwd: &Path) -> Result<Workspace> {
    let toplevel_raw = git(cwd, &["rev-parse", "--show-toplevel"])?;
    let common_raw = git(cwd, &["rev-parse", "--git-common-dir"])?;
    let toplevel = normalize(cwd, &toplevel_raw)?;
    let common_dir = normalize(cwd, &common_raw)?;
    let repo_key = repo_key(&common_dir);
    Ok(Workspace {
        common_dir,
        toplevel,
        repo_key,
    })
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|error| Error::new(format!("git failed to start: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            return Err(Error::new("not a git repository"));
        }
        return Err(Error::new(format!("not a git repository: {detail}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn normalize(cwd: &Path, raw: &str) -> Result<PathBuf> {
    let path = PathBuf::from(raw);
    let absolute = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    absolute.canonicalize().map_err(|error| {
        Error::new(format!(
            "cannot canonicalize {}: {error}",
            absolute.display()
        ))
    })
}

fn repo_key(common_dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path_key(common_dir).as_bytes());
    hex_encode(&hasher.finalize())
}

fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    // Windows and the default macOS volume compare paths without case.
    if cfg!(windows) || cfg!(target_os = "macos") {
        text.to_lowercase()
    } else {
        text.into_owned()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0xf) as usize] as char);
    }
    encoded
}
