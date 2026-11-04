# Operating policy

Policy for people and agents building Converge.

- The specification source is `docs/`. Invariants in `docs/04-invariants.md` win conflicts.
- The language of the docs and of these repository governance files is English.
- Keep one crate named `converge` until a module is used without the loop.
- Permission modes are `ask-each`, `accept-edits`, `auto`, `dontAsk`, and `bypass`. Bypass is rejected on a laptop workspace and allowed only on isolated compute.
- After a plan is approved, the permission mode stays unchanged. The work mode is `dynamic-workflow` if the plan includes a WorkflowScript, otherwise `single-agent`.
- Changes that affect session resume, worktrees, or permission must keep the three-OS contract in `docs/01-multi-os.md`: Linux, macOS, and Windows. WSL counts as Linux.
- Acceptance tests in `docs/14-acceptance-tests.md` are the gate for calling a feature done.
- Agents follow `AGENTS.md`.
