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

fn head(fixture: &Fixture) -> Vec<u8> {
    fixture.git(&["rev-parse", "HEAD"]).stdout
}

fn workflow_with_one_child(fixture: &Fixture) -> PathBuf {
    let dir = fixture.write_workflow(r#"[{"op":"agent","id":"alpha","goal":"add the file"}]"#);
    let run = fixture.converge(&["workflow", "--test", "test -f file.txt"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    dir
}

fn status_of_alpha(dir: &Path) -> serde_json::Value {
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("workflow.json")).unwrap()).unwrap();
    document["steps"][0]["status"].clone()
}

#[test]
fn merge_brings_a_passing_child_into_the_parent_and_cleans_up() {
    let fixture = Fixture::new("merge");
    let dir = workflow_with_one_child(&fixture);
    assert!(!fixture.repo.join("child.txt").exists());

    let merge = fixture.converge(&["merge", "alpha", "--test", "test -f child.txt"]);
    assert!(
        merge.status.success(),
        "{}",
        String::from_utf8_lossy(&merge.stderr)
    );

    assert!(fixture.repo.join("child.txt").exists());
    assert!(fixture.git(&["status", "--porcelain"]).stdout.is_empty());
    let log =
        String::from_utf8_lossy(&fixture.git(&["log", "-2", "--format=%s"]).stdout).into_owned();
    assert!(log.contains("Merge alpha: add the file"), "{log}");
    assert!(!fixture.worktree(&dir, "alpha").exists());
    let branches = fixture.git(&["branch", "--list", "converge/*"]).stdout;
    assert!(
        branches.is_empty(),
        "{}",
        String::from_utf8_lossy(&branches)
    );
    assert_eq!(status_of_alpha(&dir), "merged");
}

#[test]
fn a_failing_test_on_the_merged_tree_undoes_the_merge() {
    let fixture = Fixture::new("undo");
    let dir = workflow_with_one_child(&fixture);
    let before = head(&fixture);

    let merge = fixture.converge(&["merge", "alpha", "--test", "test -f absent.txt"]);
    assert!(!merge.status.success());
    assert!(String::from_utf8_lossy(&merge.stderr).contains("merge undone"));

    assert_eq!(head(&fixture), before);
    assert!(!fixture.repo.join("child.txt").exists());
    assert!(fixture.git(&["status", "--porcelain"]).stdout.is_empty());
    assert!(!fixture
        .git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .status
        .success());
    assert!(fixture.worktree(&dir, "alpha").exists());
    assert_eq!(status_of_alpha(&dir), "done");

    let retry = fixture.converge(&["merge", "alpha", "--test", "test -f child.txt"]);
    assert!(
        retry.status.success(),
        "{}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(fixture.repo.join("child.txt").exists());
}

#[test]
fn a_conflict_undoes_the_merge_and_keeps_the_parent_clean() {
    let fixture = Fixture::new("conflict");
    let dir = workflow_with_one_child(&fixture);
    fs::write(fixture.repo.join("child.txt"), "the parent wrote this").unwrap();
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "-qm", "parent change"]);
    let before = head(&fixture);

    let merge = fixture.converge(&["merge", "alpha", "--test", "test -f file.txt"]);
    assert!(!merge.status.success());

    assert_eq!(head(&fixture), before);
    assert!(fixture.git(&["status", "--porcelain"]).stdout.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("child.txt")).unwrap(),
        "the parent wrote this"
    );
    assert_eq!(status_of_alpha(&dir), "done");
}

#[test]
fn merge_refuses_a_dirty_parent_a_failed_child_and_a_bad_id() {
    let fixture = Fixture::new("refuse");
    let dir = workflow_with_one_child(&fixture);
    fs::write(fixture.repo.join("file.txt"), "my unsaved work").unwrap();
    let dirty = fixture.converge(&["merge", "alpha", "--test", "test -f file.txt"]);
    assert!(!dirty.status.success());
    assert!(String::from_utf8_lossy(&dirty.stderr).contains("uncommitted"));
    assert_eq!(
        fs::read_to_string(fixture.repo.join("file.txt")).unwrap(),
        "my unsaved work"
    );
    fixture.git(&["checkout", "--", "file.txt"]);
    assert!(!fixture
        .converge(&["merge", "../alpha", "--test", "true"])
        .status
        .success());
    assert!(!fixture
        .converge(&["merge", "nobody", "--test", "true"])
        .status
        .success());

    let document = fs::read_to_string(dir.join("workflow.json")).unwrap();
    fs::write(
        dir.join("workflow.json"),
        document.replace("\"done\"", "\"failed\""),
    )
    .unwrap();
    let failed = fixture.converge(&["merge", "alpha", "--test", "test -f file.txt"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("is failed"));
    assert!(!fixture.repo.join("child.txt").exists());
}

#[test]
fn the_commit_hook_can_block_a_merge() {
    let fixture = Fixture::new("hook");
    let dir = workflow_with_one_child(&fixture);
    assert!(fixture.converge(&["hook", "install"]).status.success());
    let lint = fixture.repo.join("lint");
    fs::write(&lint, "#!/bin/sh\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&lint, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let before = head(&fixture);
    let merge = fixture.converge(&["merge", "alpha", "--test", "test -f file.txt"]);
    assert!(!merge.status.success());
    assert_eq!(head(&fixture), before);
    assert!(!fixture.repo.join("child.txt").exists());
    assert_eq!(status_of_alpha(&dir), "done");
}

// The script stands in for a child's model loop, using the same files and environment.
#[cfg(unix)]
fn child_asks_script() -> &'static str {
    r#"#!/bin/sh
printf '%s' "$CONVERGE_CHILD_PERMISSION" > mode.txt
mkdir -p "$CONVERGE_CHILD_ASKS"
printf '{"child":"%s","name":"shell","arguments":"{}"}' "$CONVERGE_CHILD_ID" > "$CONVERGE_CHILD_ASKS/q.ask"
for attempt in $(seq 1 200); do
  [ -f "$CONVERGE_CHILD_ASKS/q.answer" ] && break
  sleep 0.05
done
cp "$CONVERGE_CHILD_ASKS/q.answer" answer.txt
"#
}

#[cfg(unix)]
#[test]
fn a_child_starts_in_the_parent_mode_and_is_stopped_without_a_terminal() {
    let fixture = Fixture::new("asks");
    fs::write(fixture.base.join("bin").join("hermes"), child_asks_script()).unwrap();
    let dir = fixture.write_workflow(r#"[{"op":"agent","id":"alpha","goal":"a"}]"#);
    assert!(fixture
        .converge(&["mode", "permission", "ask-each"])
        .status
        .success());
    let run = fixture.converge(&["workflow", "--test", "test -f file.txt"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let worktree = fixture.worktree(&dir, "alpha");
    assert_eq!(
        fs::read_to_string(worktree.join("mode.txt")).unwrap(),
        "ask-each"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("answer.txt")).unwrap(),
        "stop"
    );
}
