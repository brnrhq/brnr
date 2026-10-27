# Reference

This part of the book is brnr's interface: its commands and their flags,
what each prints with `--json`, its exit codes and error codes, its events,
the bridge protocol, the config file and its files. It applies to the source
revision that contains it: read the one of the tag of your `brnr --version`.
For installing and a walkthrough, start with the
[introduction](../introduction.md) and the guide.

The command line is the interface. The `brnr` crate's Rust library is not a
second one: it is the command line as functions
([ADR 8.5](../adr/0008-the-command-line-and-compatibility.md#85-the-library-is-private-and-its-facade-mirrors-the-command-line)).
`brnr::run(args)` is `brnr <args>`, run to the exit status the command would
exit with. The typed facade has a module for each command group and a
function for each verb, `brnr::session::list`, `brnr::prompt::send`, which
return what the command prints with `--json`, as these pages describe it,
with the same names; an error is `brnr::Error`, with its [code](errors.md).
A function runs its command in the calling process, which says on stderr
what the command says there.

```rust
let mut flags = brnr::session::ListFlags::default();
flags.include = vec!["active".into()];
for row in brnr::session::list(&flags)?.sessions {
    println!("{} {}", row.session, row.state);
}
```

- A command that waits, and `event watch`, `session delete`, `session
  prune` and `doctor`,
  return `brnr::Exit`: its exit status, with what it printed. `event log`
  and `event watch` give each event to a closure.
- `session new` and `session resume` start the process from the running
  program, as `<program> host`: a program that calls them hands its
  arguments to `brnr::run` when the first is `host`, first thing in its
  `main`.
- Not in the facade: `--foreground`, `event notify` (`event::watch` does
  what it does), `doctor --report`, `--help`, `--version`, SSH routing and `remote check`.
  `brnr::run` accepts remote options, but replaces the calling process with
  SSH on a successful dispatch; it returns only if routing or execution fails.

The `internals` feature, which the crate's own tests and fuzz targets use,
is not covered by any compatibility promise.

## How to read it

There is a page for each command group, one section for each command. A
section has the command's syntax, its flags, what it prints with `--json`
(a table with one row for each field, and an example), and its exit codes
and error codes:

- **Field**: the name in the JSON. A dotted name (`last_turn.stop_reason`) is
  a field of an object in a field. `[]` after a name is each item of a list.
- **Type**: `string`, `number` (an integer, but where the row says
  otherwise), `boolean`, `object`, `list of …`. **or null** says that the
  field can be null, and the row or the text under the table says when. A
  field without "or null" is never null. **optional** says that the field
  is not always there, and the row says when. A field without "optional" is
  always there. **ACP's** says that the value is ACP's, as the agent or the
  editor sent it: its fields are ACP's, and have no rows here.
- **Example**: what brnr printed for the command, against the test agent,
  `tests/fake_agent.py`. Ids, times and paths are those of that run. A test
  runs each example's command and checks that what brnr prints has the same
  shape: the same fields at every level (but an optional one), each of the
  same JSON type and of a type its row names, in every item of a list. It
  checks that every field of the example, at every level, has a row
  ([tests/docs.rs](https://github.com/brnrhq/brnr/blob/main/tests/docs.rs),
  `adr_0008_every_json_example_has_the_shape_brnr_prints`).
- **Text output** is one sentence: it shows what `--json` has, for people,
  and is not covered.

Other pages: [events](events.md), the [bridge protocol](bridge.md),
[exit codes](exit-codes.md), [errors](errors.md), the
[configuration](configuration.md) and [files](files.md), which has the
compatibility rules.

## Remote execution

All execution commands, including `acp`, `doctor`, `skill` and `--version`,
accept `--remote <[user@]host>` before `--`. SSH resolves aliases, usernames,
ports and keys. An explicit target bypasses local brnr config; `--profile`
then names an execution profile on the destination. Otherwise `--profile`
can select a local [routing profile](configuration.md#profilesnameremote).
Only `acp`, `session new` and `session resume` consult a default routing
profile implicitly. Routing happens once; a second hop is refused.

Arguments, stdin, stdout, stderr and ordinary exit statuses pass through
system SSH, without a terminal. Paths, attachments, skill installation and
notification commands refer to the destination. Authentication and host
trust must already work; SSH runs with `BatchMode=yes` and
`StrictHostKeyChecking=yes`. Both brnr versions must match exactly.
Transport failures exit 255 and may have no brnr JSON. Help stays local.
See [remote hosts](../guide/remote.md) for setup and lifecycle limits, and
[`remote check`](remote.md) for installation checks.

## Covered and not covered

Before 1.0.0 every interface here can change outright, with no aliases or
shims ([ADR 1, P9](../adr/0001-principles.md#p9-no-backwards-compatibility-before-10)).
From 1.0.0, what these pages mark **Covered** is kept compatible within a
major version, and what they mark **Not covered** can change in any release
([ADR 8.4](../adr/0008-the-command-line-and-compatibility.md#84-what-the-compatibility-promise-covers)).
[Compatibility](files.md#compatibility) has the rules: what a minor release
can add, and what only a major release can change.

**Covered** from 1.0.0:

- the commands, their flags and positional arguments, and what each means;
- each command's `--json` structure, its fields, their types and what they
  mean;
- the exit code of each situation [exit codes](exit-codes.md) lists, and
  each [error code](errors.md) and what it means;
- each [event](events.md)'s name and fields, wherever it goes;
- the [bridge protocol](bridge.md), as a bridge that brnr starts speaks it;
- the [config TOML and the environment variables](configuration.md);
- [transcripts and where brnr's files are](files.md);
- ACP, and what [`brnr acp`](acp.md) passes through.

**Not covered**: the usage text, as `--help` prints it; `brnr host`; the
human-readable tables and summaries; the wording of diagnostics and errors,
on stderr or in a JSON field (`message`); the control socket; what host logs
and lock files contain; what is in the runtime directory.

## Conventions

A concept has one name and one shape in every command's JSON
([ADR 8.2](../adr/0008-the-command-line-and-compatibility.md#82-same-data-in-text-and-json)):

- **A list is an object**, the list under the plural of what it lists
  (`{"sessions": [...]}`), never a bare array.
- **A row is its detail's object with fewer fields.** `session list` rows
  have `session status`'s fields, `permission requests` rows have
  `permission show`'s, and `elicitation requests` rows have `elicitation
  show`'s.
- **A name is a count or a list, never both.** A count is `<what>_count`.
- **`dropped`** is a list of the messages dropped, each `{message, text,
  reason}`, as its [`message_dropped`](events.md#message_dropped) event has
  them.
- **`agent`** is `{program, name, version}`. `program` is the command brnr
  ran (its file name). `name` and `version` are what the agent says it is in
  `initialize` (ACP's `agentInfo`). Each is null where brnr doesn't know it.
- **`owner`** is `editor` or `headless`, null where it isn't known. A
  process that doesn't answer is `state: "unreachable"`.
- **ACP's objects keep ACP's names**: a permission request's `options` are
  `{optionId, name, kind}`.
- **Times** are RFC 3339, in UTC, to the microsecond
  (`2026-10-11T06:00:00.000000Z`), but where a row says otherwise.
- **Durations** are whole seconds.

## Output

A command writes its result to stdout and its diagnostics to stderr. With
`--json` it prints structured data in place of the human-readable table or
summary: one JSON value, or one event per line for `event log`, `event watch`
and `session new --foreground`. Parse the JSON, not the human-readable
columns. Agent-supplied objects vary with the agent's capabilities.

A command that fails says so on stderr, `brnr: <message> [<code>]`, and,
with `--json`, prints its error as its JSON, `{"error": {"code",
"message"}}` ([errors](errors.md)). A usage error is not made JSON, nor are
other diagnostics.

<!-- example: brnr session status sess-9 --json; before: Idle; exit: 1 -->
```json
{
  "error": {
    "code": "session_not_found",
    "message": "no session sess-9 (see brnr session list)"
  }
}
```

| Field | Type | Meaning |
|---|---|---|
| `error` | object | The failure. |
| `error.code` | string | The [error code](errors.md). |
| `error.message` | string | What failed, for people. Not covered. |

## Command syntax

`<name>` is a required value, `[...]` is optional, `...` means repeatable,
and `|` separates alternatives. Do not type the brackets.

- A `<session>` is a session's id: an exact, opaque string from the agent,
  not a prefix or a process id.
- A `<request>` is a handle: `p1` for an approval, from `permission
  requests`; `e1` for an elicitation, from `elicitation requests`.
- A `<message>` is a message id such as `m1`, from `prompt send`.
- A `<pid>` is a brnr process, which can own more than one session.

Most commands are `brnr <group> <verb>`, grouped by what they act on:
`process`, `session`, `prompt`, `queue`, `permission`, `elicitation`,
`config` and `event`
([ADR 8.1](../adr/0008-the-command-line-and-compatibility.md#81-commands-are-grouped-by-what-they-act-on-with-acps-verbs)).

- `brnr <group> --help` prints one group's syntax. A group without a verb is
  a usage error.
- `brnr <group> <verb> --help` prints that command's syntax. So does `-h`,
  anywhere before a `--`, and not as a flag's value.
- Help goes to stdout and exits 0 in every command: `brnr acp`, `doctor` and
  `skill` too.
- A usage error does nothing. It says what was wrong and the command's usage
  on stderr, and exits 2
  ([ADR 8.3](../adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status)).
  It is an unknown command, verb or option, an argument missing or one too
  many, a value a flag can't take, or flags that don't go together.
- An option a command doesn't take fails as unknown. It is never read as the
  command's `<session>` or `<pid>`. A word with one dash, such as `-x`, is an
  argument (but for `brnr skill`).
- A command brnr doesn't have, such as the names these replaced
  (`brnr send`), fails as unknown.
- `--` separates brnr's options from the agent or notification command and
  its arguments. Programs are run as argument vectors, not shell strings:
  run a shell yourself if you need shell syntax.
- With no agent after `--`, the selected profile gives it. `session list`
  asks an agent only when `--profile` or `-- <agent>` names one.
  `--profile` defaults to `default` when that profile exists.
- `brnr --help` prints the syntax below, a section for each group. `-h` is
  an alias, and `-V` is an alias of `--version`, which prints `brnr
  <version>`.
- `brnr host` is internal: brnr starts it, not a person.

**Covered** from 1.0.0: the commands, their flags and positional arguments,
and what each means. **Not covered**: the usage text itself, as `--help`
prints it, and `brnr host`.

```text
usage:
  brnr acp [--profile <p>] [--strict] [--remote <target>] [-- <agent> [args...]]
             what an editor runs as its ACP agent

process
  brnr process list [--json]
  brnr process stop <pid> [--json]

session
  brnr session new [--pid <pid>] [<new flags>] [-- <agent> [args...]]
                   a process for a headless session, in the background (or the foreground);
                   --pid: a session in that running process, without the process flags
  brnr session resume [--pid <pid>] <session> [--take-over] [<new flags>] [-- <agent> [args...]]
                   the same, carrying on a session
  brnr session status <session> [--json]
  brnr session fork <session> [--json]
  brnr session close <session> [--json]
  brnr session delete <session> [--purge] [--profile <p>] [--json] [-- <agent> [args...]]
  brnr session list [--include active,inactive] [--profile <p>] [--cwd <dir>] [--json]
                    [-- <agent> [args...]]
  brnr session prune --older-than <s> [--dry-run] [--json]
                     delete brnr's transcripts of sessions, and the host logs of processes,
                     last active more than <s> seconds ago

<new flags>
  process: [--profile <p>] [--auth <method>] [--strict] [--stop-when-idle <s>]
           [--response-timeout <s>] [--foreground [--quiet]]
  session: [--cwd <dir>] [--mode <m>] [--model <m>] [--thought-level <l>]
           [--option <o>=<v>]...
  prompt:  [--prompt <text> | -] [--file <path>]... [--image <path>]...
           [--wait [--timeout <s>]] [--json]

prompt
  brnr prompt send <session> [--steer | --interrupt | --context [--replace]]
                   [--prompt <text> | -] [--file <path>]... [--image <path>]...
                   [--wait [--timeout <s>]] [--json]
  brnr prompt cancel <session> [--keep-held] [--json]
  brnr prompt commands <session> [--json]

queue
  brnr queue list <session> [--json]
  brnr queue show <session> <message> [--json]
  brnr queue drop <session> <message> [--json]
  brnr queue clear <session> [--messages] [--context] [--json]

permission
  brnr permission requests [<session>] [--json]
  brnr permission show <session> <request> [--json]
  brnr permission allow <session> <request> [--always] [--option-id <id>] [--json]
  brnr permission reject <session> <request> [--always] [--option-id <id>] [--json]

elicitation
  brnr elicitation requests [<session>] [--json]
  brnr elicitation show (<session> | --pid <pid>) <request> [--json]
  brnr elicitation accept (<session> | --pid <pid>) <request> [--field <k>=<v>]... [-] [--json]
  brnr elicitation decline (<session> | --pid <pid>) <request> [--json]
  brnr elicitation cancel (<session> | --pid <pid>) <request> [--json]

config
  brnr config get <session> [--mode] [--model] [--thought-level] [--option <o>]... [--json]
  brnr config set <session> [--mode <m>] [--model <m>] [--thought-level <l>]
                  [--option <o>=<v>]... [--json]

event
  brnr event log <session> [--last <n>] [--follow] [--events <default|all|event>,...] [--json]
  brnr event watch (<session> | --pid <pid>) [--events <default|all|event>,...] [--json]
  brnr event notify (<session> | --pid <pid> | --stdin) [--events <default|all|event>,...]
                    -- <command> [args...]
  brnr event wait <session> [--for idle|turn|permission|elicitation|exit] [--timeout <s>]
                  [--json]

remote
  brnr remote check (<target> | --profile <p>) [--json]

brnr
  brnr doctor [--fix | --report] [--json]
  brnr skill [<reference> | install [--dir <dir>]...]
             the skill for agents that use brnr: print it, or install it
  brnr --version

brnr <group> --help lists a group's commands. <session> is a session's id, as brnr session list
shows it. <request> is a pending approval's or elicitation's handle, as brnr permission requests
and brnr elicitation requests show them. <pid> is a brnr process, which runs one agent for one or
more sessions, as brnr process list shows them. --json prints the same data as the text: one JSON value, or one event per line for event
log, event watch and session new --foreground. --strict is stable ACP only: no --steer into a
running turn, no session fork.
Execution commands accept --remote <[user@]host> before --: execute over SSH.
--profile <p> can select a local routing profile. SSH uses existing host trust and keys.
Remote paths and event notify commands are on the destination. See brnr remote --help.
```
