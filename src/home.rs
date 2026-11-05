use std::env;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub fn converge_home() -> Result<PathBuf> {
    let value = if cfg!(windows) {
        env::var_os("USERPROFILE")
    } else {
        env::var_os("HOME")
    };
    let home = value.ok_or_else(|| {
        Error::new(if cfg!(windows) {
            "USERPROFILE is not set"
        } else {
            "HOME is not set"
        })
    })?;
    Ok(PathBuf::from(home).join(".converge"))
}

pub(crate) fn make_private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| Error::new(format!("chmod {}: {error}", path.display())))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
