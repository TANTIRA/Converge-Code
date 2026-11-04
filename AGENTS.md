# AGENTS.md

Instructions for coding agents working in this repository. This file is not the product's end-user `CONVERGE.md`.

## Product

- Converge is a Rust CLI: one session on a git repository.
- The specification is in `docs/README.md`.
- Before changing behavior, read `docs/04-invariants.md` and `docs/15-build-order.md`.
- Invariants beat convenience.
- The first binary is the loop, the OS shell, file edit, `--test`, the transcript, and `converge -p`.
- Do not start IDE clients, MMDP, or weight promotion in the first milestone unless the user asks.

## Repository rules

- Docs stay in English.
- Do not commit secrets, API keys, or session transcripts.
- Do not skip git hooks.
- Do not add a bypass of permission checks on a laptop workspace. Bypass exists only for isolated compute.

## Locked layout

- Home directory: `~/.converge` (Windows: `%USERPROFILE%\.converge`).
- Organization instructions: `/Library/Application Support/Converge/instructions.md` (macOS), `/etc/converge/instructions.md` (Linux), `%ProgramData%\Converge\instructions.md` (Windows).
- User instructions: `~/.converge/instructions.md`.
- Project instructions: `CONVERGE.md`. Load `AGENTS.md` only as a fallback when `CONVERGE.md` is absent.
- Local instructions: `CONVERGE.local.md`. It is gitignored.
- Memory and sessions live under `~/.converge/projects/<repo_key>/`, keyed by the canonical git common dir.

## Locked protocols

- UHP IDE transport: Unix socket `~/.converge/run/<session_id>.sock`, Windows pipe `\\.\pipe\converge-<session_id>`, JSON-RPC 2.0.
- WorkflowScript is `workflow.json`, schema version 1, with operations `agent`, `parallel`, and `pipeline`.
- After plan approval, the permission mode is unchanged. The work mode is `dynamic-workflow` if the plan includes a WorkflowScript, otherwise `single-agent`.

## Writing code

- Ship the smallest complete implementation that meets the spec.
- Do not add mocks, stubs, or fakes. Tests and runtime code use real interfaces and real boundaries.
- Keep code secure, usable, scalable, and simple. Prefer a direct design over a clever one.
- Do not add comments that restate the code. Comment only to explain a non-obvious constraint.
- Do not leave dead code.
- Do not hardcode secrets, environment-specific paths, hosts, or values that belong in configuration.
