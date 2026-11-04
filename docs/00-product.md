# Product

Converge is one CLI written in Rust. The user chats in the terminal on a git repo, and edits that same repo from an editor. One prompt, one transcript, one `/goal`, one budget.

## Surfaces

| Surface | Role |
|---|---|
| Terminal | Chat. The transcript scrolls above, the input box sits below. Model replies and tools enter the same thread |
| `converge -p` | The same session without a TTY, for pipes and CI |
| IDE client | Attaches to the same UHP session. See [02-ide-clients.md](02-ide-clients.md) |

The user may edit files, run tests, and create commits from an editor or from git. The next turn reads the working tree that has already changed. Resume loads the transcript, the goal, and the mode at the same git root.

`converge -p` does not open an input box. A permission that needs a person follows `dontAsk`: the process exits with a non-zero code.

## Backends

Hermes, Codex, Grokbot, and Claude Code are backend adapters behind one prompt. The user does not open four CLIs.

UHP unifies start, stream, cancel, and resume. Each backend keeps its own tool surface. The first binary is not required to include all four adapters. See [15-build-order.md](15-build-order.md).

## What the user sees in an interactive session

- Several turns in one process.
- Tools are visible in the thread: commands that ran and files that were edited.
- Allow, deny, and ask appear in the thread, or in the focused IDE client.
- Esc cancels the turn that is running.
- Ctrl-C cancels the turn. The press that closes the process follows the shortcut contract in [01-multi-os.md](01-multi-os.md).
- The status bar shows the work mode and the permission mode.

Ordinary chat does not force a test command on every message. The process stays alive until the user closes it. The test gate applies when the turn is a task with `/goal` or `--test`.

## Named inputs

| Input | Effect |
|---|---|
| `/goal` | Writes, shows, or clears the session goal. The user only |
| `/tasks` | Shows background tasks |
| `--test` | The completion command for a `converge -p` run or a task that uses it |
| `Shift+Tab` | Cycles the permission modes that are in the terminal cycle |

Other slash names for changing the work mode are not locked. The user selects the work mode, and it is visible on the status bar.
