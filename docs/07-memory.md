# Memory

Memory is not a separate plane. Four stores enter a turn under different rules. Memory becomes context. Hooks and the permission mode hold actions.

## Stores

| Store | Writer | Loaded |
|---|---|---|
| Organization, user, project, then local instructions | User | Start of the session, from the broadest scope to the most specific |
| Transcript | Runtime | Resume. Not instructions |
| Agent memory index, then topic files | Agent through a kernel tool | Start of the session. The index is capped at 200 lines or 25 KB, whichever is reached first |
| Goal | User through `/goal` | Every turn, including after compaction and after resume |

Instructions load from broad to specific: organization, then user, then project, then local.

| Scope | File |
|---|---|
| Organization | `instructions.md` at `/Library/Application Support/Converge/instructions.md` on macOS, `/etc/converge/instructions.md` on Linux, and `%ProgramData%\Converge\instructions.md` on Windows |
| User | `~/.converge/instructions.md` (`%USERPROFILE%\.converge\instructions.md` on Windows), for every repo that belongs to that user |
| Project | `CONVERGE.md` at the repo root, shared through git. If `CONVERGE.md` is absent and `AGENTS.md` is present, load `AGENTS.md`. If both exist, load only `CONVERGE.md` |
| Local | `CONVERGE.local.md` at the repo root. It is gitignored |

`<repo_key>` and the session directory are defined in [01-multi-os.md](01-multi-os.md). Agent memory for a repository is `~/.converge/projects/<repo_key>/memory/MEMORY.md`. That file is the index: the first 200 lines or 25 KB, whichever is reached first. Topic files are sibling markdown files in that same memory directory.

A session record lives at `~/.converge/projects/<repo_key>/sessions/<session_id>/`. It stores the normalized git toplevel of its workspace. Resume loads the session whose workspace matches the current toplevel. Worktrees of one repo share `<repo_key>` because the key is the canonical path of `git rev-parse --git-common-dir`.

## Agent note types

| Type | When it is written |
|---|---|
| feedback | The user corrects something in the thread |
| lesson | After the completion command exits with code 0, through the write-lesson action |
| project | A working fact that cannot be read from the code or from git history |
| reference | A pointer to information outside the repo |

All worktrees of one repo share the same agent memory. A child does not have a memory store separate from the parent repo.

A note that can be derived from the code is not written as memory. Instructions already loaded are not copied again into an agent note.

## Goal

`/goal` writes the goal on `Session`, plus a completion command when the user states one. The runtime inserts that goal on every turn.

A chat message adds a constraint. That message does not replace the goal. Only a new `/goal` or a clear by the user changes it.

A `/goal` without a completion command is still inserted. The runtime cannot prove that task is done. A task that must finish uses the goal and the test command together.

The goal is stored in the session directory under `~/.converge`, not in a file a workspace tool can overwrite.

## Transcript

The transcript stores the chat and is sufficient for resume. The transcript is not copied into instructions and does not become a lesson.

A new process at the same git root loads the transcript, the goal, the mode, and the budget. A new `converge -p` continues the newest unfinished session whose stored toplevel matches, and a session whose completion command already exited 0 starts a new session.

## Compaction

Compaction drops old messages from the model window. What is kept:

- the goal
- the last test result
- the project instructions already loaded at the start of the session

Instructions and memory are not reloaded in the middle of a turn. A version written during the turn is used on the next process.
