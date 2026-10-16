# 64. What the compatibility promise covers

Accepted 2026-10-14. Amends 1 (P9) and 35. Resolves #148. Nothing to
implement: the promise starts at 1.0.0, and until then P9 is as it was.

Amended by 65: the session locks are in the state directory
(`$BRNR_HOME/sessions/<id>.lock`), not the runtime directory, so where they
are is covered and what they contain isn't; ADR 65 lists how both
directories are found and the state directory's layout.

## Context

P9 has no backwards compatibility before 1.0, and from 1.0.0 on
"compatibility is kept within a major version", but not of what. brnr has
several interfaces, and each is a different kind of promise (#148): the
command line, `--json` output and text output, exit codes, the protocol
bridges speak, event objects, transcripts and host logs under `~/.brnr`,
config TOML, the Rust library on crates.io, and what `brnr acp` passes
between the editor and the agent. What brnr's files said was the pre-1.0
policy only: `docs/interface.md`, SECURITY.md ("Before 1.0, only the latest
release gets fixes"), CHANGELOG.md's conventions, CONTRIBUTING.md and
AGENTS.md.

Two of them don't split along the list:

- Bridges and the control socket speak one protocol. A started bridge writes
  requests on its stdout and reads replies and events on its stdin; a client
  connected to `$BRNR_DIR/<pid>.sock` writes and reads the same lines
  (`command` in src/host/control.rs answers both alike; a peer differs only in
  how it is cut off). ADR 35 calls both bridges. But the socket is in the
  runtime directory, which is private (P13, ADR 50): it is how brnr's own
  commands, and the bridges brnr starts, reach a process, not an interface.
  Leaving the protocol out with it would leave a bridge's requests, such as
  answering a permission, with no promise.
- The library. Cargo's semver rules bind every public item of a crate on
  crates.io (ADR 47), whatever its docs say. #147 decided that the library
  is private, with a facade that mirrors the CLI, and that what the crate's
  own tests and fuzz targets reach is behind a feature, off by default.

## Decision

### P9 from 1.0.0

From 1.0.0, P9's "compatibility is kept within a major version" is of what
is covered below, and of nothing else. Before 1.0.0, P9 is as it was:
changes are made outright, with no aliases or shims.

### Covered

- **The command line**: its commands, their flags and positional arguments,
  and what each means. The library's facade (#147) is covered as part of
  it: it mirrors the command line, and changes as it does. Cargo's semver
  binds every public item on crates.io anyway.
- **`--json` structures**: what each command prints with `--json`, its fields,
  their types and what they mean. A field that is a message for people
  carries text whose wording isn't covered (see below).
- **Exit codes**: what each command exits with, in each situation
  `docs/interface.md` lists.
- **Events for bridges**: each event's name and fields, wherever it goes: a
  bridge's stdin, `--json` lines (`event log`, `event watch`, `session new
  --foreground`), `event notify`'s stdin and its `BRNR_*` variables.
- **The bridge protocol, as a bridge brnr starts sees it**: a bridge started
  from the profile, its requests on its stdout and its replies and events on
  its stdin: each request's `cmd`, its fields and its reply; how long a
  bridge lasts (events until its process exits, then stdin EOF, then
  SIGTERM); and `BRNR_PID` in its environment. Only bridges brnr starts are
  covered: a client that connects to the control socket itself has no
  promise, whatever it speaks.
- **Transcripts**: the events file and the raw ACP file of each session
  under `$BRNR_HOME` (`projects/<folder>/<session id>.jsonl` and
  `.acp.jsonl`), and their records. A 1.x brnr reads what an earlier 1.x
  wrote (`event log`, `session list`, `session resume`). What each record
  promises in detail is #70's.
- **Where brnr's files are**: how the state directory (`$BRNR_HOME`, else
  `~/.brnr`) and the runtime directory are found, and where each file brnr
  writes under `$BRNR_HOME` is. Where a file is is covered; what it contains
  is covered only where this list says so (transcripts), not for host logs
  or lock files.
- **Config TOML**: the file (`BRNR_CONFIG`, else its default path), its
  profiles and their sections, keys and values, and the names `experimental`
  and `features` take.
- **ACP, and what `brnr acp` passes through**: brnr speaks ACP version 1 to
  agents, and to editors through `brnr acp` (ADR 54). `brnr acp` passes what
  is between the editor and the agent as P1 says: unchanged, but for the
  changes named there (ADR 2, 3, 4), with the defaults they have at 1.0.0.

The environment variables brnr reads are inputs, as flags are, and go with
what they belong to: `BRNR_CONFIG` and `BRNR_START_TIMEOUT` with config,
`BRNR_HOME` with where brnr's files are. `BRNR_DIR` is covered as where the
runtime directory is, not as what is in it.

Stable error codes (#205) are covered once they land.

### Not covered

Whatever isn't listed above, and in particular:

- Human-readable output: tables, summaries and events as text, their
  columns and layout; `--help` and usage text; `doctor`'s check lines and
  `doctor --report`; `brnr skill`. Scripts read `--json`.
- The wording of error messages, wherever they are: on stderr, in a
  `--json` field, in a reply's `error`.
- What host logs (`$BRNR_HOME/hosts/<run id>.jsonl`) and session lock files
  contain: their records and fields. Where they are is covered as above.
- The control socket. It is in the runtime directory, which is private, and
  it is not an interface: it is how brnr's own commands and the bridges brnr
  starts reach a process. Its path (`$BRNR_DIR/<pid>.sock`), how processes
  are found, their metadata files (`<pid>.json`), and connecting to it
  directly, by `BRNR_SOCKET` (which a started bridge has) or otherwise, have
  no promise. A client connected directly speaks the bridge protocol today,
  but what it speaks, and whether it is answered, can change in any release.
- What is in the runtime directory (`BRNR_DIR`), and its layout: sockets,
  metadata files, the session lock files (`sessions/<id>.lock`, ADR 3). How
  it is found is covered, as above.
- `brnr host`, which brnr starts itself, never a person.
- The library's internals, behind the feature that is off by default
  (#147).

### Compatible and breaking changes in 1.x

- Compatible, so in a minor release: a new command, flag, JSON field,
  event, event field, bridge request, config key, or an exit code for a
  situation that had none.
- Breaking, so in a major release only: removing or renaming any of those;
  changing a field's type or what it means; changing the exit code of a
  situation that has one.
- Whoever reads what brnr writes ignores the fields and events it doesn't
  know: bridges, scripts reading `--json`, `event notify` commands, readers
  of transcripts. A bridge subscribed to every event gets the new ones
  too.
- A deprecated form keeps working until the next major version: the
  changelog says so under Deprecated, with what to use instead.

### Security fixes

Security fixes are made on the latest major version, in a release of it;
upgrading within that major is the fix, and breaks nothing covered. There
is one major so far, so no support period for older majors is set; it will
be, by a record amending this one, once a second major exists.

### When it starts

At 1.0.0. Before 1.0.0-rc.1, #150 (`--json` shapes), #151 (event and
transcript record names), #153 (the exit code of a usage error, `--help`)
and #154 (flags and config keys spelled two ways) still settle names and
shapes, breaking as P9 allows. What they settle is what 1.0.0 promises.

## Consequences

- `docs/interface.md` marks what is covered and what isn't; SECURITY.md,
  CHANGELOG.md's conventions, CONTRIBUTING.md and AGENTS.md say the 1.0
  policy next to the pre-1.0 one.
- From 1.0.0, a breaking change to what is covered waits for 2.0.0. A
  `**Breaking:**` entry in a 1.x release is wrong; a form that is replaced
  is deprecated instead, and the old form kept.
- A bridge that needs the promise is started from a profile. One that
  connects to the socket itself works as before, with no promise.
- brnr still refuses a config key it doesn't know (P7, ADR 33): a profile
  using a key added in 1.3 doesn't load in 1.2.
- Scripts match exit codes and `--json` fields, not messages. Until stable
  error codes (#205) land, there is nothing finer to match.
- Text output can change in any release, for people: P5 has it carry the
  same data as `--json`, so nothing is in it that `--json` doesn't have.

## Considered

- Leaving the bridge protocol out with the socket: bridges' requests, which
  go over the same protocol on a started bridge's stdin and stdout, would
  have no promise.
- Covering the socket too: it is in the private runtime directory, there for
  brnr's own commands and the bridges it starts, and the runtime directory's
  layout, how processes are found and their metadata would be held for 1.x.
  A started bridge has all a bridge needs.
- Covering text output: two contracts for the same data (P5), and text is
  for people, who are served by changing it.
- Covering the wording of errors: every better message would wait for a
  major. Stable error codes (#205) let the wording change freely.
- Covering what host logs contain: they are what a process records for
  itself and for `doctor`, not for others to read; the session's story is
  in its events file (ADR 22).
- A support period for older majors now: there is only one.
- Keeping the library public: every internal change would be a semver
  change (#147).
