# Interoperability

Every face uses the same session, kernel, and modes. There is no seventh plane.

| Face | Contract |
|---|---|
| Terminal and IDE | One UHP session, one transcript, one goal. IDE clients use the transport in [02-ide-clients.md](02-ide-clients.md) |
| `converge -p` | The exit code follows the test, the budget, and denial of an action that would ask |
| Git | Workspace, worktree, and code |
| MCP | External tools whose schemas are loaded when used |
| REST, S3, JDBC | MMDP tables and files when the session is off the machine |
| Metadata | Attribution and lineage already written once. Technical metadata does not become a property of `Model` |
| Webhook | A side effect of a kernel action, per step |

## Data off the machine

A table read from outside is an Iceberg or Parquet table in MMDP, or a virtual catalog that points at that table. Weight files and other files stay in their original format and are referenced with a URI plus a digest.

A JDBC read carries the purpose on the same session token.

## Metadata

The metadata service provides a read of attribution and lineage for an external catalog. Lineage writes stay in the pipeline and in kernel actions.

## Webhooks

A webhook runs as part of a kernel action that is already allowed, for example after `PromoteWeightEdit` or another action that declares the side effect. A webhook is not an exit path from bash.

## Logic outside the Rust binary

The Converge binary and the kernel are Rust. A transform on MMDP compute may use a runtime registered as a compute module. That module does not call the loop or A2A.
