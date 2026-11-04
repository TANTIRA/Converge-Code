use crate::error::{Error, Result};

pub fn admit() -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err(Error::new("isolated compute is a Linux entry point"));
    }
    Ok(())
}
