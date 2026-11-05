# Subagents

A child is a session with its own worktree. The parent stays on its goal. A merge into the parent is a separate step.

## Isolation

Every child uses a path from `git worktree`. `git status` in the parent is clean of the child's work until there is a merge.

A delegation message carries the goal and the worktree path. That message does not carry file contents.

## Async

An async child does not block typing in the parent. The parent's `/goal` stays set. The child's result enters the transcript on a later turn.

A permission the child requests appears in the parent thread, or in the focused IDE client.

Ctrl-B moves the running turn to the background when the terminal supports it. When it does not, the task is still visible through `/tasks`.

## Dynamic workflow

The model writes a `WorkflowScript`. The kernel executes that document. It is not an arbitrary script language. The user can open the file in an editor before approving it.

The file is `workflow.json` inside the session directory (`~/.converge/projects/<repo_key>/sessions/<session_id>/workflow.json`). Encoding is JSON, schema version 1.

```json
{ "version": 1, "steps": [] }
```

A step is:

```json
{ "op": "agent", "id": "string", "goal": "string", "model": null, "worktree": true }
```

`op` is `agent`, `parallel`, or `pipeline`. `model` is a string or null. `parallel` and `pipeline` contain `children`, an array of steps. `agent` has no children.

| Operation | Effect |
|---|---|
| agent | One child |
| parallel | Several children at once |
| pipeline | Children in sequence. The next step starts after the previous step finishes |

Each child may use its own model and its own worktree. The parent goal is not replaced.

In ask and plan, the document may be written and is not yet run. Running a workflow is a kernel action after the user approves.

A script that was interrupted resumes from `workflow.json` in the session directory on resume.

`converge workflow --test <command>` runs the script. Each `agent` step starts a child session, `converge -p <goal> --test <command>`, in a worktree under `<git common dir>/converge-worktrees/<session_id>/`. A step is `done` only when its child exits with code 0, and otherwise `failed`. A failed `pipeline` step stops the steps after it. After every child passes, the command also runs in the parent.

In a terminal chat, `/task <goal> --test <command>` starts a background task in `multi-task` or `dynamic-workflow`.

## Single-agent

The single-agent work mode rejects async spawn and rejects running a workflow. An attempt to create a child fails in the kernel, not silently inside the model.

## Multi-task

The multi-task mode keeps the parent interactive. Several `Task`s run in the background. Each task has its own completion condition.

`/tasks` shows that task status in the terminal and in an IDE client. The kernel function "task status" is the source of that display.

## Modes that reject children

| Mode | Spawn |
|---|---|
| single-agent | Rejected |
| ask | The workflow is not run |
| plan | The workflow is not run until the plan is approved and the session leaves plan |
| debug, dynamic-workflow, multi-task | According to the mode table in [03-modes.md](03-modes.md), with a worktree required when there is a child |
