# Loop

There are two loops. The inner loop is a model turn. The outer loop starts a run and decides when it stops.

## Inner loop

One turn:

1. The kernel supplies the memory already loaded, the goal, the budget, and the tool list the mode admits.
2. The model picks a tool inside that list.
3. The runtime runs the tool in the repo, with the shell for that OS.
4. Hooks run. For a task with a completion command, the hook refuses to stop while that command has not exited with code 0.
5. The result returns to the model, or the turn ends because the user cancelled, the budget is exhausted, or the completion command passed.

Chat without `/goal` and without `--test` does not force a test command on every message. An interactive process stays alive until the user closes it.

A model claim that the work is done does not end a task that has a test command.

## Outer loop

| Part | Rule |
|---|---|
| Trigger | The user in a TTY, an IDE client, `converge -p`, a schedule, or a git event |
| Stop | The completion command exits with code 0, the budget is exhausted, or escalation |
| State | Git for code. The session directory `~/.converge/projects/<repo_key>/sessions/<session_id>/` for the goal, the plan, `workflow.json`, and the transcript |
| Verifier | The test command, or a child that only reads |
| Escalation | Ask in the TTY or the IDE. Without an interactive client, the exit code is non-zero |
| Budget | Steps, tokens, and time, counted in the kernel |

An outer round may be a new process with an empty model context. Progress for that round is read from git and the session store. The goal and the project instructions are still inserted.

The verifier is not a sentence from the model that made the edits. When the verifier is a child, that child only reads.

## Budget

The budget lives on the session. Exhausted steps, tokens, or time escalate the session to the user. Without a TTY and without an interactive client, the process exits with a non-zero code and does not write a lesson.

Model-catalog quotas for sessions off the machine are described in [10-planes.md](10-planes.md). The session budget is still counted in the kernel.

## Boundary between the two loops

| | Inner loop | Outer loop |
|---|---|---|
| Who starts it | A turn that is already running | A trigger |
| Model context | Continues, then may be compacted | May be a new process with an empty context |
| What holds the stop | The hook on the completion command | The test, the budget, or escalation |
| What writes code | The model through an admitted tool | Does not write code itself |
