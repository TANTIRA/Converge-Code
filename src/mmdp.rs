use std::fs;
use std::path::Path;

use crate::error::{Error, Result};

pub fn put_file(off_machine: bool, catalog: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    if !off_machine {
        return Err(Error::new("on a laptop, git is the source of truth"));
    }
    fs::create_dir_all(catalog)
        .map_err(|error| Error::new(format!("create {}: {error}", catalog.display())))?;
    let path = catalog.join(name);
    fs::write(&path, bytes)
        .map_err(|error| Error::new(format!("write {}: {error}", path.display())))
}
