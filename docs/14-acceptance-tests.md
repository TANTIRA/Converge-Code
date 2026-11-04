# Acceptance tests

Each row is a pass condition that can be run. A model claim does not replace an observation of the repo, the process, or the transcript.

| Test | Passes when |
|---|---|
| Chat | Several turns in a TTY. Tools are visible in the same thread |
| User codes | An edit the user made in an editor is visible on the next turn |
| Tests until pass | The test command exits 0 and `converge -p` also exits 0 |
| Resume | The process dies. The same session continues in the same directory, with the same transcript and goal. The session loaded is the one under `~/.converge/projects/<repo_key>/sessions/` whose stored toplevel matches |
| Three OS | Resume, worktree, and edits pass on Linux, macOS, and Windows |
| Windows without Bash | The command tool uses PowerShell and the command test still runs |
| File lock | An edit still lands in a file the editor has open on Windows, without moving to another path |
| IDE | Two clients on one `session_id` see the same transcript and the same diff. On Linux and macOS they attach to `~/.converge/run/<session_id>.sock`. On Windows they attach to `\\.\pipe\converge-<session_id>` |
| Hook | Lint fails. `git log` does not grow, whether the agent or the user requested the commit |
| Ask | The repo source does not change |
| Plan | The plan exists in the session directory. The source changes only after the plan is approved. Approval leaves the permission mode unchanged and sets the work mode to `dynamic-workflow` when the plan includes a `WorkflowScript`, otherwise `single-agent` |
| Debug | The session stops changing source when the test command on `/goal` exits 0 |
| Single-agent | Spawn is rejected |
| Async | The parent still accepts typing while the child runs. The child is in a worktree. The parent status is clean |
| Workflow | A `parallel` operation creates more than one worktree. `workflow.json` in the session directory is schema version 1. The parent stays clean until merge |
| Multi-task | `/tasks` shows background tasks in the terminal and in an IDE client |
| Goal | The goal remains after compaction and after resume. Bash does not delete it |
| Memory | The lesson is loaded on the next session from `memory/MEMORY.md` and its topic files. The same test fails, the lesson is reverted, and the code edit that already passed stays |
| MCP | The initial context contains only the one server the task uses |
| Bypass | Rejected on a laptop workspace. Accepted only on isolated compute |
| Weight | A weight change outside `PromoteWeightEdit` is rejected |
| Outer loop | The run stops on a passing test, an exhausted budget, or escalation. Without a TTY, failure exits non-zero |
| Lesson on failure | A failed `converge -p` does not add a lesson |
| Instructions | Organization, user, `CONVERGE.md` or `AGENTS.md`, then `CONVERGE.local.md` load in that order. When both project files exist, only `CONVERGE.md` is loaded |

The three-OS test counts WSL as Linux. Windows isolation for bypass is tested as its own module, not as a rerun of the Linux module.
