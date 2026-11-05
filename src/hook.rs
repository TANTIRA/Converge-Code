use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::shell;

pub fn install(repo: &Path, bin: &Path) -> Result<PathBuf> {
    let hooks = git_path(repo, "hooks")?;
    fs::create_dir_all(&hooks)
        .map_err(|error| Error::new(format!("create {}: {error}", hooks.display())))?;
    let hook = hooks.join("pre-commit");
    match fs::read(&hook) {
        Ok(existing) if !String::from_utf8_lossy(&existing).contains(" hook pre-commit") => {
            return Err(Error::new(format!(
                "{} already exists and is not Converge's. Merge it by hand: add `converge hook pre-commit` to it",
                hook.display()
            )));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(Error::new(format!("read {}: {error}", hook.display())));
        }
        _ => {}
    }
    let script = format!(
        "#!/bin/sh\ncd \"$(git rev-parse --show-toplevel)\" || exit 1\nexec {} hook pre-commit\n",
        shell_quote(bin)
    );
    write_executable(&hook, &script)?;
    Ok(hook)
}

pub fn pre_commit(repo: &Path) -> Result<()> {
    let Some(command) = lint_command(repo)? else {
        return Ok(());
    };
    let output = shell::run(repo, &command)?;
    if output.status != 0 {
        let detail = format!("{}{}", output.stdout, output.stderr);
        return Err(Error::new(format!(
            "lint failed with status {}\n{detail}",
            output.status
        )));
    }
    Ok(())
}

fn lint_command(repo: &Path) -> Result<Option<String>> {
    match std::env::var("CONVERGE_LINT") {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) => Ok(None),
        Err(_) if is_executable(&repo.join("lint")) => Ok(Some("./lint".to_string())),
        Err(_) => Ok(None),
    }
}

pub(crate) fn git_path(repo: &Path, name: &str) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-path", name])
        .current_dir(repo)
        .output()
        .map_err(|error| Error::new(format!("git failed to start: {error}")))?;
    if !output.status.success() {
        return Err(Error::new("not a git repository"));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let path = PathBuf::from(&raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

fn write_executable(path: &Path, script: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    file.write_all(script.as_bytes())
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = file
            .metadata()
            .map_err(|error| Error::new(format!("stat {}: {error}", path.display())))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| Error::new(format!("chmod {}: {error}", path.display())))?;
    }
    Ok(())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.exists()
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
