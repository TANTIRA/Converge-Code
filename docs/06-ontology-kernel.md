# Ontology kernel

The kernel is the only door for session objects and durable actions. The kernel holds the schema plus typed functions and actions.

Workspace tools are admitted according to the work mode and the permission mode. The kernel records the result. The kernel is not a hop that bytes pass through.

## Objects that always exist

| Type | Role |
|---|---|
| Workspace | Git root of the session |
| Session | One transcript that can be resumed. Carries the mode and the budget |
| Goal | The goal the user sets with `/goal` |
| Task | One task, including a background task |
| Skill | Instructions loaded at the start of the session |
| Hook | A gate on a specific action, including commit |
| Permission | Allow, deny, or ask for a tool pattern, matching the active permission mode |
| ChildSession | A child session plus the worktree path |
| MemoryNote | One agent memory note |
| WorkflowScript | `workflow.json` in the session directory. Schema in [09-subagents.md](09-subagents.md) |

Technical objects that the user does not search for may still exist inside UHP for `session_id` and the identity of a delegated task. They do not replace the types in this table.

## Actions

| Action | Effect |
|---|---|
| Write lesson | Adds a `MemoryNote` of type `lesson` after the completion command exits with code 0 |
| Approve plan | Leaves the permission mode unchanged. Work mode becomes `dynamic-workflow` when the approved plan includes a `WorkflowScript`, otherwise `single-agent`. Rejecting the plan or keeping planning stays in `plan` |
| Run workflow | Runs a `WorkflowScript` that has been approved |
| Cancel task | Stops a task or the session. A cancelled task does not write a lesson |
| PromoteWeightEdit | The only weight change. See the weight objects below |

## Functions

| Function | Result |
|---|---|
| Remaining budget | Steps, tokens, and time left on the session |
| Task status | The state of each `Task`, for `/tasks` and IDE clients |
| Effective classification | The `WeightEdit` classification derived from the parent `Checkpoint` |

## What each work mode opens

| Work mode | Workspace tools | Kernel actions |
|---|---|---|
| ask | Read | No source writes, no run-workflow |
| plan | Read. The plan is written to the session store | Approve or reject the plan. The workflow is not run |
| debug | Read, the test command on the goal, source edits until the test passes | After the test passes, source writes close. A lesson is allowed when the lesson conditions are met |
| single-agent | According to the permission mode | Spawn is rejected |
| dynamic-workflow | According to the permission mode, plus writing the script | Run the workflow after the script is approved |
| multi-task | According to the permission mode on the parent | Several background `Task`s, each with a `ChildSession` when that task has a child |

Ask and plan may store a workflow document. Storing is not running.

## Weight objects

These objects exist when the repo contains weights.

| Type | Role |
|---|---|
| Model | Identity of a model |
| Checkpoint | One weight state. Classification lives here |
| WeightEdit | An edit between two checkpoints. `editStatus`: `draft`, `promoted`, or `aborted`. Effective classification is derived from the parent |
| Evaluation | `MetricResult`: name, value, unit, confidence, reason |
| Harness | A backend, with a `kind` property |
| Run | One execution decision |

`WeightEdit` is the object-backed relationship between a parent checkpoint and a child checkpoint. A direct link to the parent remains for navigation. Layer extent and reason live on `WeightEdit`.

`PromoteWeightEdit` in one action combines:

- the link to the child checkpoint
- the classification check
- publication of the URI
- the lineage record

`aborted` status lives on `WeightEdit`. There is no second version object for a cancelled state.

A weight change outside this action is rejected, including a bash write and a write from the model process.
