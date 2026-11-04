# Security policy

## Reporting

Report vulnerabilities privately. Contact the maintainers through a private channel.

If no private contact is published, open a private security advisory on the repository host. Do not file a public issue.

## Threat boundaries

These boundaries are part of the specification in `docs/11-security.md` and `docs/04-invariants.md`.

- The kernel decides permissions.
- Bytes do not pass through the security service.
- Prompts and files sent to a model leave the machine and follow the permission mode.
- Bypass is rejected on a laptop workspace and allowed only on isolated compute.
- `converge -p` has no TTY. When an action would ask a person, the process fails closed.
- Retention must not delete the user's git commits.
- Lineage is written once.

## Secrets

Never commit API keys, tokens, or the contents of `~/.converge`.

## Supported operating systems

Linux, macOS, and Windows. WSL counts as Linux.
