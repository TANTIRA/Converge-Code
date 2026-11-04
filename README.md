# Converge

Converge is a Rust CLI for agentic coding on a git repo: one session, multiple model and harness backends.

## Status

`converge -p` is the first binary. It edits files, runs the OS shell, and finishes only when `--test` exits 0. Later surfaces stay in the specification.

## Start here

- [docs/README.md](docs/README.md) — development specification index
- [docs/15-build-order.md](docs/15-build-order.md) — first milestone
- [docs/04-invariants.md](docs/04-invariants.md) — invariants
- [AGENTS.md](AGENTS.md) — repository guidance for coding agents
- [SECURITY.md](SECURITY.md) — security
- [POLICY.md](POLICY.md) — policy

Invariants take precedence over convenience. The build order decides what belongs in the first binary.

## Platforms

Supported operating systems are Linux, macOS, and Windows. WSL counts as Linux.

## Continuous integration

CI lives at [.github/workflows/ci.yml](.github/workflows/ci.yml). It runs `cargo fmt --check` and `cargo test` only after `Cargo.toml` exists.

## License

The license is in [LICENSE](LICENSE).
