use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::session::Session;

const SEATBELT_PROFILE: &str = r#"(version 1)
(allow default)
(deny file-read* file-write* (subpath (param "CONVERGE")))
(deny network-outbound (subpath (param "CONVERGE")))
(deny file-write* (subpath (param "WEIGHTS")))
(deny file-write* (subpath (param "HOOKS")))"#;

pub(crate) struct Protected {
    converge_home: PathBuf,
    weights: PathBuf,
    hooks: PathBuf,
}

impl Protected {
    pub(crate) fn for_session(session: &Session) -> Self {
        // A session lives at <home>/projects/<repo_key>/sessions/<id>.
        let home = session.dir.ancestors().nth(4).unwrap_or(&session.dir);
        let workspace = &session.workspace;
        let hooks = crate::hook::git_path(workspace, "hooks")
            .unwrap_or_else(|_| workspace.join(".git").join("hooks"));
        Self {
            converge_home: real(home),
            weights: real(&workspace.join("weights")),
            hooks: real(&hooks),
        }
    }

    // Puts the sandbox in front of a shell command line.
    pub(crate) fn wrap(&self, shell: (String, Vec<String>)) -> (String, Vec<String>) {
        let (program, prefix) = shell;
        let (sandbox, mut arguments) = if cfg!(target_os = "macos") {
            ("sandbox-exec", self.seatbelt_arguments())
        } else {
            ("bwrap", self.bubblewrap_arguments())
        };
        arguments.push(program);
        arguments.extend(prefix);
        (sandbox.to_string(), arguments)
    }

    fn seatbelt_arguments(&self) -> Vec<String> {
        vec![
            "-D".to_string(),
            format!("CONVERGE={}", self.converge_home.display()),
            "-D".to_string(),
            format!("WEIGHTS={}", self.weights.display()),
            "-D".to_string(),
            format!("HOOKS={}", self.hooks.display()),
            "-p".to_string(),
            SEATBELT_PROFILE.to_string(),
        ]
    }

    fn bubblewrap_arguments(&self) -> Vec<String> {
        let mut arguments: Vec<String> = ["--dev-bind", "/", "/", "--tmpfs"]
            .map(str::to_string)
            .to_vec();
        arguments.push(self.converge_home.display().to_string());
        for path in [&self.weights, &self.hooks] {
            if path.is_dir() {
                let path = path.display().to_string();
                arguments.extend(["--ro-bind".to_string(), path.clone(), path]);
            }
        }
        arguments
    }
}

// Probes instead of checking the platform: bwrap can be missing or blocked by the
// kernel, and sandbox-exec fails when Converge already runs inside a sandbox.
pub fn available() -> bool {
    static PROBE: OnceLock<bool> = OnceLock::new();
    *PROBE.get_or_init(|| {
        let mut probe = if cfg!(target_os = "macos") {
            let mut command = Command::new("sandbox-exec");
            command.args(["-p", "(version 1)(allow default)", "/usr/bin/true"]);
            command
        } else if cfg!(target_os = "linux") {
            let mut command = Command::new("bwrap");
            command.args(["--dev-bind", "/", "/", "/bin/true"]);
            command
        } else {
            return false;
        };
        probe
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

pub(crate) fn missing_warning() -> Option<&'static str> {
    (!available()).then_some(
        "warning: no OS sandbox is available, so model commands can read ~/.converge, including API keys. On Linux, install bubblewrap.",
    )
}

fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
