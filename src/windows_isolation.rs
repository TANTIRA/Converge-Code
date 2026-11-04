use crate::error::{Error, Result};

pub fn admit() -> Result<()> {
    if !cfg!(windows) {
        return Err(Error::new("windows isolation does not run on this host"));
    }
    Ok(())
}

pub fn laptop_bypass() -> Result<()> {
    Err(Error::new("bypass is rejected on a laptop workspace"))
}
