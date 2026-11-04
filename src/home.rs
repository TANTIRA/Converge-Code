use std::env;
use std::path::PathBuf;

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
