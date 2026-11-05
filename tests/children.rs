use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    path: std::ffi::OsString,
}

impl Fixture {
    // The child uses a backend CLI, so these runs never call a real model.
    fn new(name: &str) -> Self {
        let base = std::env::temp_dir().join(format!("cvchild-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let repo = base.join("repo");
        let home = base.join("home");
        let bin = base.join("bin");
        for dir in [&repo, &home, &bin] {
            fs::create_dir_all(dir).unwrap();
        }
        let base = base.canonicalize().unwrap();
        let repo = base.join("repo");
        let home = base.join("home").join(".converge");
        converge::use_backend(&home, "hermes").unwrap();
        if cfg!(windows) {
            fs::write(
                base.join("bin").join("hermes.cmd"),
                "@echo off\r\necho child> child.txt\r\necho %CONVERGE_MODEL%> model.txt\r\n",
            )
            .unwrap();
        } else {
            let script = base.join("bin").join("hermes");
            fs::write(
                &script,
                "#!/bin/sh\nprintf child > child.txt\nprintf '%s' \"$CONVERGE_MODEL\" > model.txt\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let mut path = std::ffi::OsString::from(base.join("bin"));
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let fixture = Self { base, repo, path };
        for args in [
            &["init", "-q"][..],
            &["config", "user.email", "t@example.com"],
            &["config", "user.name", "t"],
            &["config", "commit.gpgsign", "false"],
        ] {
            fixture.git(args);
        }
        fs::write(fixture.repo.join("file.txt"), "one").unwrap();
        fixture.git(&["add", "-A"]);
        fixture.git(&["commit", "-qm", "first"]);
        fixture
    }

    fn git(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(&self.repo)
            .output()
            .unwrap()
    }

    fn converge(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_converge"))
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", self.base.join("home"))
            .env("USERPROFILE", self.base.join("home"))
            .env("PATH", &self.path)
            .env_remove("CONVERGE_MODEL")
            .env_remove("CONVERGE_BUDGET_STEPS")
            .output()
            .unwrap()
    }

    fn session_dir(&self) -> PathBuf {
        let workspace = converge::identify(&self.repo).unwrap();
        let sessions = self
            .base
            .join("home")
            .join(".converge")
            .join("projects")
            .join(workspace.repo_key)
            .join("sessions");
        fs::read_dir(sessions)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }

    fn write_workflow(&self, steps: &str) -> PathBuf {
        assert!(self
            .converge(&["mode", "work", "dynamic-workflow"])
            .status
            .success());
        let dir = self.session_dir();
        fs::write(
            dir.join("workflow.json"),
            format!(r#"{{"version":1,"steps":{steps}}}"#),
        )
        .unwrap();
        dir
    }

    fn worktree(&self, dir: &Path, id: &str) -> PathBuf {
        self.repo
            .join(".git")
            .join("converge-worktrees")
            .join(dir.file_name().unwrap())
            .join(id)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn workflow_runs_real_children_in_their_own_worktrees() {
    let fixture = Fixture::new("pass");
    let dir = fixture.write_workflow(
        r#"[{"op":"parallel","id":"group","children":[
            {"op":"agent","id":"alpha","goal":"a","model":"child-model","worktree":true},
            {"op":"agent","id":"beta","goal":"b","model":null,"worktree":true}]},
            {"op":"pipeline","id":"after","children":[{"op":"agent","id":"gamma","goal":"c"}]}]"#,
    );
    let run = fixture.converge(&["workflow", "--test", "test -f file.txt"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );

    for id in ["alpha", "beta", "gamma"] {
        assert!(
            fixture.worktree(&dir, id).join("child.txt").exists(),
            "{id}"
        );
    }
    let model = fs::read_to_string(fixture.worktree(&dir, "alpha").join("model.txt")).unwrap();
    assert_eq!(model.trim(), "child-model");
    assert!(!fixture.repo.join("child.txt").exists());
    assert!(fixture.git(&["status", "--porcelain"]).stdout.is_empty());

    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("workflow.json")).unwrap()).unwrap();
    assert_eq!(document["steps"][0]["status"], "done");
    assert_eq!(document["steps"][0]["children"][1]["status"], "done");
    assert_eq!(document["steps"][1]["status"], "done");
    assert!(fs::read_to_string(dir.join("transcript.jsonl"))
        .unwrap()
        .contains("\"kind\":\"test\""));
}

#[test]
fn a_child_that_fails_its_test_is_not_done_and_stops_the_pipeline() {
    let fixture = Fixture::new("fail");
    let dir = fixture.write_workflow(
        r#"[{"op":"pipeline","id":"line","children":[
            {"op":"agent","id":"first","goal":"a"},
            {"op":"agent","id":"second","goal":"b"}]}]"#,
    );
    let run = fixture.converge(&["workflow", "--test", "test -f absent.txt"]);
    assert_eq!(run.status.code(), Some(1));
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("workflow.json")).unwrap()).unwrap();
    assert_eq!(document["steps"][0]["status"], "failed");
    assert_eq!(document["steps"][0]["children"][0]["status"], "failed");
    assert!(document["steps"][0]["children"][1].get("status").is_none());
    assert!(!fixture.worktree(&dir, "second").exists());
}

#[test]
fn a_finished_step_does_not_run_again_on_resume() {
    let fixture = Fixture::new("resume");
    let dir = fixture.write_workflow(r#"[{"op":"agent","id":"once","goal":"a","status":"done"}]"#);
    let run = fixture.converge(&["workflow", "--test", "test -f file.txt"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!fixture.worktree(&dir, "once").exists());
}
