# Planes

Six planes. Workspace bytes do not move through security. File contents do not move through A2A.

## UHP

UHP owns the lifecycle of the process and of clients.

| Capability | Contract |
|---|---|
| Start | One `session_id` for the terminal, `converge -p`, and attached IDE clients |
| Stream | Replies, tool events, and diffs to the terminal and to attached clients |
| Cancel | Ctrl-C, Esc, or cancel from a client. A cancelled task does not write a lesson |
| Resume | Load the session under `~/.converge/projects/<repo_key>/sessions/` whose stored workspace toplevel matches the current toplevel. That record holds the transcript, goal, mode, and budget |
| `/tasks` | The list of background `Task`s from the task-status function |

UHP unifies lifecycle across backends. UHP does not unify bash, a backend-specific edit tool, or a backend-specific skill.

## A2A

A2A carries delegation between sessions.

Message contents:

- the parent `session_id`
- the goal
- the child worktree path

File contents do not enter the message. A file part, when one exists, contains a URI and a digest.

A child whose result is required for `converge -p` is awaited when the work mode requires that result. In an interactive session, an async child does not block typing.

## MCP

MCP is external tools.

- A grant is derived from the session permission.
- The initial context contains only the schema of the server this task uses.
- Other servers may exist in configuration without entering the initial context.
- Bash, compaction, and scratch stay inside the session, outside the MCP tool list.

## MMDP

MMDP is data, compute, and the model catalog when the session leaves the laptop.

On the laptop, the source of truth is the git repo. MMDP is used for CI, schedules, cloud, and promoted weights.

| Part | Contract |
|---|---|
| Tables | Iceberg or Parquet |
| Files | Original format. Not forced into table rows |
| Data catalog | May be virtual. Using data does not require a copy into another store |
| Model catalog | A quota per use case. Platform models and self-registered models are equal |
| Compute | Ephemeral for cloud sessions. Linux isolation is the bypass reference. Windows isolation is its own module |

What remains after cloud compute is gone: git, the transcript, and objects that have been promoted.

A kernel function may call MMDP compute. An MMDP module does not call the loop or A2A.

## Handle

When an object leaves the machine, the handle carries:

| Field | Meaning |
|---|---|
| digest | Identity of the contents |
| URI | Location in the catalog |
| model_id | The related model |
| parent digest | The parent checkpoint or artifact |
| session_id | The session that produced it |
| trace_id | The join key to lineage |

## Kernel

The kernel holds the schema, functions, and actions in [06-ontology-kernel.md](06-ontology-kernel.md). Other planes do not create durable actions outside the kernel.

## Security

Security gives permission decisions, hooks, the budget, and lineage. Bytes do not pass through security. Details are in [11-security.md](11-security.md).
