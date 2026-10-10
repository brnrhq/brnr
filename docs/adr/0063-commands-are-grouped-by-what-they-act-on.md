# 63. Commands are grouped by what they act on, with ACP's verbs

Proposed 2026-10-09. Partly implemented: the groups, with today's commands
renamed into them (`queue list` keeps `--clear` and `--clear-context` for
now). Not yet: `session new`, `resume`, `list` and `delete`, `queue show`
and `clear`, `permission allow` and `reject`, `config`, `--pid` and
`--permission-timeout`, nor the names beyond the command line. Supersedes
15. Amends 7, 12, 14, 16, 18, 19, 21, 27, 28, 33, 35 and 58.

## Context

brnr's commands are one flat level that grew a command at a time, and their
names drift from ACP's where both name the same thing:

- `send` is `session/prompt`, while `start --prompt` and the socket's `send`
  mode `prompt` already use ACP's word.
- `sessions` is `session/list`, while `list` is brnr's own index, which
  never asks the agent (ADR 15).
- `--set` sets a config option; `model` sounds like `session/set_model`,
  which brnr never sends (ADR 28).
- `approve` and `deny` pick an option of a kind, and `deny` answers
  `cancelled` when there is no reject option, which ACP defines as the
  answer to a cancelled turn, not a refusal.
- `session/delete` (stable in v1, `sessionCapabilities.delete`) has no
  command, and a process can't open a second session other than by `fork`.

ACP's draft v2 (`unstable_protocol_v2` in `agent-client-protocol-schema`
1.10.2) moves further: no `session/set_mode` or `modes`, no
`session/set_model`, no `session/load`; modes, models and thought levels are
config options told apart by category (`mode`, `model`, `model_config`,
`thought_level`).

## Decision

### The rule

A command is `brnr <entity> <verb>`: the entity it acts on, and ACP's verb
wherever ACP has one (P1). The entities are brnr's model of a process:

```text
process
├── event
└── session          also in the agent's store, open in at most one process
    ├── prompt       a turn: session/prompt until its stop reason
    ├── queue        brnr's held messages and context for the next prompt
    ├── permission   the agent's session/request_permission requests
    └── config       the session's config options
```

A command's group is its entity, not its depth: a nested entity takes its
parent as an argument (`brnr permission allow <session> <request>`). Tools
that act on no entity stay at the top level.

```text
brnr process    list | stop <pid>
brnr session    new [--pid <pid>] [<new flags>] [-- <agent> [args...]]
                | resume [--pid <pid>] <session> [--take-over] [<new flags>] [-- <agent> [args...]]
                | list [--include active,inactive] [--profile <p>] [--cwd <dir>] [-- <agent> [args...]]
                | status <session> | fork <session> | close <session>
                | delete <session> [--purge] [--profile <p>] [-- <agent> [args...]]
brnr prompt     send <session> [--steer | --interrupt | --context [--replace]]
                     [--file <path>]... [--image <path>]... [--wait [--timeout <s>]] (<text>... | -)
                | cancel <session> [--keep-held] | commands <session>
brnr queue      list <session> | show <session> <message> | drop <session> <message>
                | clear <session> [--messages] [--context]
brnr permission requests [<session>] | show <session> <request>
                | allow <session> <request> [--always] [--option <id>]
                | reject <session> <request> [--always] [--option <id>]
brnr config     get <session> [--mode] [--model] [--thought-level] [--option <o>]...
                | set <session> [--mode <m>] [--model <m>] [--thought-level <l>] [--option <o>=<v>]...
brnr event      log <session> [--last <n>] [--follow] [--events <e>,...]
                | watch (<session> | --pid <pid>) [--events <e>,...]
                | notify (<session> | --pid <pid> | --stdin) [--events <e>,...] -- <command> [args...]
                | wait <session> [--for idle|turn|permission|exit] [--timeout <s>]
brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]
brnr doctor [--fix | --report] | skill [<reference> | install [--dir <dir>]...] | --version

<new flags>
  process: --profile <p> --auth <method> --strict --stop-when-idle <s>
           --permission-timeout <s> --foreground [--quiet]
  session: --cwd <dir> --mode <m> --model <m> --thought-level <l> --option <o>=<v>
  prompt:  --prompt <text> | -   --file <path>   --image <path>   --wait [--timeout <s>]
```

`--json` stays wherever a command prints data (ADR 34). `--` always ends
brnr's options: what follows is the agent's argument vector, or notify's
command's. `--profile` and `-- <agent>` combine as they do today.

### What each change means

- **`session new` and `resume` replace `start`.** Without `--pid` they start
  a process whose first session they open, committed with its first prompt
  as one start (ADR 7, P10); `resume` is `session/resume`, or
  `session/load` where the agent has only that (ADR 14). With `--pid` they
  open a session in a running process: the process flags are an error
  there (P7); an editor's process refuses, as it refuses `fork` (ADR 4);
  with `stop_when_idle` and an agent that can't close sessions, a second
  session is refused, as `fork` is (ADR 12). `--take-over` works with
  `--pid` as without.
- **`--permission-timeout <s>`** is the profile's headless
  `permission_timeout` as a flag, winning over it as ADR 58's flags do.
- **`session list`** joins brnr's index and the agent's, superseding ADR 15:
  - Without an agent named, it is brnr's index (today's `list --all`): every
    cwd and agent, with nothing started.
  - With `--profile` or `-- <agent>` (cwd here unless `--cwd`), brnr's rows
    are narrowed to that cwd, the agent is started and asked
    (`session/list`, every page), and the two are joined on the session id:
    exact, so brnr never guesses which recorded agent a command line is
    (P4). A session open in a process that the agent lists there is joined
    whatever cwd brnr recorded for it, its lock being one per id (ADR 53).
    An agent that can't list fails the command (P7).
  - A cwd is the same however it is spelled: `--cwd` and a row's cwd are
    compared as the directories they name (a trailing slash or a symlink
    makes no difference). An `unreachable` session's cwd is unknown (its
    process doesn't answer), so it is the cwd's if the agent lists it there
    or brnr has its transcript there; its CWD stays null unless the agent
    gives one.
  - `--include` filters by state: `active` is open in a process (`idle`,
    `busy`, `waiting`, `unreachable`), `inactive` isn't, whoever knows it;
    both by default.
  - A SOURCE column, `brnr`, `agent` or `both`, says who reported the row,
    after AGENT: SESSION, TITLE, STATE, PID, AGENT, SOURCE, LAST ACTIVE,
    CWD. A session only the agent knows is `inactive` with SOURCE `agent`,
    replacing ADR 15's `-`. TITLE and LAST ACTIVE are the agent's where it
    gives them, as in ADR 15.
- **`session delete`** is `session/delete`, refused for a session open in a
  process ("close it first") and for an agent without
  `sessionCapabilities.delete` (P7). The agent is the one brnr recorded for
  the session, or `--profile`/`-- <agent>` for one it hasn't. It deletes the
  agent's copy only, as an editor's `session/delete` through `brnr acp`
  does: both emit `session_deleted`, and the transcript stays, a `brnr` row
  in `session list`. `--purge` also deletes brnr's transcript of it (the
  events file and `<id>.acp.jsonl`, ADR 22), not the process logs, which
  are shared, and does so whatever the agent answered. With `--purge`, the
  command succeeds only if the agent deleted the session or doesn't have it
  (ACP's `resource_not_found`, -32002), which is said; any other error, or
  no answer, is said and fails the command, the transcript deleted all the
  same: the agent may still have the session (P3, P7). Only an agent that
  answers `resource_not_found` doesn't have it: the Codex adapter (codex-acp
  2.1.1) answers an unknown session as deleted, so `--purge` succeeds, but
  the Claude adapter (claude-agent-acp 0.85.1, whose SDK throws a plain
  error) answers -32603, so `--purge` of a session it already deleted fails.
  A file that can't be deleted, or have `session_deleted` recorded in it, is
  said and fails the command, the others done all the same (P3); what was
  and wasn't deleted is said as it is, not as "no transcript" or "deleted
  all the same". A symlinked project folder isn't looked in, and a
  transcript made private first (ADR 59) is said on stderr: the command has
  no host log for `made-private`. A deletion isn't activity: LAST ACTIVE
  stays when a process last served the session.
  `session_deleted` is recorded in the session's transcript: by the command,
  as a record of no process's (`host_id` null), since no process has the
  session open; by the editor's process, which closes the session if it had
  it open. An agent whose `session/delete` fails (an error, or no answer)
  hasn't deleted it, and fails the command, but for `--purge` and an agent
  that doesn't have the session.
- **`prompt send`** is `send`, flags unchanged; **`prompt cancel`** is
  `cancel`, the turn's `session/cancel`; **`prompt commands`** is `commands`.
- **`queue`** is `queue` split into verbs. `show` is new: one held message
  in full. `clear` with neither flag clears both, `--messages` or
  `--context` only that one, so each of today's `--clear` and
  `--clear-context`, alone or together, remains.
- **`permission allow` and `reject`** replace `approve` and `deny`. Each
  answers with the option of one ACP kind: `allow_once`, `allow --always`
  `allow_always`, `reject_once`, `reject --always` `reject_always`. No other
  kind stands in for it, and `reject` never answers `cancelled`: a request
  without that kind, or with two options of it, fails, listing the options
  (P4, P7). `--option <id>` names the option instead, for a kind brnr
  doesn't know (v2 allows `_…` kinds) or two options of one kind: one of an
  ACP kind must be on the verb's side (`allow_*` for `allow`), and with
  `--always` the always kind; one of a kind brnr doesn't know goes with
  either verb, as ADR 27 has it. `prompt cancel` is how a request is
  answered `cancelled`, as ACP has it. `requests` lists permission requests,
  `show` shows one (ADR 27).
- **The permission timeout** answers with the `reject_once` option; a
  request without one has its turn cancelled (`session/cancel`), so it and
  every request pending in the session are answered `cancelled` as ACP
  requires. Never `reject_always`: nobody made that choice.
- **`config set`** finds an option by category with `--mode`, `--model` and
  `--thought-level` (`mode`, `model`, `thought_level`, ADR 28) and by id
  with `--option`; the same flags as `session new`, resolved as ADR 58
  resolves a start's. A mode on an agent with v1 modes and no mode option is
  `session/set_mode`; nothing else is sent but `session/set_config_option`.
  **`config get`** shows every option with its current value and its
  choices, and the v1 modes where the agent has them: what `mode`, `model`
  and `config` without a value list today. `--mode`, `--model`,
  `--thought-level` and `--option <o>` narrow it to those options, found as
  `set` finds them (`config get <session> --mode` lists the modes).
- **`event`** is `log`, `watch`, `notify` and `wait`, flags unchanged.
- **`process list`** is `ps`, **`process stop`** is `stop`.

### Beyond the command line

The same names reach brnr's other interfaces, so one word means one thing
(P5):

- Socket and bridge commands (ADR 35): `approve`/`deny` become `allow`/
  `reject` with `always` and `option`; `set_mode`, `set_model` and `set_config` become
  one `set_config` taking `mode`, `model`, `thought_level` and `options`;
  `new` and `resume` are added for `--pid`. `send` stays: it is the
  message, whatever its mode.
- A profile's headless part (ADR 33): `mode`, `model`, `thought_level`,
  `options` (was `config`), `permission_timeout`, `stop_when_idle`.
- The editor's `experimental` actions (ADR 4): `approve` becomes
  `permission`, `settings` becomes `config`.
- Events: `permission_resolved` says `allowed` or `rejected`, and which
  kind; `session_deleted` is new.
- Errors, hints and the usage text name the new commands.

## Consequences

- **Breaking**, all of it, with no aliases (P9): every script, bridge,
  profile and editor config that names an old command, socket command or key
  changes.
- `brnr session list` with no options shows inactive sessions too, where
  `list` showed running ones only.
- An agent offering only `allow_always` can't be allowed once: `allow`
  fails and says so, where `approve` allowed always.
- `brnr session --help` and each group's lists its verbs; `brnr --help`
  lists the groups.

## Considered

- Grouping by the kind of id a command takes (session, request, message,
  pid): `watch` and `notify` take a session or a pid and fit neither, and
  mode, model and config stayed apart.
- Mirroring ACP's tree: nearly everything is `session/*`, so a `session`
  prefix on every command would say nothing.
- `process start` opening a process with no session: a process stops with
  its last session (ADR 12), and a start without its prompt is the gap ADR 7
  closed.
- Keeping `list` and `sessions` apart (ADR 15): two answers to one question.
  Asking every agent in every cwd is still not done; the agent is asked only
  when named.
- `permission respond <option>` with ACP's option ids alone: ids are the
  agent's, so a script works for one agent; kinds are ACP's. `--option`
  keeps ids for what kinds can't say.
- `permission dismiss`: ACP has no outcome for it but `cancelled`, which is
  a cancelled turn's.
- `--config` for `--option`: it reads as a config file's path.
- `permission list`: what is listed are requests, not permissions.

## Tests

Run `cargo test --release adr_0063_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0063_old_commands_are_unknown`: each command moved into a group
    fails under its old name as an unknown command, and does nothing.
  - `adr_0063_help_lists_the_groups_and_their_commands`: `brnr --help` has a
    section per group; `brnr <group> --help`, and a group without a verb,
    print the group's commands; an unknown verb is said, with the group's
    usage; a command used wrongly shows its own; a verb's `--help` or `-h`,
    before any `--`, prints its usage, and after `--` is the command's.
  - `adr_0063_config_get_lists_options_choices_and_modes`: every option with
    its category, value and choices, each choice with its name and
    description, and the v1 modes as a row with no option, the same in text
    and JSON.
  - `adr_0063_config_get_narrows_by_category_and_id`: `--mode`, `--model`,
    `--thought-level` and `--option` narrow the list, in its order; it
    lists only options the agent advertised, so one it doesn't have fails.
  - `adr_0063_config_set_by_category_and_by_id`: each setting is sent once,
    the mode first; two values for one setting, or a mode, model or thought
    level the agent has no option for, fail before anything is sent; an
    option by id the agent hasn't advertised is sent (ADR 28), and when the
    agent refuses it the error says what was set before it.
  - `adr_0063_new_and_resume_replace_start`: `session new` opens a session
    and `session resume` resumes it (`session/resume`) with the recorded
    agent; `--set`, `--resume` and `--take-over` on `new` fail; each
    says its usage, with the flags they share.
  - `adr_0063_thought_level_is_the_option_of_its_category`: `--thought-level`
    and the profile's `thought_level` set the option of that category,
    after the model and before the prompt; the flag wins over the profile;
    two values for it from one source, or an agent without one, fail the
    start before anything is set.
  - `adr_0063_permission_timeout_flag_wins_over_the_profile`: a shorter
    `--permission-timeout` rejects before the profile's would, and a longer
    one keeps the profile's from answering.
  - `adr_0063_allow_and_reject_pick_by_kind`: each verb, with and without
    `--always`, answers with the option of its kind, whatever their order,
    and `permission_resolved` says `allowed` or `rejected` and the kind.
  - `adr_0063_a_missing_or_doubled_kind_fails`: no option of the kind, or
    two, fails listing the options, and nothing is answered; `reject` never
    answers `cancelled`.
  - `adr_0063_option_must_be_on_the_verbs_side`: `--option` of the other
    side, or of the once kind with `--always`, fails; one of a kind brnr
    doesn't know goes with either verb.
  - `adr_0063_queue_show`: `queue show` prints one held message in full, its
    text, whether it interrupts, and its attachments, in text and `--json`
    (the content blocks), and drops nothing; a message not held fails; the
    socket's `show` goes alone, and one that isn't a message id is refused.
  - `adr_0063_queue_clear_flags`: `queue clear --messages` drops the held
    messages only, `--context` the held context only, and both flags or
    neither drop both, each with its event; `queue list` takes neither
    `--clear` nor `--clear-context`.
  - `adr_0063_list_without_an_agent_is_brnrs_index`: `session list` with no
    agent named lists the sessions open in brnr's processes and those it has
    transcripts of, in every cwd, source `brnr`, with the columns in order,
    and starts no agent, not even the default profile's; `--cwd` narrows it.
  - `adr_0063_list_joins_the_agents_sessions_on_id`: with `-- <agent>` or
    `--profile`, the agent is asked for every page; brnr's sessions in the
    cwd are joined with the agent's on the id (`both`), one only the agent
    knows is `inactive` with source `agent`, the agent's title and time win
    where it gives them, over brnr's title too, and another cwd's are left
    out; an agent that can't list fails.
  - `adr_0063_list_finds_a_cwd_however_it_is_spelled`: a session opened
    with `--cwd <dir>/` or a symlink to `<dir>` is `<dir>`'s, with an agent
    named or not, and the open one the agent lists is `both` and `active`,
    not one only the agent knows.
  - `adr_0063_list_include_filters_by_state`: `--include active` keeps the
    sessions open in a process, `inactive` the others, whoever knows them;
    both by default; an unknown state fails.
  - `adr_0063_list_stops_an_agent_that_wont_go` (ADR 15's): the agent asked,
    ignoring SIGTERM, is killed with what it started.
  - `adr_0063_delete_keeps_the_transcript`: `session delete` asks the
    recorded agent, which deletes its copy; the transcript stays, listed,
    with `session_deleted` in it, its LAST ACTIVE unchanged, and the agent
    is still found for it.
  - `adr_0063_delete_a_session_only_the_agent_knows`: one brnr has no
    transcript of needs its agent named, and records nothing.
  - `adr_0063_delete_purge`: `--purge` deletes the session's two files, not
    another session's nor the host logs, and does so for an agent that
    doesn't have the session (-32002), saying so, and succeeding.
  - `adr_0063_delete_purge_fails_when_the_agent_does`: for any other error,
    the transcript is deleted too, the error said, and the command fails.
  - `adr_0063_delete_purge_says_what_it_couldnt_delete`: a transcript in a
    folder brnr can't write to isn't deleted, which is said, not "no
    transcript" nor "all the same", and the command fails, whatever the
    agent answered.
  - `adr_0063_delete_records_in_each_transcript_it_can`: `session_deleted`
    goes into each transcript that can take it, one refused is said and
    fails the command, a symlinked folder isn't looked in, and a transcript
    made private first is said.
  - `adr_0063_delete_refuses_an_open_session`: refused, the agent not asked,
    until the session is closed.
  - `adr_0063_delete_needs_an_agent_that_can_delete`: an agent without
    `sessionCapabilities.delete` isn't asked, and nothing is deleted.
- [tests/headless.rs](../../tests/headless.rs)
  - `adr_0063_an_editors_delete_is_recorded`: the editor's `session/delete`
    reaches the agent; once answered, `session_deleted` by the editor is in
    the transcript, and an open session closes.
- [tests/security.rs](../../tests/security.rs)
  - `adr_0063_timeout_rejects_once_else_cancels_the_turn`: the timeout
    answers `reject_once`; without it, `session/cancel` goes first and the
    request is answered `cancelled`, and the turn ends.
  - `adr_0063_purge_deletes_only_that_sessions_files`: nothing a symlink
    points at, nor an id that climbs names, is deleted.
- [src/render.rs](../../src/render.rs)
  - `adr_0063_a_deletion_is_shown_with_who_deleted`.

Not implemented yet, each to come with `adr_0063_` tests naming its claims:
`session new --pid` and its refusals.
