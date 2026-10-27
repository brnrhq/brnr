# remote

Read-only checks of an SSH destination or local routing profile.
The group does not accept `--remote`; help stays local.

## `brnr remote check`

```text
brnr remote check (<target> | --profile <p>) [--json]
```

Check that remote brnr is available and matches the local package version,
then run the destination's [`doctor`](doctor.md). The preflight neither
consumes stdin nor prints a successful probe.

| Argument or flag | Meaning |
|---|---|
| `<target>` | SSH alias or `[user@]hostname`; use `brnr` on remote PATH and remote default paths, without reading local brnr configuration. |
| `--profile <p>` | Local routing profile whose `remote` table supplies the destination and optional executable, cwd and environment overrides. |
| `--json` | Print doctor's JSON result; a preflight or routing failure prints a brnr error object. SSH failures may have no JSON. |

Exactly one of `<target>` and `--profile` is required. A profile without a
`remote` table fails. No check creates, edits or deletes local configuration.
SSH authentication and host trust must already be established; see
[remote hosts](../guide/remote.md).

**Output:** doctor's [checks object](doctor.md), with the same fields and
text output. Exit 0 when all checks pass; 1 for routing, preflight or doctor
failure; 2 for usage errors; 255 for SSH failures. See
[exit codes](exit-codes.md).
