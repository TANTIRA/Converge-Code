# Multi-OS

Official targets: Linux, macOS, and Windows. WSL counts as Linux.

CLI flags are the same on all three operating systems. Behavior that passes on one OS must pass on the other two for resume, worktree, and file edits.

## Git

Git is required. A child uses `git worktree`, not a copied directory.

## Shell

| Environment | Command tool |
|---|---|
| Linux, macOS, WSL, Git Bash | Bash |
| Windows without Bash | PowerShell |

There is one command tool. The runtime picks the shell from this table. A model call does not pick a shell by assuming Bash exists on every machine.

## Resume key

The session key is the normalized git root.

Normalization includes:

- the absolute path of the git root
- the OS path separator
- the OS case rules

A raw path-string comparison must not split a session between two processes in the same directory. One repo opened with different path forms remains one session.

## Files

The edit tool must wait or retry when Windows locks a file the editor has open. A lock failure is not a reason to write to another path.

## Shortcuts

| Shortcut | Effect |
|---|---|
| Ctrl-C | Cancels the turn on all three operating systems |
| Esc | Closes a dialog. During a running turn, cancels that turn |
| Ctrl-B | Moves the turn to the background when the terminal supports it |
| Shift+Tab | Cycles permission modes in the terminal: ask-each, accept-edits, plan |

Ctrl-B is not required in every terminal. When the terminal does not support it, a background task can still be started from a work mode that allows spawn, and is seen through `/tasks`.

In an IDE client, the client's mode control replaces Shift+Tab.

## Config

User-owned runtime files live under the OS home directory in `.converge`: `~/.converge` on Linux and macOS, `%USERPROFILE%\.converge` on Windows.

| Path | Contents |
|---|---|
| `~/.converge/instructions.md` | User instructions |
| `~/.converge/projects/<repo_key>/` | One directory per repository |
| `~/.converge/projects/<repo_key>/memory/` | Agent memory for that repository |
| `~/.converge/projects/<repo_key>/sessions/<session_id>/` | That session's record |
| `~/.converge/run/<session_id>.sock` | UHP socket on Linux and macOS. See [02-ide-clients.md](02-ide-clients.md) |

`<repo_key>` is a directory name derived from the canonical path of `git rev-parse --git-common-dir`. Worktrees of one repo share that directory. The directory name is safe as a single path segment on Linux, macOS, and Windows.

Each session record stores the normalized git toplevel of its workspace. Resume loads the session whose workspace matches the current toplevel.

Project and local instruction filenames live in the repo. Scope and filenames are in [07-memory.md](07-memory.md).

## Bypass and isolation

The reference isolated compute for bypass is a Linux module. Windows isolation is its own module, not an assumption of the same behavior as the Linux module. Bypass on a laptop workspace is rejected on every OS. See [11-security.md](11-security.md).
