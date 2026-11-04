# Security

Security decides and records. Security is not a path for bytes.

## Login

Local login uses an API key or OAuth. A session off the machine may use an IdP the organization already has.

## Session token

The session token carries:

- role
- classification
- purpose
- the budget of steps, tokens, and time

The permission mode is how that token is applied to workspace tools. Purpose is checked together with role and classification when the session uses organization authorization. A credential that reads tables off the machine carries the same purpose, so the SQL path is the same check.

## Hooks

The commit hook runs lint for a user commit and an agent commit. When lint fails, the commit is not created and `git log` does not grow.

The hook on the completion command refuses to stop while the test command has not exited with code 0.

Hooks hold actions. Memory contents do not hold actions.

## Retention

Retention is a kernel action on memory notes and MMDP objects. Retention does not delete git commits the user owns.

Access history remains queryable after an object is retired.

## Lineage

Lineage is written once by a pipeline and by a kernel action. The metadata service reads that record. There is no second lineage store written separately.

`PromoteWeightEdit` writes lineage inside the same action as the child link, the classification check, and publication of the URI.

## Data that leaves the machine

Prompts and files sent to a model are data that leaves the machine. That send is subject to the same permission mode as other tools.

## Bypass

| Place | Bypass |
|---|---|
| Laptop workspace, every OS | Rejected |
| Isolated Linux compute | Allowed. All workspace tools run without asking |
| Windows isolation | Its own module. Not a copy of the Linux module's assumptions |

Bypass is not in the Shift+Tab cycle. An IDE client does not open bypass on a laptop workspace.

Without a TTY, modes other than bypass on isolated compute deny an action that would ask and exit with a non-zero code.
