use std::env;
use std::process::ExitCode;

use converge::Invocation;

fn main() -> ExitCode {
    let invocation = match Invocation::parse(env::args().skip(1)) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("current directory: {error}");
            return ExitCode::from(1);
        }
    };
    let result = match invocation {
        Invocation::Print(args) | Invocation::Schedule(args) | Invocation::Trigger(args) => {
            converge::run_print(&cwd, &args)
        }
        Invocation::HookInstall => {
            let bin = match env::current_exe() {
                Ok(bin) => bin,
                Err(error) => {
                    eprintln!("current executable: {error}");
                    return ExitCode::from(1);
                }
            };
            converge::install_hook(&cwd, &bin).map(|_| 0)
        }
        Invocation::HookPreCommit => converge::pre_commit(&cwd).map(|_| 0),
        Invocation::Chat => converge::run_chat(&cwd),
        Invocation::ModeWork(mode) => converge::set_work_mode_cli(&cwd, &mode),
        Invocation::ModePermission(mode) => converge::set_permission_mode_cli(&cwd, &mode),
        Invocation::Tasks => converge::print_tasks(&cwd),
        Invocation::MmdpPut(name) => converge::mmdp_put_laptop(&cwd, &name),
    };
    match result {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
