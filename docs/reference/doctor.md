# doctor

## `brnr doctor`

Checks what brnr depends on, and says what is wrong: the runtime directory,
the session locks, the transcripts, the config file and every profile in
it, how long transcripts are kept (`retention`), the adapters, the running processes, the host logs, and files in a format
this brnr doesn't read.

Even without `--fix`, doctor runs `brnr-claude-adapter`,
`brnr-codex-adapter` and `brnr-muse-adapter` with `--version`, and
`muse-acp` when no npm package identifies it. It resolves these beside brnr
or on the caller's PATH. It reads `package.json` for `claude-agent-acp`,
`codex-acp` and npm `muse-acp` without running them. These probes do not
constrain what the selected executable can do. See
[doctor](../guide/doctor.md).

    brnr doctor [--fix | --report] [--json]

| Flag | Meaning |
|---|---|
| `--fix` | Also make the documented safe repairs: tighten the permissions of directories and files the user owns, and remove stale metadata, sockets and locks. It never touches what it would refuse to use, nor a log, nor the config file. |
| `--report` | Print a bug report to read, then paste into an issue, in place of the checks: brnr's version, the OS, the adapters, the checks that aren't ok, and the end of the most relevant host logs, secrets redacted and the home directory as `~`. It uploads nothing. Not with `--fix`. |
| `--json` | Print the checks, or the report, as JSON. |

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `checks` | list of objects | One for each check, in the order they ran. |
| `checks[].level` | string | `ok`, `info`, `warn` or `fail`. |
| `checks[].check` | string | What it checked: `config`, `runtime dir`, `profile work`, … |
| `checks[].message` | string | What it found, for people. Not covered. |

<!-- example: brnr doctor --json; before: Nothing -->
```json
{
  "checks": [
    {
      "level": "ok",
      "check": "runtime dir",
      "message": "/run/user/1000/brnr: private"
    },
    {
      "level": "ok",
      "check": "config",
      "message": "/home/me/.config/brnr/config.toml: 2 profiles"
    }
  ]
}
```

**Output** (`--report --json`), **not covered**, as `--report` isn't:

| Field | Type | Meaning |
|---|---|---|
| `brnr` | string | brnr's version. |
| `os` | string | The OS, its version and the kernel's: `macOS 26.4 (Darwin 25.4.0, arm64)`. |
| `adapters` | list of objects | The adapters' checks, `{level, check, message}`. |
| `adapters[].level` | string | As in `checks`. |
| `adapters[].check` | string | As in `checks`. |
| `adapters[].message` | string | As in `checks`. |
| `checks` | list of objects | The other checks that aren't `ok`, `{level, check, message}`. |
| `host_logs` | list of objects | The most relevant host logs' ends. |
| `host_logs[].run` | string | The process's run id. |
| `host_logs[].ended` | string | How it ended. |
| `host_logs[].lines` | list of strings | Its last lines, redacted. |

<!-- example: brnr doctor --report --json; before: Nothing -->
```json
{
  "brnr": "0.10.0",
  "os": "macOS 26.4.1 (Darwin 25.4.0, arm64)",
  "adapters": [
    {
      "level": "info",
      "check": "brnr-claude-adapter",
      "message": "not found next to brnr or on PATH"
    }
  ],
  "checks": [],
  "host_logs": []
}
```

Text output: a line for each check, `ok    config: …`; with `--report`,
Markdown. Neither is covered: read `--json`.

**Exit codes:** 0 if no check failed; 1 if one did, how many said on
stderr. A failed check is its data, not an error: it has no error code.
