use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub prompt: String,
    pub test_command: String,
    pub goal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Print(Args),
    HookInstall,
    HookPreCommit,
    Chat,
    ModeWork(String),
    ModePermission(String),
    Tasks,
    Schedule(Args),
    Trigger(Args),
    MmdpPut(String),
    ProviderList,
    ProviderUse(String),
    Setup,
    BackendShow,
    BackendUse(String),
}

impl Invocation {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let args: Vec<String> = args.into_iter().collect();
        if args.is_empty() || args.first().map(String::as_str) == Some("chat") {
            return Ok(Self::Chat);
        }
        match args[0].as_str() {
            "hook" => match args.get(1).map(String::as_str) {
                Some("install") => Ok(Self::HookInstall),
                Some("pre-commit") => Ok(Self::HookPreCommit),
                _ => Err(Error::new("usage: converge hook install | pre-commit")),
            },
            "mode" => match args.get(1).map(String::as_str) {
                Some("work") => Ok(Self::ModeWork(required(&args, 2, "work mode")?)),
                Some("permission") => {
                    Ok(Self::ModePermission(required(&args, 2, "permission mode")?))
                }
                _ => Err(Error::new(
                    "usage: converge mode work <mode> | permission <mode>",
                )),
            },
            "tasks" => Ok(Self::Tasks),
            "setup" => Ok(Self::Setup),
            "provider" => match args.get(1).map(String::as_str) {
                None | Some("list") => Ok(Self::ProviderList),
                Some("use") => Ok(Self::ProviderUse(required(&args, 2, "provider")?)),
                _ => Err(Error::new("usage: converge provider [list | use <name>]")),
            },
            "backend" => match args.get(1).map(String::as_str) {
                None | Some("show") => Ok(Self::BackendShow),
                Some("use") => Ok(Self::BackendUse(required(&args, 2, "backend")?)),
                _ => Err(Error::new("usage: converge backend [show | use <name>]")),
            },
            "schedule" => Ok(Self::Schedule(parse_print(&args[1..])?)),
            "trigger" => Ok(Self::Trigger(parse_print(&args[1..])?)),
            "mmdp" => {
                if args.get(1).map(String::as_str) == Some("put") {
                    Ok(Self::MmdpPut(required(&args, 2, "name")?))
                } else {
                    Err(Error::new("usage: converge mmdp put <name>"))
                }
            }
            "-h" | "--help" => Err(Error::new(usage())),
            _ => Ok(Self::Print(parse_print(&args)?)),
        }
    }
}

pub fn parse_print(args: &[String]) -> Result<Args> {
    let mut prompt = None;
    let mut test_command = None;
    let mut goal = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "-p" | "--print" => {
                index += 1;
                prompt = Some(required(args, index, "-p")?);
            }
            "--test" => {
                index += 1;
                test_command = Some(required(args, index, "--test")?);
            }
            "--goal" => {
                index += 1;
                goal = Some(required(args, index, "--goal")?);
            }
            other if other.starts_with("--test=") => {
                test_command = Some(other["--test=".len()..].to_string());
            }
            other if other.starts_with("--goal=") => {
                goal = Some(other["--goal=".len()..].to_string());
            }
            "-h" | "--help" => return Err(Error::new(usage())),
            other => {
                return Err(Error::new(format!(
                    "unknown argument: {other}\n{}",
                    usage()
                )));
            }
        }
        index += 1;
    }
    let prompt = prompt.ok_or_else(|| Error::new(format!("-p is required\n{}", usage())))?;
    let test_command =
        test_command.ok_or_else(|| Error::new(format!("--test is required\n{}", usage())))?;
    if prompt.is_empty() || test_command.is_empty() {
        return Err(Error::new(usage()));
    }
    Ok(Args {
        prompt,
        test_command,
        goal,
    })
}

fn required(args: &[String], index: usize, flag: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("{flag} requires a value\n{}", usage())))
}

fn usage() -> &'static str {
    "usage: converge -p <prompt> --test <command> [--goal <text>]"
}
