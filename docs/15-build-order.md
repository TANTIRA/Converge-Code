# Build order

One crate named `converge` until a module is used without the loop. Splitting a crate happens when that use is real, not in milestone 1.

The specification in the other files stays whole. This order only decides what enters the binary at each step.

## Milestone 1

A `converge` binary sufficient for two tests: tests until pass, and `converge -p` that fails with a non-zero code.

Required contents:

- One CLI process.
- The working directory is a git repo.
- Two tools: edit a file, and run a command with the shell for that OS.
- One model adapter behind an interface that can gain another backend without changing the loop.
- `--test` as the completion command.
- A hook that refuses to stop while `--test` has not exited with code 0.
- The transcript, the goal when one is set, the mode, and the budget are written to disk from the first run, under `~/.converge/projects/<repo_key>/sessions/<session_id>/`, keyed so resume matches the normalized git toplevel.
- `converge -p` without a TTY. An action that would ask is denied. Failure exits with a non-zero code.
- A failed run does not write a lesson.

Milestone 1 is not required to include: IDE clients, all four backends, MCP, workflow, MMDP, weight objects, or bypass mode.

`<repo_key>` is still the canonical `git rev-parse --git-common-dir` key from [01-multi-os.md](01-multi-os.md), so the first binary already writes sessions where later worktrees can find them.

## Later steps

| Order | Contents | Tests that turn green |
|---|---|---|
| 1 | Milestone 1 contents | Tests until pass. `converge -p` fails with a non-zero code |
| 2 | A resume key that is stable on Linux, macOS, and Windows, including PowerShell and file locks | Resume. Three OS |
| 3 | Commit hook | Lint fails, `git log` does not grow |
| 4 | Child worktree | Parent stays clean |
| 5 | TTY chat, then the VS Code and Cursor client, then JetBrains, on the UHP socket or named pipe | Chat. IDE |
| 6 | `/goal`, ask, plan, and debug, including the post-approval work-mode switch | Goal. Ask. Plan. Debug |
| 7 | One MCP server; other servers stay out of the initial context | MCP |
| 8 | Write-lesson action and revert, under `memory/MEMORY.md` | Memory. Lesson on failure |
| 9 | Async, `/tasks`, and workflow `agent` / `parallel` / `pipeline` as `workflow.json` schema version 1 | Async. Workflow. Multi-task. Single-agent rejects spawn |
| 10 | Outer loop, budget, and further IDE clients on the same protocol | Outer loop |
| 11 | Weight kernel and MMDP for sessions off the machine, including bypass only on isolated compute | Weight. Bypass |

Devin and other IDEs follow at step 10 when they use the same client protocol. They do not add a kernel type.

## Crate

Milestone 1 is the `converge` crate alone. A module inside that crate may become its own crate when the module is used without the loop. Example split triggers: an MMDP catalog read without a session, or a client that only attaches to UHP. Until that trigger exists, the code stays in `converge`.
