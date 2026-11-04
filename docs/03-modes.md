# Modes

Work mode and permission mode are two axes. The status bar shows both.

`Shift+Tab` in the terminal cycles ask-each, accept-edits, and plan. Debug, single-agent, dynamic-workflow, multi-task, auto, dontAsk, and bypass are not in that cycle. In an IDE, the client's mode control performs the same cycle for those three permission modes.

## Work modes

| Work mode | The kernel admits | Done when |
|---|---|---|
| ask | Read and answer. No source-file edits. No command that changes state | The answer turn ends |
| plan | Read, plus write a plan in the session store. The repo source is not edited | The user approves the plan, rejects it, or keeps planning |
| debug | Read, the test command on `/goal`, and source edits | The test command exits with code 0 |
| single-agent | The full loop under the permission mode. Spawn is rejected | The completion command on `/goal` passes, or the user closes the turn |
| dynamic-workflow | The model writes an orchestration script. The runtime runs it | The script finishes and the completion command passes |
| multi-task | The parent stays interactive. Several tasks run in the background | Each task has its own completion condition. `/tasks` shows all of them |

A workflow script is a document the kernel runs, with three operations: `agent`, `parallel`, and `pipeline`. The user can open that document in an editor before approving it. In ask and plan, the document may be written and is not yet run.

The user selects the work mode. A slash name for changing the work mode is not locked.

## After a plan is approved

Approving a plan leaves the permission mode unchanged.

| User choice | Work mode afterward |
|---|---|
| Approve, and the plan includes a `WorkflowScript` | `dynamic-workflow` |
| Approve, and the plan does not include a `WorkflowScript` | `single-agent` |
| Reject the plan, or keep planning | `plan` |

## Permission modes

| Permission mode | Without asking | With a TTY | Without a TTY |
|---|---|---|---|
| ask-each | Reads | Edits, commands, and network are asked | Anything that would ask is denied, non-zero code |
| accept-edits | Reads and edits inside the workspace | Other commands are asked | Anything that would ask is denied, non-zero code |
| auto | Actions that pass a separate checker | Rejections are shown | A rejection stops the run with a non-zero code |
| dontAsk | Reads and tools already allowed | Anything that would ask is denied | Non-zero code |
| bypass | All workspace tools | Only on isolated compute | Only on isolated compute |

The kernel rejects bypass on a laptop workspace.

`converge -p` has no TTY. A permission that needs a person follows the without-TTY column. The process does not hang waiting for an answer.

## Intersections that must hold

| Work | Permission | Result |
|---|---|---|
| ask or plan | any | A workflow script may be stored. That script is not run |
| debug | ask-each or accept-edits | Edits exist only to make the test command on `/goal` pass. After it passes, source writes are closed |
| single-agent | anything except bypass on a laptop | Async spawn and workflow are rejected |
| dynamic-workflow or multi-task | ask-each | Child permissions appear in the parent thread |
| any | bypass on a laptop | Rejected |

The checker in auto mode is a party separate from the model that proposes the action. Rejections are shown to the user when there is a TTY or an IDE client.
