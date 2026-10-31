# remote

Check an SSH destination's brnr by starting system SSH and running the
destination's doctor, including its adapter `--version` probes. It writes
no local configuration. System SSH is the `ssh` on the caller's PATH; a
planted executable can run local code without network access or SSH keys.
See [restricted agents](../guide/agents.md#brnr-from-a-restricted-agent).
The group does not accept `--remote`; help stays local.

## `brnr remote check`

```text
brnr remote check <target> [--json]
```

Check that `brnr` is on the remote shell's PATH and matches the local
package version, then run the destination's [`doctor`](doctor.md). The
preflight neither consumes stdin nor prints a successful probe.

| Argument or flag | Meaning |
|---|---|
| `<target>` | SSH alias or `[user@]hostname`. brnr reads no local brnr configuration. |
| `--json` | Print doctor's JSON result; a preflight failure prints a brnr error object. SSH failures may have no JSON. |

No check creates, edits or deletes local configuration. SSH authentication
and host trust must already be established; see
[remote hosts](../guide/remote.md).

**Output:** doctor's [checks object](doctor.md), with the same fields and
text output. Exit 0 when all checks pass; 1 for preflight or doctor
failure; 2 for usage errors; 255 for SSH failures. See
[exit codes](exit-codes.md).

**Errors:** `remote_brnr_not_found` (no `brnr` on the remote shell's PATH:
set it in `~/.zshenv` or `~/.bashrc` there), `remote_version_mismatch`
(another version, or one without remote support).
