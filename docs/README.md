# Converge development documentation

These documents are the specification for engineers building Converge. The source is the final HLD. This repo does not yet contain a product implementation.

Read them in order for the full picture. Each file stands alone for the part it locks.

| File | Contents |
|---|---|
| [00-product.md](00-product.md) | Product, surfaces, backends |
| [01-multi-os.md](01-multi-os.md) | Linux, macOS, Windows, WSL |
| [02-ide-clients.md](02-ide-clients.md) | Repo sharing and the UHP client protocol |
| [03-modes.md](03-modes.md) | Work modes and permission modes |
| [04-invariants.md](04-invariants.md) | Contracts that must hold |
| [05-architecture.md](05-architecture.md) | Context, end-to-end flow, planes |
| [06-ontology-kernel.md](06-ontology-kernel.md) | Schema, actions, functions, what each mode admits |
| [07-memory.md](07-memory.md) | Instructions, transcript, agent memory, goal |
| [08-loop.md](08-loop.md) | Inner loop and outer loop |
| [09-subagents.md](09-subagents.md) | Async, workflow, single-agent, `/tasks` |
| [10-planes.md](10-planes.md) | UHP, A2A, MCP, MMDP, kernel, security |
| [11-security.md](11-security.md) | Login, token, hooks, retention, lineage, bypass |
| [12-learning.md](12-learning.md) | Three learning layers and the revert rule |
| [13-interop.md](13-interop.md) | External faces |
| [14-acceptance-tests.md](14-acceptance-tests.md) | Acceptance tests |
| [15-build-order.md](15-build-order.md) | Build order and what milestone 1 contains |

Invariants in [04-invariants.md](04-invariants.md) take precedence over implementation convenience. The build order in [15-build-order.md](15-build-order.md) decides what is in the first binary. Features not yet in that order remain part of the specification, and are not part of the first binary.
