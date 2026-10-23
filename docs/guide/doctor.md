# Doctor

```sh
brnr doctor                        # checks what brnr depends on
brnr doctor --fix                  # … and repairs what it safely can
brnr doctor --report               # a bug report to read, then paste into an issue
```

It checks that the runtime directory is private, isn't a symlink, is short
enough for socket paths, and holds nothing left by processes that are gone
(their metadata and sockets); that nothing keeps a session from being locked,
and no lock is left by a process that is gone; that transcripts are readable
only by you; that the config parses, with every key in its part, and each
profile's agent, cwd, MCP servers and bridges are valid; where the adapters
are found; and that every running process answers, naming the sessions one
that doesn't still serves. A process is gone when nobody listens on its
socket, even if its pid now belongs to another process (after a reboot, say);
one that is stopped is running but not answering. macOS also refuses a
connection to a stopped process whose socket's backlog is full, so there a
refusal counts only if the process with that pid started after brnr's file
was written.

It also lists the host logs (`~/.brnr/hosts/`) of processes that died
without recording it, killed by SIGKILL for instance: logs without `process_exited`
whose process isn't running. The line says how many, and for the latest three
when each last wrote and the sessions it had open; it is information, not a
warning. So is its list of the files in a format this brnr doesn't read
(`file formats`), transcripts and host logs, an older brnr's or a newer
one's (ADR 5.10). `--fix` makes the runtime directory and transcripts private (only
what you own, never through a symlink; an access control list stays, for
you to remove) and removes what processes that are
gone left behind; it leaves logs alone. It exits non-zero if a check fails;
`--json` prints `{checks}`, each a `level`, `check` and `message`.

`--report` prints, instead of the checks, what a bug report needs, as
Markdown to paste into an issue: brnr's version, the OS, the adapters, the
checks that aren't ok, and the last 20 lines of the latest host log and of
the latest one of a process that panicked or died without recording it.
Secrets are redacted as brnr records them, and your home directory is `~`;
the log lines are otherwise as recorded, prompts included, so read it before
you paste it. With `--json` it is one object (`brnr`, `os`, `adapters`,
`checks`, `host_logs`) with the same data. Nothing is sent anywhere.

When brnr panics, it prints a link to a new issue with the version, the OS
and the panic's message filled in; nothing is sent unless you open it and
submit the form. The link is printed on the command's stderr; for a process
that runs detached, by `brnr session new` or `brnr acp` when its start fails with
the panic, and in the foreground by the process itself.
