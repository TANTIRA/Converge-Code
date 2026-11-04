# Invariants

These invariants are engineering contracts. An implementation that breaks one of them is not done, even if the surrounding feature already runs.

1. A model claim does not complete a task. What completes it is the command on `/goal` or `--test`.
2. Only the user changes `/goal`. Bash, the edit tool, and a child cannot clear or replace it.
3. The transcript, goal, mode, and budget are written to disk from the first run.
4. Instructions and memory already loaded are not reloaded in the middle of a turn.
5. A child uses `git worktree`. The parent stays clean until there is a merge.
6. An async child does not block typing. Its result arrives on a later turn. Child permissions appear in the parent thread.
7. A lesson is written only after the completion command exits with code 0, through a kernel tool.
8. Weights change only through the `PromoteWeightEdit` action.
9. When the step, token, or time budget is exhausted, the session escalates to the user. Without a TTY and without an interactive client, the exit code is non-zero.
10. The commit hook runs lint for a user commit and an agent commit.
11. Two IDE clients share one session and one workspace.
12. The resume key is stable on Linux, macOS, and Windows.

## Direct consequences

| Invariant | What must not be done |
|---|---|
| 1 | Ending `converge -p` with code 0 because the model wrote that the task is done |
| 2 | Storing the goal in a file the edit tool or bash can overwrite |
| 3 | Storing the transcript only in process memory |
| 4 | Reloading a skill or the memory index in the middle of the same turn |
| 5 | Running a child in the parent working directory |
| 7 | Writing a lesson from a failed run, including `converge -p` that exits with a non-zero code |
| 8 | Writing weight bytes from bash or from the model process |
| 10 | Skipping lint because a user, rather than the agent, created the commit |
| 12 | Binding a session to a raw path string |

Hooks and the permission mode hold actions. Memory contents do not hold actions.
