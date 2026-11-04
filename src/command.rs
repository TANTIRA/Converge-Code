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

pub fn show_backend_cli() -> Result<i32> {
    let home = home::converge_home()?;
    let name = crate::backend::selected(&home)?;
    println!("{name}");
    Ok(0)
}

pub fn use_backend_cli(name: &str) -> Result<i32> {
    let home = home::converge_home()?;
    crate::backend::use_backend(&home, name)?;
    println!("{name}");
    Ok(0)
}

pub fn list_providers_cli() -> Result<i32> {
    let home = home::converge_home()?;
    let text = crate::provider::list(&home)?;
    writeln!(io::stdout(), "{text}").map_err(|error| Error::new(error.to_string()))?;
    Ok(0)
}

pub fn use_provider_cli(name: &str) -> Result<i32> {
    let home = home::converge_home()?;
    crate::provider::configure_cli(&home, name)?;
    println!("{name}");
    Ok(0)
}

pub fn setup_cli() -> Result<i32> {
    if !std::io::IsTerminal::is_terminal(&io::stdin()) {
        return Err(Error::new("converge setup needs a terminal"));
    }
    let home = home::converge_home()?;
    let (providers, active) = crate::provider::profiles(&home)?;
    println!("Select a provider:");
    for (index, profile) in providers.iter().enumerate() {
        let mark = if profile.name == active {
            " active"
        } else {
            ""
        };
        println!("  {}  {}{mark}", index + 1, profile.name);
    }
    print!("Provider: ");
    io::stdout()
        .flush()
        .map_err(|error| Error::new(error.to_string()))?;
    let answer = crate::secret::read_reply()?;
    let name = if answer.is_empty() {
        if active.is_empty() {
            return Err(Error::new("provider name is empty"));
        }
        active
    } else if let Ok(number) = answer.parse::<usize>() {
        providers
            .get(number.checked_sub(1).unwrap_or(usize::MAX))
            .map(|profile| profile.name.clone())
            .ok_or_else(|| Error::new(format!("provider {answer} is not configured")))?
    } else {
        answer
    };
    crate::provider::configure_cli(&home, &name)?;
    println!("{name}");
    Ok(0)
}

pub fn mmdp_put_laptop(cwd: &Path, name: &str) -> Result<i32> {
    let workspace = git::identify(cwd)?;
    mmdp::put_file(false, &workspace.toplevel.join("catalog"), name, b"")?;
    Ok(0)
}
