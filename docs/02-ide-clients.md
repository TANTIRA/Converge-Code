# IDE clients

There are two relationships with an IDE. Both are valid. Only one uses a protocol.

## Sharing the repo

VS Code, Cursor, JetBrains, Devin, and other editors always share the git repo. Their file edits and commits are visible on the next turn without a plugin. That is enough for the "user codes" test.

## Active client

An active connection uses the UHP client protocol. The IDE is not a second agent. Tools, hooks, modes, `/goal`, worktrees, and the budget stay in the kernel.

| Client | Form |
|---|---|
| VS Code and Cursor | One client implementation. Cursor loads the same client |
| JetBrains | A second client. The CLI must already be installed on the machine |
| Devin and other IDEs | Only if they use the same protocol. Not a kernel branch |

## Messages

The client sends:

- user text
- selection or editor context

The client receives:

- reply stream
- tool events
- permission prompts
- diffs

Every message carries the `session_id` of the session it is attached to.

Two clients may attach to one `session_id`. Input enters the transcript in arrival order. Permission prompts appear in the focused client.

### Transport

The CLI owns the TTY. IDE clients attach to the socket. They do not speak to the model directly.

| OS | Endpoint |
|---|---|
| Linux and macOS | Unix domain socket at `~/.converge/run/<session_id>.sock` |
| Windows | Named pipe `\\.\pipe\converge-<session_id>` |

Encoding is JSON-RPC 2.0, UTF-8.

| Direction | Methods |
|---|---|
| Client to kernel | `session/attach`, `session/input`, `session/selection`, `permission/respond` |
| Kernel to client | `session/stream`, `session/tool`, `permission/ask`, `session/diff`, `session/status` |

`permission/ask` carries `id`, `name`, and `arguments`, the text of the call. A client shows that text before the user answers. `permission/respond` carries the same `id` and `allow`. The kernel drops an answer whose `id` is not the open question.

## What a client does not do

A client does not run its own model loop. A client does not write lessons. A client does not create a worktree outside the kernel. A client does not open bypass mode on a laptop workspace.
