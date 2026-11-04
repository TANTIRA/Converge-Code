use std::io::{self, Write};
use std::path::Path;

use crate::error::{Error, Result};
use crate::git;
use crate::home;
use crate::mmdp;
use crate::session::{self, Budget};
use crate::windows_isolation;
use crate::workflow;

fn open_session(cwd: &Path) -> Result<session::Session> {
    let workspace = git::identify(cwd)?;
    let converge_home = home::converge_home()?;
    match session::load_unfinished(&converge_home, &workspace)? {
        Some(session) => Ok(session),
        None => session::create(
            &converge_home,
            &workspace,
            None,
            Budget {
                steps: None,
                tokens: None,
                seconds: None,
            },
        ),
    }
}

pub fn set_work_mode_cli(cwd: &Path, mode: &str) -> Result<i32> {
    let mut session = open_session(cwd)?;
    session::set_work_mode(&mut session, mode)?;
    println!("{mode}");
    Ok(0)
}

pub fn set_permission_mode_cli(cwd: &Path, mode: &str) -> Result<i32> {
    if mode == "bypass" {
        return windows_isolation::laptop_bypass().map(|_| 0);
    }
    let mut session = open_session(cwd)?;
    session::set_permission_mode(&mut session, mode)?;
    println!("{mode}");
    Ok(0)
}

pub fn print_tasks(cwd: &Path) -> Result<i32> {
    let session = open_session(cwd)?;
    let text = workflow::tasks_text(&session.dir)?;
    writeln!(io::stdout(), "{text}").map_err(|error| Error::new(error.to_string()))?;
    Ok(0)
}

pub fn mmdp_put_laptop(cwd: &Path, name: &str) -> Result<i32> {
    let workspace = git::identify(cwd)?;
    mmdp::put_file(false, &workspace.toplevel.join("catalog"), name, b"")?;
    Ok(0)
}
