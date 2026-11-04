use crate::error::{Error, Result};

pub fn admit() -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err(Error::new("isolated compute is a Linux entry point"));
    }
    if !separate_pid_namespace() {
        return Err(Error::new(
            "isolated compute requires its own pid namespace",
        ));
    }
    Ok(())
}

fn separate_pid_namespace() -> bool {
    #[cfg(target_os = "linux")]
    {
        match (namespace("self"), namespace("1")) {
            (Ok(mine), Ok(init)) => mine != init,
            _ => false,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn namespace(id: &str) -> Result<String> {
    std::fs::read_link(format!("/proc/{id}/ns/pid"))
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| Error::new(error.to_string()))
}
