# Recursive learning

Learning happens inside the same loop. The runtime writes a lesson after the task is verified. The next session loads that lesson. A skill that is already loaded is not reloaded in the middle of a turn.

## Three layers

| Layer | What is written | When it is used |
|---|---|---|
| Task | Edits in the repo | When the completion command exits with code 0 |
| Lesson | A `MemoryNote` of type `lesson`: symptom, rule, how to test | The next session in the same repo |
| How to learn | A change to the skill that writes lessons | After the child worktree passes and the result is merged |

The third layer is written only in a child worktree. The parent stays clean until the merge. Depth stops at the third layer. A session does not spawn a fourth layer.

Lessons live in `~/.converge/projects/<repo_key>/memory/`, the shared memory of the repo. See [07-memory.md](07-memory.md).

## Write condition

A lesson is written only when the completion command exits with code 0, through the kernel write-lesson action.

A failed run, including `converge -p` with a non-zero code, does not add a lesson. The failure trace stays in the transcript.

`feedback`, `project`, and `reference` follow [07-memory.md](07-memory.md). They are not lessons and do not replace the test condition on `lesson`.

## Revert

A lesson that makes the next session fail the same test is reverted. The code fix in the repo stays.

Revert removes the lesson from the memory that will be loaded. Revert does not undo task edits that already passed in the earlier session.

## What this does not do

- Copy the transcript into instructions.
- Write a lesson in the middle of a turn before the test passes.
- Change weights as part of a learning layer. Weights still go through `PromoteWeightEdit`, and only when the evaluation passes.
