# Converge

Converge is a Rust CLI for agentic coding on a git repo: one session, multiple model and harness backends.

## Status

`converge -p <prompt> --test <command>` runs one session and finishes only when the test command exits 0. On a terminal, `converge` opens the chat screen. Piped stdin stays a line protocol. `converge setup` asks for a provider and stores the API key for later runs.

Install the binary with:

```sh
cargo install --path . --bin converge
```

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

CI lives at [.github/workflows/ci.yml](.github/workflows/ci.yml). It runs `cargo fmt --check` and `cargo test` on Linux, macOS, and Windows. A separate job calls the model only when `ANTHROPIC_API_KEY` is configured, and it does not print the key.

## License

The license is in [LICENSE](LICENSE).
