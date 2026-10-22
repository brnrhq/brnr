# 8. The command line and compatibility

Draft 2026-10-20. Implemented, but 8.4 is a promise that applies from
1.0.0, 8.5's typed facade (#215) isn't built, and 8.6's evals with a real
agent aren't done. Factor: F5 (ADR 1). Brings together former ADRs 63, 15, 34,
21, 64, 66 and 46 (docs/adr/README.md maps them).

brnr is a Unix command-line utility, and its command line is its
interface (F5). This topic covers how commands are named and grouped, what
they print in text and JSON, how waiting commands exit, what stays
compatible from 1.0.0, why the Rust library is no second interface, and the
skill that teaches agents to use brnr.

## 8.1 Commands are grouped by what they act on, with ACP's verbs

A command is `brnr <entity> <verb>`: the entity it acts on, and ACP's verb
wherever ACP has one (P1). The entities are brnr's model of a process:

```text
process
├── event
└── session          also in the agent's store, open in at most one process
    ├── prompt       a turn: session/prompt until its stop reason
    ├── queue        brnr's held messages and context for the next prompt
    ├── permission   the agent's session/request_permission requests
    ├── elicitation  the agent's elicitation/create requests (ADR 4.5)
    └── config       the session's config options
```

A command's group is its entity, not its depth: a nested entity takes its
parent as an argument (`brnr permission allow <session> <request>`). Tools
that act on no entity stay at the top level. The commands:

```text
brnr process     list | stop <pid>
brnr session     new [--pid <pid>] [<new flags>] [-- <agent> [args...]]
                 | resume [--pid <pid>] <session> [--take-over] [<new flags>]
                   [-- <agent> [args...]]
                 | list [--include active,inactive] [--profile <p>] [--cwd <dir>]
                   [-- <agent> [args...]]
                 | status <session> | fork <session> | close <session>
                 | delete <session> [--purge] [--profile <p>] [-- <agent> [args...]]
brnr prompt      send <session> [--steer | --interrupt | --context [--replace]]
                   [--prompt <text> | -] [--file <path>]... [--image <path>]...
                   [--wait [--timeout <s>]]
                 | cancel <session> [--keep-held] | commands <session>
brnr queue       list <session> | show <session> <message>
                 | drop <session> <message> | clear <session> [--messages] [--context]
brnr permission  requests [<session>] | show <session> <request>
                 | allow <session> <request> [--always] [--option-id <id>]
                 | reject <session> <request> [--always] [--option-id <id>]
brnr elicitation requests [<session>] | show <s> <request>
                 | accept <s> <request> [--field <k>=<v>]... [-]
                 | decline <s> <request> | cancel <s> <request>
                   (<s> is <session> | --pid <pid>)
brnr config      get <session> [--mode] [--model] [--thought-level] [--option <o>]...
                 | set <session> [--mode <m>] [--model <m>] [--thought-level <l>]
                   [--option <o>=<v>]...
brnr event       log <session> [--last <n>] [--follow] [--events <e>,...]
                 | watch (<session> | --pid <pid>) [--events <e>,...]
                 | notify (<session> | --pid <pid> | --stdin) [--events <e>,...]
                   -- <command> [args...]
                 | wait <session> [--for idle|turn|permission|elicitation|exit]
                   [--timeout <s>]
brnr acp [--profile <p>] [--strict] [-- <agent> [args...]]
brnr doctor [--fix | --report] | skill [<reference> | install [--dir <dir>]...]
     | --version

<new flags>
  process: --profile <p> --auth <method> --strict --stop-when-idle <s>
           --response-timeout <s> --foreground [--quiet]
  session: --cwd <dir> --mode <m> --model <m> --thought-level <l>
           --option <o>=<v>
  prompt:  --prompt <text> | -   --file <path>   --image <path>
           --wait [--timeout <s>]
```

How words are read:

- `--json` goes wherever a command prints data (ADR 8.2). `--` always ends
  brnr's options: what follows is the agent's argument vector, or
  `event notify`'s command's. `--profile` and `-- <agent>` combine the
  same way wherever a command takes both.
- `brnr --help` has a section per group. `brnr <group> --help`, or a group
  without a verb, prints the group's commands. An unknown verb is said,
  with the group's usage; a command used wrongly shows its own usage. A
  verb's `--help` or `-h`, before any `--`, prints its usage; after `--` it
  is the command's, and as a flag's value it is that value (`--prompt -h`
  sends `-h`). Help goes to stdout and exits 0, and a usage error goes to
  stderr and exits 2, in every command (ADR 8.3).
- A prompt is `--prompt`'s value, sent as given (`--prompt --wait` sends
  `--wait`), and `--prompt -` reads it from stdin, for `prompt send` as for
  a start. A word that is neither a flag, a flag's value nor a positional
  the command takes, such as a prompt given as words, is a usage error.
- An option a command doesn't take fails as unknown, and nothing is done.
  It is never read as a `<session>` or `<pid>`: `session status --json`
  without a session is a usage error. Every command says an unknown option
  (`session list`, `process list`, `doctor` and `skill` too), before its
  usage. A word with one dash, such as `-x`, is an argument
  (`event log` has no `-f`).
- Names that this tree replaced have no alias (P9), and the CHANGELOG lists
  them: such a command fails as unknown and does nothing; so do the
  socket's `approve`, `deny`, `set_mode` and `set_model`. A profile with a
  headless `config` key, or the experimental actions `approve` or
  `settings`, fails to load.

`session new` and `session resume`:

- Without `--pid`, each starts a process whose first session it opens,
  committed with its first prompt as one start (ADR 3.4, P10). `resume` is
  `session/resume`, or `session/load` where the agent has only that
  (ADR 3.11). `--take-over` is `resume`'s only; each command says its usage,
  with the flags they share.
- With `--pid`, each opens a session in that running process, with its
  agent, profile and MCP servers, in `--cwd` (a resumed session's recorded
  cwd, else here), in strict mode too; its `--json` is as without `--pid`.
  The process flags (`--profile`, `--auth`, `--strict`, `--stop-when-idle`,
  `--response-timeout`, `--foreground`, `--quiet`) and `-- <agent>` are an
  error there, and nothing is sent (P7). An editor's process refuses, as it
  refuses `session fork` (ADR 7.1). With `stop_when_idle` and an agent that
  can't close sessions, a second session is refused, as `session fork` is
  (ADR 3.8).
- `resume --pid` of a session open in that process is refused, with
  `--take-over` or not. One that another process holds is refused; with
  `--take-over` it is closed there and resumed here. `--take-over` is
  refused before the session is closed where it runs if the process would
  refuse it (starting, stopping, or an agent with neither `session/resume`
  nor `session/load`), rather than leave it open in neither.
- The session, its settings (over the profile's) and its prompt commit as
  one, as a start's do (ADR 3.4). The start timeout goes to the process,
  which abandons the opening once it passes, as it does when a setting
  fails, the process is stopping or the command has gone; the command waits
  10 s longer. An abandoned opening closes the session again, the command
  says so and what was set, and no prompt is sent. An agent that can't
  close sessions keeps it, and the error names it. A new session that
  can't be locked is closed again; a resume of one isn't sent (ADR 3.9).
- A session being opened counts as one of the process's: its others closing
  or going idle meanwhile don't stop the process (ADR 3.8).
- `--response-timeout <s>` is the profile's headless `response_timeout` as
  a flag, and wins over it as a start's flags do (ADR 6.1).
- `--thought-level` and the profile's `thought_level` set the option of
  category `thought_level`, after the model and before the prompt; the
  flag wins over the profile. Two values for it from one source, or an
  agent without such an option, fail the start before anything is set.

`session list` joins brnr's index and the agent's list:

- Without an agent named, it is brnr's index: the sessions open in brnr's
  processes and those it has transcripts of, in every cwd, for every agent,
  with nothing started, not even the default profile's agent. `--cwd`
  narrows it.
- With `--profile` or `-- <agent>` (the cwd here unless `--cwd`), brnr's
  rows are narrowed to that cwd. The agent is started just to ask it
  (`initialize`, `session/list` for every page, then it exits; no brnr
  process), and an agent that won't go is killed with what it started. The
  two lists are joined on the session id, exactly, so brnr never guesses
  which recorded agent a command line is (P4). A session open in a process
  that the agent lists there is joined whatever cwd brnr recorded for it,
  its lock being one per id (ADR 9.2). An agent that can't list fails the
  command (P7).
- A cwd is the same however it is spelled: `--cwd` and a row's cwd are
  compared as the directories they name (a trailing slash or a symlink
  makes no difference). An `unreachable` session's cwd is unknown, so it is
  the cwd's if the agent lists it there or brnr has its transcript there;
  its CWD stays null unless the agent gives one.
- `--include` filters by state: `active` is open in a process (`idle`,
  `busy`, `waiting`, `unreachable`), `inactive` isn't, whoever knows it;
  both by default. An unknown state fails.
- Every table of sessions has the same columns, in the same order,
  formatted the same way: SESSION, TITLE, STATE, PID, AGENT, SOURCE,
  LAST ACTIVE, FORKED FROM, CWD (`session`, `title`, `state`, `pid`,
  `agent`, `source`, `last_active`, `forked_from`, `cwd` in `--json`, each
  row in `{sessions}`, AGENT the `agent`'s `program`, ADR 8.2).
  Rows are ordered most recently active first.
- STATE is a running session's live state (`idle`, `busy`, `waiting` for an
  approval or an elicitation), `unreachable` for one whose process holds
  its lock but doesn't answer (ADR 3.9), with the holder's pid, and
  `inactive` for the rest. A running session's STATE and PID come from
  brnr's index, with an agent named too.
- SOURCE, `brnr`, `agent` or `both`, says who reported the row. A session
  only the agent knows is `inactive`, SOURCE `agent`, with no PID.
- TITLE and LAST ACTIVE are the agent's where it gives them, over brnr's;
  LAST ACTIVE is brnr's own where the agent gives none. FORKED FROM is the
  session a fork came from, or null (ADR 3.12).
- LAST ACTIVE is in UTC, in `--json` and text alike: an agent's `updatedAt`
  is read with its offset and written as brnr writes its own times
  (`2026-10-11T08:00:00+02:00` is `2026-10-11T06:00:00.000000Z`), and rows
  are ordered by the time each names, not by how it is written (#152). One
  that isn't an RFC 3339 time is passed on as the agent gave it and said on
  stderr (P4), after the rows with a time and before those with none.
- `process list` has no CWD column: sessions have cwds. The process's cwd is
  in its JSON, often `/` for an editor's.

`session delete` is `session/delete`:

- It is refused for a session open in a process: `sess-1 is running in
  process 4466: close it first (brnr session close sess-1)`. A session a
  process holds otherwise is refused in the words `session resume` uses
  (ADR 3.9), with nothing to do about it offered: a holder that doesn't
  answer (`sess-1 is running in process 4466, which is not answering`), is
  opening it (`sess-1 is opening in process 4466`), is closing it (free once
  its records are written, ADR 5.5), or isn't listed here (`…, which brnr
  doesn't list`, or the runtime directory and the `BRNR_DIR=…` that reaches
  it). A process that takes the session's lock after the command found it
  free is said the same way. It is refused for an agent without
  `sessionCapabilities.delete` too, which isn't asked (P7). The
  agent is the one brnr recorded for the session, or `--profile` or
  `-- <agent>` for one it hasn't; a session brnr has no transcript of needs
  its agent named, and nothing is recorded for it.
- It deletes the agent's copy only, as an editor's `session/delete` through
  `brnr acp` does. Both emit `session_deleted`, and the transcript stays, a
  `brnr` row in `session list`, its agent still found for it. A deletion
  isn't activity: LAST ACTIVE stays when a process last served the session.
- `session_deleted` is recorded in the session's transcript: by the
  command, as a record of no process's (`host_id` null), since no process
  has the session open; by the editor's process, which closes the session
  if it had it open.
- An agent whose `session/delete` fails (an error, or no answer) hasn't
  deleted the session, and the command fails, but for `--purge` and an
  agent that doesn't have the session (below).
- `--purge` also deletes brnr's transcript of the session (the events file
  and `<id>.acp.jsonl`, ADR 5.1) in every project folder, an older brnr's
  too, and the older brnr's files set aside beside them
  (`<id>.older.jsonl`, `<id>.older.acp.jsonl`, …, ADR 5.9): every file of
  the session's id brnr finds, each said as it is deleted (N1, P3). Not
  the process logs, which are shared, whatever the agent answered. Nothing a symlink points at, nor an id that
  climbs out of its folder, is deleted. With `--purge` the command succeeds
  only if the agent deleted the session or doesn't have it (ACP's
  `resource_not_found`, -32002), which is said; any other error, or no
  answer, is said and fails the command, the transcript deleted all the
  same: the agent may still have the session (P3, P7).
- A file that can't be deleted, or have `session_deleted` recorded in it, is
  said and fails the command, the others done all the same (P3). What was
  and wasn't deleted is said as it is, not as "no transcript" or "deleted
  all the same". A symlinked project folder isn't looked in. A transcript
  made private first (ADR 9.3) is said on stderr: the command has no host
  log for `path_made_private`.

`prompt`, `queue` and `process`:

- `prompt send` sends a message (ADR 4.2); `prompt cancel` is the turn's
  `session/cancel` (ADR 4.3), and how a permission request is answered
  `cancelled`, as ACP has it; `prompt commands` lists the agent's slash
  commands.
- `queue show` prints one held message in full: its text, whether it
  interrupts, and its attachments, in text and `--json` (the content
  blocks). It drops nothing; a message not held fails. The socket's `show`
  goes alone, and one that isn't a message id is refused.
- `queue clear` with neither flag drops the held messages and the held
  context; `--messages` or `--context` drops only that one. Each drop is an
  event.
- `process list` lists processes; `process stop` stops one, with all its
  sessions.

`permission`:

- `requests` lists permission requests, `show` shows one (ADR 4.5).
- `allow` and `reject` each answer with the option of one ACP kind:
  `allow` `allow_once`, `allow --always` `allow_always`, `reject`
  `reject_once`, `reject --always` `reject_always`, whatever the options'
  order. No other kind stands in for it, and `reject` never answers
  `cancelled`: a request without that kind, or with two options of it,
  fails, listing the options, and nothing is answered (P4, P7).
- `--option-id <id>` names the option instead, for a kind brnr doesn't know
  (v2 allows `_…` kinds) or two options of one kind. One of an ACP kind
  must be on the verb's side (`allow_*` for `allow`), and with `--always`
  the always kind; one of a kind brnr doesn't know goes with either verb
  (ADR 4.5).
- `permission_resolved` says `allowed` or `rejected`, and which kind.
- The response timeout answers a permission request with its `reject_once`
  option. A request without one has its turn cancelled (`session/cancel`)
  first, so it and every request pending in the session are answered
  `cancelled`, as ACP requires, and the turn ends. Never `reject_always`:
  nobody made that choice.

`config`:

- `config set` finds an option by category with `--mode`, `--model` and
  `--thought-level` (`mode`, `model`, `thought_level`, ADR 6.2), and by id
  with `--option`; the same flags as `session new`, resolved as a start's
  are (ADR 6.1). Two values for one setting, or a mode, model or thought
  level the agent has no option for, fail before anything is sent. Each
  setting is sent once, the mode first. A mode on an agent with v1 modes
  and no mode option is `session/set_mode`; nothing else is sent but
  `session/set_config_option`. An option by id the agent hasn't advertised
  is sent (ADR 6.2); when the agent refuses it, the error says what was set
  before it.
- `config get` shows every option with its category, current value and
  choices (each with its name and description), and the v1 modes, where the
  agent has them, as a row with no option, the same in text and JSON.
  `--mode`, `--model`, `--thought-level` and `--option <o>` narrow it to
  those options, in its order, found as `set` finds them
  (`config get <session> --mode` lists the modes). It lists only options
  the agent advertised, so one the agent doesn't have fails.

`event` is `log`, `watch`, `notify` and `wait` (ADR 5.3 to 5.8, ADR 8.3).

The same names reach brnr's other interfaces, so one word means one thing
(P5):

- Socket and bridge commands (ADR 5.7): `allow` and `reject`, with `always`
  and `option_id`; one `set_config` taking `mode`, `model`, `thought_level`
  and `options`; `new` and `resume` for `--pid`, with the command's
  `timeout`. `send` is the message, whatever its mode.
- A profile's headless part (ADR 6.1): `mode`, `model`, `thought_level`,
  `options`, `response_timeout`, `stop_when_idle`.
- The editor's experimental actions (ADR 7.1): `permission`, `config` and
  `elicitation` (ADR 4.5).
- Events: `permission_resolved` and `session_deleted`, as above.
- Errors, hints and the usage text name these commands.

One name, one meaning, one spelling (#154). Where a name had two, one
wins, and the other form goes, with no alias (P9):

- `--option` is a config option, and only that: an id in `config get
  --option <o>`, an `<o>=<v>` pair in `config set`, `session new` and
  `session resume`, its value form following the verb as `--mode`'s does.
  The option of a permission request is `--option-id <id>`, ACP's
  `optionId`, a name ACP gives only to a permission option:
  `permission allow <session> <request> [--always] [--option-id <id>]`,
  and `reject` the same; `--option` there is an unknown option (exit 2,
  ADR 8.3). The socket's `allow` and `reject` take `option_id` in place of
  `option`, and refuse `option`, answering nothing: ignored, it would let
  a bridge that names an option get the kind's option instead (P3, P7).
  For the same reason an `option_id` that isn't a string (null, a number)
  is refused, answering nothing, not read as absent; only an absent one
  picks by kind.
- The prompt is `--prompt <text> | -` wherever a command takes one:
  `session new`, `session resume` and `prompt send`
  (`prompt send <session> [flags] [--prompt <text> | -]`, with a prompt, a
  file or an image: none is `nothing to send`, a usage error). A prompt
  given as words is a word too many, a usage error (ADR 8.3), and the rule
  that `prompt send`'s options go before its prompt is gone.
- A duration is in whole seconds, and its name has no unit: `--timeout`,
  `--stop-when-idle`, `--response-timeout`, and the profile's
  `stop_when_idle` and `response_timeout` (ADR 6.1) are so already. In
  `--json` and the socket's replies, `uptime_seconds` becomes `uptime`,
  `turn_seconds` `turn_elapsed`, and `timeout_seconds` (the time left
  before `response_timeout` answers a request) `timeout_left`.
- Config's names follow the same rule (ADR 6.1): `log` takes strings only,
  and a command in config is one shape.

**Why.** brnr knows sessions two ways: from its own transcripts and
processes, in any cwd, with no agent to ask; and from an agent's
`session/list`, which knows sessions started outside brnr, but only that
agent's, in one folder. One command joins the two answers to one question.
Commands that grew one at a time drift from ACP's names for the same
things, and `cancelled` is ACP's answer for a cancelled turn, not a
refusal. ACP's draft v2 makes modes, models and thought levels config
options told apart by category. Grouping by entity, with ACP's verbs, gives
one word for one thing (P1, P5); one row shape lets every table of
sessions read alike. Names that still had two meanings or two spellings
(#154) are what a script and a reader get wrong: `--option` meant a config
option in one command and a permission option in another, the prompt was
a flag in one and words in another, and a duration's name said its unit in
some places only (F5, P5).

**Consequences.**

- An agent offering only `allow_always` can't be allowed once: `allow`
  fails and says so.
- Only an agent that answers `resource_not_found` doesn't have a session,
  for `session delete --purge`. The Codex adapter (codex-acp 2.1.1)
  answers an unknown session as deleted, so `--purge` succeeds. The Claude
  adapter (claude-agent-acp 0.85.1) answers -32603, so `--purge` of a
  session it already deleted fails.
- `brnr session --help`, and each group's, lists its verbs; `brnr --help`
  lists the groups.

**Considered.**

- Grouping by the kind of id a command takes (session, request, message,
  pid): `event watch` and `event notify` take a session or a pid and fit
  neither, and mode, model and config stayed apart.
- Mirroring ACP's tree: nearly everything is `session/*`, so a `session`
  prefix on every command would say nothing.
- `process start`, opening a process with no session: a process stops with
  its last session (ADR 3.8), and a start without its prompt is the gap
  ADR 3.4 closed.
- brnr's index and the agent's list as two commands: two answers to one
  question.
- One command that asks every agent in every cwd: it would start an agent
  for each, and not every agent can list. The agent is asked only when
  named.
- Asking the agent's list through a running process's control socket: it
  needs a running process in the right folder.
- Each table of sessions with its own columns (`UPDATED` against
  `LAST ACTIVE`, one column folding state and pid, timestamps formatted two
  ways) and its own order.
- `permission respond <option>` with ACP's option ids alone: ids are the
  agent's, so a script would work for one agent; kinds are ACP's.
  `--option-id` keeps ids for what kinds can't say.
- `permission dismiss`: ACP has no outcome for it but `cancelled`, which is
  a cancelled turn's.
- `--config` for `--option`: it reads as a config file's path.
- `permission list`: what is listed are requests, not permissions.
- For `--option`'s two meanings (#154): renaming the config side, which is
  in more places (`config get`, `config set`, a start, the profile's
  `options`, the socket's `set_config`); `--config` for it reads as a
  config file's path (above). A positional option id for `allow` and
  `reject`: it would name the agent's id with no word to say what it is.
- The prompt as positional words everywhere: `session new` and
  `session resume` end their options with `-- <agent>`, which a prompt's
  words can't come before, and a positional word that starts with `-`
  must then be read as the prompt or as an option, a guess (P4). A flag's
  value is taken as given.
- A unit in every duration's name (`stop_when_idle_seconds`): longer
  names, all in one unit; one rule said once does as well.

## 8.2 Same data in text and JSON

- A command's text and its `--json` carry the same data (P5). `session list`
  is an index, one row per session (ADR 8.1). `process list` follows the
  same rule.
- `session status <session>` is one session's detail, as a flat object:
  owner, title, mode, model, whether a turn is running and for how long,
  running tools, plan progress, held messages, pending approvals and
  elicitations, token usage, the last turn (`last_turn`: `stop_reason`,
  `error`, `tokens`, ADR 4.4, with a line for it in the text), the last
  agent message (at most 4000 characters of it; the events have all of
  it), the session it was forked from (`forked_from`, ADR 3.12), and the
  agent (`agent`: `program`, the command brnr ran, and `name` and
  `version`, what the agent says it is in `initialize`, ACP's
  `agentInfo`). The control socket's `status` stays the full report, which
  bridges use; brnr shapes what it prints from it.
- Running tools are the running turn's (#164). A tool call the agent left
  unfinished, in a turn that was cancelled or failed, isn't running once
  the turn has ended; nor is one from a load's replayed history that ended
  or failed, nor a replayed call with a finished call's id. ACP has no
  status for such a call, so brnr gives it none.
- Text shows values, not JSON.
- Every command that prints data takes `--json`: one JSON value, or one
  event per line for `event log`, `event watch` and
  `session new --foreground`. A command that fails says so on stderr, with
  the same exit status, and with `--json` prints its error as its JSON,
  `{"error": {code, message}}` (ADR 8.7). `process stop` and `session close` take it too, though their
  answer is mostly their exit status: they print what they did, the process
  stopping (`{pid, status}`) and the session closed (`{session}`, as
  `session fork` names its session). `event notify` doesn't: its stdout is
  its command's.
- The usage starts with `acp`, then has a section per group (`process`,
  `session`, `prompt`, `queue`, `permission`, `elicitation`, `config`,
  `event`), then brnr (`doctor`, `skill`, `--version`). It says once, at
  the end, what a `<session>`, a `<request>` and a `<pid>` are, and that
  `--json` is everywhere. `brnr <group> --help` prints a group's part. A
  usage error prints that command's lines, not all of it.

One concept has one name and one shape in every command's `--json`, and in
the socket's replies where the commands print their fields as they are
(#150). A field means one thing and has one type wherever it is, and is
named as ADR 5.9 has it:

- **Lists.** A command that lists prints an object, the list under the
  plural of what it lists: `process list` `{processes}`, `session list`
  `{sessions}`, `permission requests` and `elicitation requests`
  `{requests}`, `doctor` `{checks}`, as `doctor --report --json`,
  `config get` (`{session, options}`) and `prompt commands`
  (`{session, commands}`) have theirs. It has no `version`: brnr's major
  version is the version of what it prints (ADR 8.4).
- **Rows.** A row is the object its command's detail prints, with fewer
  fields, none renamed or reshaped: `session list`'s are `session
  status`'s (with `source`), `permission requests`' are `permission
  show`'s, `elicitation requests`' are `elicitation show`'s, which has the
  process's `pid` as its row does; a `process list` row's `sessions` are
  `session list` rows with `session` and `state` only.
- **ACP's objects whole** (ADR 5.9). A permission request's `options` are
  the agent's, `{optionId, name, kind}`, in `permission requests` as in
  `permission show` and `permission_request`; `--option-id` takes an
  `optionId` (ADR 8.1).
- **A count or a list, never both.** A count of what is listed elsewhere is
  `<what>_count`, as `records_skipped`'s `acp_count` is: `session status`,
  and the socket's `status`, have `held_count`, `context_count`,
  `pending_count` and `elicitation_count`, where `held` and `context` are
  `queue list`'s lists, and `pending` and `elicitations` the socket's
  `pending` answer's.
- **`dropped`** is the messages dropped, a list of `{message, text,
  reason}` as their `message_dropped` events have them: in `prompt
  cancel`, `queue drop` and `queue clear`, and in `--wait`'s turn, where it
  is empty, or the message whose drop ended the wait (ADR 8.3).
- **`agent`** is `{program, name, version}` in `process list`, `session
  list` and `session status`: `program` the file name of the command brnr
  ran, `name` and `version` what the agent says it is in `initialize`
  (`agentInfo`); each null where brnr doesn't know it. A process that
  doesn't answer, an inactive session and a session only the agent lists
  have no `agentInfo` brnr kept. The socket's `status`, the full report,
  keeps the command as `agent`, as a profile's `agent` has it (ADR 6.1),
  and `agentInfo` as `agent_info`; the commands make `agent` of the two.
- **`owner`** is who owns a process's sessions, `editor` or `headless`,
  and null where it isn't known: a process that doesn't answer. Whether one
  answers is its `state` in `process list`, `starting`, `running`,
  `stopping` or `unreachable`, as a session in it is `unreachable` in
  `session list`.
- **Who and where.** `by` is who acted (ADR 5.9), never a process: a
  session's lock is held by `lock_pid`, and the processes serving it
  without it are `shared_pids` (ADR 3.9). `session fork` names the session
  it forked `forked_from`, as `session status` and `session list` do.
  `session delete` names the files `session_deleted` went into
  `recorded_in`: `recorded` is `history`'s boolean.
- **`event wait`** prints one shape for each `--for`, whatever ended the
  wait and whether it held when the wait began (ADR 8.3).

**Why.** Scripts and agents read `--json`, people read text (F5). If the two
carried different data, brnr would have two interpretations of one session
where P5 allows one, and a script could not see what a person sees.

At f017499 one thing had several shapes (#150): `dropped` was a reason in
`--wait` and a list of messages in `prompt cancel` and `queue`, so
`jq '.dropped | length'` worked on one and not the other; a permission
request was rewritten in `permission requests` (`name` dropped), ACP's in
`permission show`, and either that or its event in `event wait`, which
also made up an `idle` event and a `process_exited` with no fields but its
status; `agent` was a string in the lists and an object in `session
status`; `held` and `context` were counts in one and lists in another;
`owner` held reachability in `process list`; and every list was a bare
array. A script written against one command broke on the next, and ADR
8.4 would hold each of these for 1.x (F5, P5). One shape learned once is
fewer concepts (N4), and a filter written for a row works on the detail.
An object can gain a field in a minor release (ADR 8.4) where a bare array
can't: what belongs beside a list, such as the processes that didn't
answer, which go to stderr now, has room. Within a major, what changes in
an object is fields added, which readers ignore, so a `version` beside them
would never change; a 2.0 can add one if it needs one.

**Considered.**

- `session list --json` as every process's whole status report, while the
  text showed six columns.
- `session status` as a summary in text and the raw report in `--json`.
- `--json` on a few commands only.
- Bare arrays for lists, as they were: nothing can be added beside the
  rows without a break.
- A `version` in each list's object: within 1.x it could only stay `1`.
- JSON lines for lists, as `event log` prints: a list is one value, read
  whole; the `event` commands stream because events go on.
- `dropped` as the drop's reason in `--wait`: `prompt cancel` and `queue`
  drop several messages, so one name would keep two types.
- Lists in `session status` for `held` and `context`: it counts what
  `queue list` lists, as it counts the approvals `permission requests`
  lists, each with its tool call.
- A request in `permission requests` as brnr rewrote it, `{option_id,
  kind}`: it dropped the option's `name`, and was a third shape of one
  request.
- `agent` a string everywhere, with `agent_name` and `agent_version` in
  `session status`: three fields for one thing, and nowhere to add a
  fourth.
- `owner: unreachable`, as it was: a field with two lists of values.

## 8.3 Waiting, and exit status

- `brnr event wait <session> [--for idle|turn|permission|elicitation|exit]
  [--timeout <s>]`:
  - `idle` (the default): no turn running, nothing held, and no approval
    or elicitation waiting, also one whose turn has ended (ADR 3.8), as
    `--stop-when-idle` counts it. It returns at once if the session is
    idle already, so `prompt send` followed by `event wait` can't race. A
    closed session counts as idle. A process whose status lacks a count
    this needs (an older brnr's) fails the wait, saying which: a count
    that isn't there isn't 0 (P4).
  - `turn`: the next turn that ends.
  - `permission`: an approval is waiting (at once if one already is).
  - `elicitation`: an elicitation is waiting, as for `permission`
    (ADR 4.5).
  - `exit`: the process exits.
- Exit status: 0 when the condition is met and, for `idle` and `turn`, the
  last turn ended normally (`end_turn`). 1 if that turn failed or stopped
  for another reason, if its message was dropped, if the agent exited first
  (but for `exit`), or (for `turn` and `permission`) if the session closed
  first. 124 on `--timeout`, as timeout(1) does; a timeout doesn't cancel
  the turn. 3 when the command was cut off for falling behind and couldn't
  catch up from the transcript, saying what it missed (ADR 10.2). The same
  codes for `prompt send --wait`, `session new --wait` and
  `session resume --wait`. `event wait` on a session that is idle already
  exits as its last turn ended.
- Usage errors and help, the same in every command (#153):
  - A usage error exits 2: an unknown command, verb or option, an argument
    missing or one too many, a value a flag can't take, `brnr` or a group
    with no command. Nothing is done, and the command's usage is printed
    on stderr, after what was wrong where brnr can say it
    (`brnr: unknown option: --bogus`); for a command brnr doesn't have,
    `brnr --help lists every command` in place of all of it. `brnr acp`
    exits 2 so already. 1 stays an operation that failed, so a script
    tells a mistake in its own command line from a failure.
  - A usage error is what the command line alone shows wrong, checked
    before anything is read or done: flags that don't go together
    (`--timeout` without `--wait`), a value not of the flag's form or not
    one of its fixed choices (`--for`, `--events`, `--include`, `--option
    <o>=<v>`, `brnr skill`'s references), an empty `--prompt`. What
    depends on anything else is a failure, 1: a session, process or
    profile that isn't there, a mode or option the agent doesn't have, a
    file that can't be read, a prompt on stdin that is empty. `brnr acp`
    too: a config error there exits 1, as in `session new` (#338).
  - `--help` and `-h` print the usage on stdout and exit 0, wherever they
    are taken (ADR 8.1): `brnr`, a group without a verb
    (`brnr session --help`), a verb, `brnr acp`, `doctor` and `skill`
    (`skill install` too).
  - The usage names a thing one way: `brnr acp --help` says `<p>` and
    `<agent>`, as `brnr --help` does, not `<name>` and `<program>`.
  - The adapters' own `--help` (`brnr-claude-adapter --help`) is left as
    upstream has it. An adapter brnr packages is upstream's program,
    unchanged but for finding the agent and `--version` (ADR 11.1), and
    not part of what ADR 8.4 covers.
- A timeout too long to count is no timeout, not a crash. `--timeout` goes
  with `--wait`: without it, `prompt send` and `session new` fail, and
  nothing is sent.
- While `prompt send --wait`, `session new --wait` and
  `session resume --wait` wait, approvals and elicitations are announced
  on stderr, with how to answer them.
- `session new --wait` prints `started …` on stderr, so that stdout is the
  agent's reply and nothing else
  (`brnr session new --wait … > answer.md`).
- With `--json`, `--wait` prints the turn as one object at its end:
  `{session, message, reply, stop_reason, error, tokens, dropped}`, and for
  `session new` and `session resume` its `pid`. `tokens` is what the agent
  reported for the turn, or null (ADR 4.4). `dropped` is the messages
  dropped, as `prompt cancel` has them (ADR 8.2): empty, or the message,
  dropped before it was sent.
- With `--json`, `event wait` prints what met its condition in one shape
  for each `--for` (ADR 8.2), whatever ended the wait, and whether it held
  when the wait began:
  - `idle`: the session's state, `{session, state, last_turn}`: `state`
    `idle`, or `inactive` once it has closed, as `session list` has it, and
    `last_turn` as `session status` has it, null before the first turn.
  - `turn`: the turn's `turn_ended` event, as `event watch --json` prints
    it.
  - `permission` and `elicitation`: the request waiting, as `permission
    show --json` and `elicitation show --json` print it. The event of one
    answered before brnr asks for it doesn't end the wait: none is
    waiting.
  - `exit`: the `process_exited` event. A process gone without sending it
    (killed) gets the same event from the command, as far as it knows it:
    `exit_status` and `seq` null, `ts` when it saw the process go, and
    `error` saying so. Only once brnr has seen the process exit, as
    catching up decides (ADR 10.2): a connection that fails while it lives
    is a failure, 1, saying brnr can't tell that it exited (P4).

  Text says the same: `idle: end_turn`, `inactive: session closed (idle)`,
  `turn ended: end_turn`, `approval p1: Edit src/lib.rs`, `exited`.

**Why.** A script or a supervising agent needs to wait for a turn without
polling, and to know from the exit status alone how it ended (F5, P3). The
control commands exited 1 for a usage error, the code of a failed
operation, while `brnr acp` exited 2; `doctor --help` and `skill --help`
printed on stderr and exited 1, while a verb's `--help` printed on stdout
and exited 0 (#153). An
exit code is part of the interface (ADR 8.4), so one situation gets one code
in every command (F5, P5). 2 is what many utilities exit for a usage error
(`grep`, `ls`, the shell's builtins) and what `brnr acp` exits, and 0, 1, 3
and 124 already have their meanings. Help asked for is the answer, so stdout
and 0, as a verb's `--help` had it already. The line between a usage error
and a failure is what the command line alone shows, so the same words get
the same code whatever the machine holds (P5), and a script that gets 2
knows retrying can't help. Saying what was wrong before the usage serves
both of 8.1's needs: the option named, and the command's form to fix it
by. A command brnr doesn't have has no usage of its own, and all of
`brnr --help` for a typo buries the one line that matters. An editor's
process reads the same config as `session new`, and nothing in what an
editor does with the code needs it to differ: the editor shows stderr
either way (ADR 3.3), so one situation keeps one code (P5).

**Considered.**

- A command per condition (`wait-idle`, …): one command with `--for` is
  smaller.
- A usage error exiting 1, as the control commands do: `brnr acp` would
  need changing instead, and a script couldn't tell a mistake from a
  failure.
- An exit code of its own for each kind of usage error: one code says the
  command line was wrong; stderr says how.
- sysexits' 64 (`EX_USAGE`): few utilities use it, and `brnr acp` has 2.
- Help on stderr: help asked for is the output, and `brnr --help | less`
  should work.
- A usage error only for what the parser catches, every other check a
  failure: `--timeout` without `--wait` would then exit 1 while
  `--timeout soon` exits 2, two codes for one kind of mistake.
- `brnr acp` exiting 2 for a config error, as it did: the same config
  error would exit 2 there and 1 in `session new`.
- Making the adapters' `--help` print a usage without starting ACP (as a
  comment on #153 asked): it would change upstream's program past what
  ADR 11.1 adds, for an interface ADR 8.4 doesn't cover.

## 8.4 What the compatibility promise covers

From 1.0.0, P9's "compatibility is kept within a major version" is of what
is covered below, and of nothing else. Before 1.0.0, P9 has changes made
outright, with no aliases or shims. Nothing is implemented: the promise
starts at 1.0.0.

Covered:

- **The command line**: its commands, their flags and positional
  arguments, and what each means. The library's facade (ADR 8.5) is
  covered as part of it: it mirrors the command line, and changes as it
  does. Cargo's semver binds every public item on crates.io anyway.
- **`--json` structures**: what each command prints with `--json`, its
  fields, their types and what they mean. A field that is a message for
  people carries text whose wording isn't covered.
- **Exit codes**: what each command exits with, in each situation
  `docs/interface.md` lists.
- **Error codes**: each error's `code` (ADR 8.7), wherever brnr reports an
  error as data, and what it means; its `message` isn't covered, nor is
  `unclassified`, which promises nothing. ACP's error the agent answered
  with, `rpc_error`, is ACP's.
- **Events for bridges**: each event's name and fields, wherever it goes:
  a bridge's stdin, `--json` lines (`event log`, `event watch`,
  `session new --foreground`), `event notify`'s stdin and its `BRNR_*`
  variables.
- **The bridge protocol, as a bridge brnr starts sees it**: a bridge
  started from the profile, its requests on its stdout and its replies and
  events on its stdin: each request's `cmd`, its fields and its reply; how
  long a bridge lasts (events until its process exits, then stdin EOF, then
  SIGTERM); and `BRNR_PID` in its environment. Only bridges brnr starts are
  covered: a client that connects to the control socket itself has no
  promise, whatever it speaks.
- **Transcripts**: the events file and the raw ACP file of each session
  under `$BRNR_HOME` (`projects/<folder>/<session id>.jsonl` and
  `.acp.jsonl`), and their records, in format 1. A 1.x brnr reads what an
  earlier 1.x wrote (`event log`, `session list`, `session resume`). What
  each record promises, and how a reader tells a format it doesn't read,
  is ADR 5.10's.
- **Where brnr's files are**: how the state directory (`$BRNR_HOME`, else
  `~/.brnr`) and the runtime directory are found, and where each file brnr
  writes under `$BRNR_HOME` is, the session locks
  (`$BRNR_HOME/sessions/<id>.lock`, ADR 3.9) among them. Where a file is
  is covered; what it contains is covered only where this list says so
  (transcripts), not for host logs or lock files.
- **Config TOML**: the file (`BRNR_CONFIG`, else its default path), its
  profiles and their sections, keys and values, and the names
  `experimental` and `features` take.
- **ACP, and what `brnr acp` passes through**: brnr speaks ACP version 1 to
  agents, and to editors through `brnr acp` (ADR 2.2). `brnr acp` passes
  what is between the editor and the agent as P1 says: unchanged, but for
  the changes named there (ADR 3.1, 3.9, 7.1), with the defaults they have
  at 1.0.0.

The environment variables brnr reads are inputs, as flags are, and go with
what they belong to: `BRNR_CONFIG` and `BRNR_START_TIMEOUT` with config,
`BRNR_HOME` with where brnr's files are. `BRNR_DIR` is covered as where the
runtime directory is, not as what is in it. Error codes are covered
(ADR 8.7): each code, and what it means, wherever an error is reported as
data; `unclassified` promises nothing, and a failure under it may get a
code of its own in a minor release.

Not covered: whatever isn't listed above, and in particular:

- Human-readable output: tables, summaries and events as text, their
  columns and layout; `--help` and usage text; `doctor`'s check lines and
  `doctor --report`; `brnr skill`. Scripts read `--json`.
- The wording of error messages, wherever they are: on stderr, in a
  `--json` field, in a reply's `error`.
- What host logs (`$BRNR_HOME/hosts/<run id>.jsonl`) and session lock files
  contain: their records and fields.
- The control socket. It is in the runtime directory, which is private
  (P13, ADR 3.9), and it is not an interface: it is how brnr's own
  commands and the bridges brnr starts reach a process. Its path
  (`$BRNR_DIR/<pid>.sock`), how processes are found, their metadata files
  (`<pid>.json`), and connecting to it directly, by `BRNR_SOCKET` (which a
  started bridge has) or otherwise, have no promise. A client connected
  directly speaks the bridge protocol, but what it speaks, and whether it
  is answered, can change in any release.
- What is in the runtime directory (`BRNR_DIR`), and its layout: sockets
  and metadata files. How it is found is covered, as above.
- `brnr host`, which brnr starts itself, never a person.
- The library's internals, behind the feature that is off by default
  (ADR 8.5).

Compatible and breaking changes in 1.x:

- Compatible, so in a minor release: a new command, flag, JSON field,
  event, event field, bridge request, config key, or an exit code for a
  situation that had none.
- Breaking, so in a major release only: removing or renaming any of those;
  changing a field's type or what it means; changing the exit code of a
  situation that has one.
- Whoever reads what brnr writes ignores the fields and events it doesn't
  know: bridges, scripts reading `--json`, `event notify` commands, readers
  of transcripts. A bridge subscribed to every event gets the new ones too.
- A deprecated form keeps working until the next major version: the
  changelog says so under Deprecated, with what to use instead.

Security fixes are made on the latest major version, in a release of it;
upgrading within that major is the fix, and breaks nothing covered. There
is one major so far, so no support period for older majors is set; it will
be, by a record that amends this one, once a second major exists.

The promise starts at 1.0.0. #150 (`--json` shapes) is decided and made
(ADR 8.2), as are #151 (event and transcript record names: ADR 5.9, #314),
#70 (what a transcript promises, its format marked: ADR 5.10), #153 (the
exit code of a usage error, `--help`: ADR 8.3, #310), #154 (flags and
config keys spelled two ways: ADR 8.1 and 6.1, #311), and #205 (error
codes: ADR 8.7). What they settle is what 1.0.0 promises.

**Why.** P9 alone says compatibility is kept within a major version, but
not of what, and each of brnr's interfaces is a different kind of promise
(#148). Bridges and the control socket speak one protocol, but the socket
is private (P13); leaving the protocol out with it would leave a bridge's
requests, such as answering a permission, with no promise.

**Consequences.**

- `docs/interface.md` marks what is covered and what isn't; SECURITY.md,
  CHANGELOG.md's conventions, CONTRIBUTING.md and AGENTS.md say the 1.0
  policy next to the pre-1.0 one.
- From 1.0.0, a breaking change to what is covered waits for 2.0.0. A
  `**Breaking:**` entry in a 1.x release is wrong; a form that is replaced
  is deprecated instead, and the old form kept.
- A bridge that needs the promise is started from a profile. One that
  connects to the socket itself works, with no promise.
- brnr still refuses a config key it doesn't know (P7, ADR 6.1): a profile
  using a key added in 1.3 doesn't load in 1.2.
- Scripts match exit codes, `--json` fields and error codes (ADR 8.7), not
  messages.
- Text output can change in any release, for people: P5 has it carry the
  same data as `--json`, so nothing is in it that `--json` doesn't have.

**Considered.**

- Leaving the bridge protocol out with the socket: bridges' requests, which
  go over the same protocol on a started bridge's stdin and stdout, would
  have no promise.
- Covering the socket too: it is in the private runtime directory, there
  for brnr's own commands and the bridges it starts, and the runtime
  directory's layout, how processes are found and their metadata would be
  held for 1.x. A started bridge has all a bridge needs.
- Covering text output: two contracts for the same data (P5), and text is
  for people, who are served by changing it.
- Covering the wording of errors: every better message would wait for a
  major. Error codes (ADR 8.7) let the wording change freely.
- Covering what host logs contain: they are what a process records for
  itself and for `doctor`, not for others to read; the session's story is
  in its events file (ADR 5.1).
- A support period for older majors now: there is only one.
- Keeping the library public: every internal change would be a semver
  change (ADR 8.5).

## 8.5 The library is private, and its facade mirrors the command line

Implemented, but for the typed facade (#215): `brnr::run` is the library's
one public item.

- The library is private. Its modules are `mod`, not `pub mod`, and an item
  is `pub(crate)` unless something outside reaches it (below).
- Its public API is a facade that mirrors the command line, and nothing
  else. From 1.0.0 the facade is covered by the compatibility promise
  (ADR 8.4); what isn't in it is not. Only the facade is what the crate
  published on crates.io offers (ADR 11.6).
- `brnr::run(args)` is the facade's coarsest piece, and its only one so
  far: `brnr <args>` as a function, `args` being the words after `brnr`,
  run to the exit status the binary would exit with. The dispatch (`acp`,
  `host`, the rest) is `run`, and the command modules (`ctl.rs`, `ctl/`)
  are the library's; `main.rs` calls `run` with its arguments and does
  nothing else. `run` adds no surface the command line doesn't have: its
  words and exit statuses are the command line's.
- The typed facade has this shape: one module per command group
  (`process`, `session`, `prompt`, `queue`, `permission`, `elicitation`,
  `config`, `event`, ADR 8.1), and the tools at the top level (`doctor`,
  `skill`), with one function per verb (`brnr::session::list`,
  `brnr::prompt::send`), taking the command's arguments and flags, and
  returning the data its `--json` prints, with the same names and shapes.
  An error carries the code the command's error has (ADR 8.7); its
  message, like the command's, is not covered. It isn't
  built yet (#215): its return types are the `--json` shapes, which ADR
  8.2 makes one name and one shape per concept (#150). A record of its own
  says so if it departs from this shape.
- The crate's own tests and fuzz targets reach internals through
  `brnr::internals`, a `#[doc(hidden)]` module behind the `internals` Cargo
  feature, off by default. It re-exports what they use (`frame`'s reading
  and writing, `json`'s parsing and spans, `schema`'s types, `log::rfc3339`,
  the error codes' names (ADR 8.7) and `host::fuzz`'s harness), and nothing
  more. The integration tests
  turn it on with a dev-dependency of the crate on itself, so `cargo test`
  needs no flag; `fuzz/Cargo.toml` turns it on for the fuzz targets.
  `host::fuzz` is compiled only for unit tests and with the feature.
- `internals` is not covered by any compatibility promise, before or after
  1.0.0: its items change, move or go whenever the tests do. The crate's
  documentation, `Cargo.toml` and this record say so. With the feature on,
  `unreachable_pub` warns of a `pub` item that nothing outside reaches, so
  `pub` keeps meaning reachable through `run` or `internals`.

That nothing else is public is the compiler's to check: a private module
can't be named from outside the crate, and `cargo doc` without the feature
lists `run` alone. The fuzz targets building against `internals` is
checked by the fuzz workflow's build (`.clusterfuzzlite/build.sh`).

**Why.** `cargo publish` uploads the library at every release (ADR 11.6),
and from 1.0.0 Cargo's semver rules apply to every `pub` item in it, though
none was meant for anyone but brnr. Holding them still would hold back
changes users never see; letting them move would break P9's promise for
whoever used them. brnr's interface is its command line, with `--json` as
its data for programs (ADR 8.2). A library is useful only as that
interface again, not a second one: P5's one interpretation, reached two
ways, with nothing the command line doesn't say.

**Consequences.**

- `cargo doc` without the feature lists `run` alone. The command line, with
  `--json`, is the interface.
- A Cargo feature can't be hidden from crates.io: `internals` is in the
  published manifest and anyone can turn it on. What it reaches is no
  promise, and is said to be none.
- Cargo turns a feature on for the whole of one build: under `cargo test`
  the binary the tests drive is built with `internals` too, and so has the
  fuzz harness and its `StartChannel` variant, which nothing in the binary
  calls. `cargo build` and `cargo install`, what users run, build without.
- Until the typed facade is built, a program that wants brnr's data runs
  the binary, or `brnr::run`, with `--json`.

**Considered.**

- Keeping the library public, and treating every change to a `pub` item as
  a semver change from 1.0.0: the library would hold still for users it was
  never written for, and every refactor would be a major version or a
  break.
- `#[doc(hidden)]` on public modules, with the README and ADR 11.6 saying
  the library isn't covered: the items would still be reachable by path,
  and a promise kept only in prose is one a user can miss and a tool like
  `cargo semver-checks` doesn't see. Private modules are the compiler's
  promise.
- Everything private, with the tests and fuzzers under a `cfg` instead of a
  feature: `cfg(test)` is set only for the crate's own unit tests, not for
  `tests/` or `fuzz/`, which are separate crates; a custom `--cfg` would
  have to be passed to every `cargo test`, by hand and in CI. A feature can
  be turned on in the manifests that need it.
- Moving the integration tests and fuzz targets into the crate as unit
  tests: the integration tests drive the real binary against
  `tests/fake_agent.py`, and cargo-fuzz builds its targets as a crate of
  its own.
- A second, unpublished crate holding the internals, which `brnr` and the
  tests both depend on: a published crate can't depend on an unpublished
  one, so it would be published too, with the same promise problem, and a
  workspace for it.
- Building the typed facade before #150: its return types would be
  `--json` shapes that #150 changes, so it would be built twice or would
  freeze them.

## 8.6 A skill for agents that use brnr

Agents use brnr to run other agents: start headless workers, send them
follow-ups, wait for their turns, read what they did, and bring their
approvals to the user. How one agent should manage another is part of
brnr's contract, so the skill states it as principles, S1 to S7, each
checked against ADR 1:

- **S1. The CLI is the interface.** The skill teaches brnr's commands as
  they are, and adds nothing between them and the agent: no scripts, no
  MCP server, no wrapper. The headless CLI behaves the same with every
  agent (P12) and prints the same data in text and JSON (P5, ADR 8.2); a
  wrapper would be a second interpretation of it. Every command the skill
  shows is one `brnr --help` lists, with `--json` wherever it prints data,
  and runs as a test.
- **S2. Exact ids, never guesses.** A session is the `session` that
  `session new --json` printed, or a row of `session list --json`; a
  request the `request` of `permission requests --json` or
  `event wait --for permission --json`; a message the `message` of
  `prompt send --json`. Never "the only session", never an id read out of
  the text output, never one remembered from another conversation without
  finding it in `session list --json` (P4, ADR 3.2, ADR 4.1).
- **S3. Wait, don't poll.** `prompt send --wait`, `session new --wait` and
  `event wait --for idle|turn|permission|exit`, each with a `--timeout`, so
  that a stuck worker can't hang its supervisor; never a loop of `sleep`
  and `session status`. The exit status is the result (ADR 8.3): 0 the turn
  ended normally, 1 it failed, stopped, or its message was dropped, 124 the
  timeout passed, which says nothing about the worker: its turn goes on,
  and waiting again, cancelling or asking the user is a choice to make.
- **S4. Approvals belong to the human.** `permission allow` and
  `permission reject` are the user's answers. An agent gives one only where
  the user delegated it, explicitly and for that scope: which sessions, and
  which kinds of request (`edit` in `src/`, `execute` of `cargo test`), for
  the task at hand. It never widens a delegation, and never changes a
  worker's mode to one that asks less (`acceptEdits`, `bypassPermissions`)
  to avoid asking, unless the user asked for that mode: how much the agent
  asks is its mode (P2, ADR 4.5), and choosing it is the user's. By default
  the agent shows the request (`permission show`), says what it is in a
  sentence, and waits for the user's answer. A request on an editor's
  session is answered in the editor (ADR 7.1).
- **S5. Clean up what you start; touch nothing else.** Workers start with
  `--stop-when-idle`, so that a supervisor that dies or forgets leaves no
  orphan (P14, ADR 3.8), and are closed (`session close`) when their work
  is done; `process stop <pid>` only for a process the agent started.
  Sessions it didn't start, an editor's above all, are observed
  (`session list`, `session status`, `event log`, `event watch`), never
  acted on (P11, ADR 7.1), and never taken over (`--take-over`) unless the
  user asks for it in so many words (ADR 3.9).
- **S6. Nothing dropped silently.** A turn that failed, a message dropped
  (`dropped` in `--json`, a `message_dropped` event), a timeout, an agent
  that exited: each is told to the user, with what brnr said, and not
  retried quietly. Retrying is the user's decision, or one the user
  delegated (P3).
- **S7. A short core, details on demand.** `SKILL.md` has when to use brnr,
  S1 to S6 as rules, and the dozen commands most uses need, short enough to
  load whole every time. The rest is in references, read when the task
  needs them.

The principles hold brnr's user to what brnr holds itself to (P3, P4,
P14), and leave to the user what brnr leaves to the agent or the user (P2,
P11). None breaks a principle. The skill is documentation and a CLI
command: outside ACP, and none of the kinds of "beyond" P1 marks.

What it covers:

- `skills/brnr/SKILL.md`: frontmatter (`name: brnr`, a `description`
  saying to use it when asked to run, supervise or talk to coding-agent
  sessions through brnr); when to use brnr; the rules; the core commands.
- `references/orchestrate.md`: a worker's life (start, send, wait, read,
  close), several workers at once, the exit statuses, `--stop-when-idle`,
  steering and interrupting, held messages.
- `references/approvals.md`: `permission requests`, `show`, `allow`,
  `reject`, and S4 in full: what a delegation is, what isn't one, and what
  to show the user.
- `references/observe.md`: `session status`, `event log`, `event watch`,
  `event notify`, choosing events, and the events' JSON, as brnr prints it.
- `references/setup.md`: installing brnr and the adapters, an editor's
  agent command, profiles, `doctor`, installing the skill.

How it ships:

- In the repository as `skills/brnr/` (`SKILL.md`, `references/*.md`), and
  in the binary, embedded at build time (`include_str!`), so the skill an
  agent reads is the one for the brnr it runs.
- `brnr skill` prints `SKILL.md`; `brnr skill <reference>` prints one
  reference (`orchestrate`, `approvals`, `observe`, `setup`), so that an
  agent that only has the binary can read them. Neither takes `--json`:
  they print a document, not data.
- `brnr skill install [--dir <dir>]...` writes the skill to `<dir>/brnr/`,
  for each `--dir`, and by default to both `~/.claude/skills/brnr/` and
  `~/.agents/skills/brnr/`: a fixed, documented rule that doesn't depend on
  which agents are installed (P4). It replaces the files it writes, removes
  references a newer brnr doesn't have, and prints each directory it
  wrote. A directory that can't be written fails, saying which (P7).
  Installing again after upgrading brnr updates it.
- The skill's Markdown files are in `DOCS` in `tests/docs.rs` (#25), so
  every line of their `sh` blocks runs against the fake agent or is skipped
  with a reason, and every `toml` block loads as a config.

Left out: an MCP server or scripts (S1); a Claude Code plugin
(`skill install` covers it for now, and a plugin can package the same
files later); writing bridges (the bridge protocol is for programs, the
README has it, and the skill points there); acting on an editor's session
(ADR 7.1's experimental actions; the skill says not to, S5); `doctor`
checking an installed skill against the binary's.

**Why.** Without guidance, an agent drives brnr as a script would: it picks
"the only session", polls `session status` in a loop, approves what blocks
it, retries what failed, and leaves workers running. Each breaks a
principle brnr holds itself to (P3, P4, P14), done by brnr's user. Agents
learn a tool from a skill: a `SKILL.md` with a `name` and a `description`
saying when to use it, loaded when the description matches, with further
files read only when needed. Claude Code reads skills from
`~/.claude/skills/<name>/` (and `.claude/skills/` in a project); Codex from
`~/.agents/skills/<name>/` (and `.agents/skills/` from the working
directory up to the repository's root), as their documentation has it in
October 2026.

**Consequences.**

- Evals with a real agent are not done: checking that it used exact ids,
  waited rather than polled, didn't approve without a delegation, and
  cleaned up. The examples' tests check that the skill's commands are
  right, not that an agent follows its rules.
- A change to the CLI that breaks an example in the skill fails the tests,
  as one in the README does.

**Considered.**

- A skill outside the repository (a separate repo, a plugin marketplace):
  it would drift from the CLI it describes.
- `brnr skill install` only where the agent's directory already exists:
  what is installed would depend on what happens to be there (P4).
- Requiring `--dir` (or `--claude`, `--codex`): every install would name
  the obvious places; `--dir` stays for a project's `.claude/skills` or
  `.agents/skills`.
- One long `SKILL.md`: loaded whole on every use, most of it unneeded (S7).
- Delegating approvals by default for "safe" kinds (`read`): what is safe
  is the user's call, and the agent's mode already says what it asks
  about (P2).

## 8.7 An error is a stable code and a message

Resolves #205.

Wherever brnr reports an error as data, it has one shape, `"error":
{"code": "<code>", "message": "<text>"}`:

- a socket or started bridge's reply that fails: `{"ok": false, "error":
  {code, message}}`;
- an event, note or record that reports a failure: `turn_ended`,
  `process_exited` (when brnr itself died), `events_missed`,
  `records_skipped`, and the host log's `start_failed`, `open_failed`,
  `lock_failed`, `timeout_failed`, `display_stopped`, `process_panicked`,
  `session_log_failed` and `editor_request_refused`. #70 marks these
  records' format as it marks every record's (ADR 5.10);
- the `--json` fields that hold one: `--wait`'s turn's and `last_turn`'s
  `error` (ADR 8.3, 8.2), `session delete`'s `error`;
- a command that fails, with `--json`: its stdout is `{"error": {code,
  message}}`, its one JSON value, unless it had printed its JSON before it
  failed (a stream's events); then stderr alone says so, and the stream
  stays one value per line. A wait whose turn failed exits 1 having
  printed the turn, whose `error` is the turn's (ADR 8.3).

`code` is a word from the list below; `message` is for people. The
rules:

- **A code is snake_case, subject first**, as a record's name is
  (ADR 5.9): `session_not_found`, `process_unreachable`,
  `transcript_not_kept`. Never a number: JSON-RPC's numbers are ACP's.
- **One code per failure a caller can act on, not per place brnr fails.**
  A script that gets `session_inactive` resumes the session; one that gets
  `session_closing` tries again in 2 s (ADR 5.5); `session_active` closes
  it first, or takes it over. Failures a caller handles alike share a
  code: every malformed socket request is `command_invalid`.
- **`unclassified` is the catch-all, and promises nothing.** A failure brnr
  gives no code of its own (an I/O error on brnr's own files, an agent's
  answer that isn't ACP, what brnr didn't foresee) is `unclassified`.
  Scripts must not match it: a failure under it may get a code of its own
  in a minor release. Every other code keeps its meaning through the
  major version (ADR 8.4).
- **The agent's error stays the agent's** (F1, P1). Where what failed is
  the agent's answer, a JSON-RPC error, the code is `agent_failed`, and the
  error has the agent's beside it as `rpc_error`, whole and unchanged
  (`{code, message, data}`, ACP's numeric code): `{"code": "agent_failed",
  "message": "session/new failed: Authentication required", "rpc_error":
  {"code": -32000, "message": "Authentication required"}}`. brnr's code says
  whose failure it was, the agent's what it was; brnr reads no ACP code
  into one of its own (P4), and a script that needs ACP's reads
  `rpc_error.code` (`session delete` exits 0 for its `-32002`, ADR 8.1).
  A turn the agent failed is such an error in `turn_ended`, and so in
  `last_turn` and `--wait`'s turn.
- **ACP stays ACP.** What brnr answers in the agent's place, through `brnr
  acp`, is a JSON-RPC error as ACP has it (`-32603`, brnr's message); the
  host log's `editor_request_refused` has brnr's code (P1).
- **Text says the code too** (P5): a failed command prints `brnr:
  <message> [<code>]` on stderr, in text and with `--json` alike, and an
  error in an event shown as text has its code after its message: at the
  end of its first line, where lines follow that are the agent's (its last
  lines on stderr, ADR 3.6). `brnr acp` says its errors to the editor's
  log as it did, with no code: the editor gets none as data.
- **What isn't an error as data has no code.** A usage error (exit 2) is
  the command line's mistake: the exit status says what it is, the usage
  how to fix it (ADR 8.3). A wait's timeout (124) and a cut-off it couldn't
  catch up on (3) have their exit codes, and the latter its
  `events_missed`, whose `error` says why. `doctor`'s failed checks are its
  data, `{checks}`: it exits 1 with how many on stderr.
- The exit status stays coarse (ADR 8.3): every code above is a failure,
  exit 1, wherever a command exits with it.

The codes, by subject:

| Code | The failure |
|---|---|
| `acp_version_unsupported` | The agent speaks an ACP version brnr doesn't (ADR 2.2). |
| `agent_exited` | The agent exited, and its process with it, before the command was done: a start, a wait, a request. |
| `agent_failed` | The agent answered with a JSON-RPC error, which `rpc_error` has. |
| `agent_not_started` | No agent to start: none given or recorded, or its program can't be run. |
| `answer_invalid` | An answer that doesn't fit its permission request or elicitation: no such option, a kind missing or doubled, the other side's option, content that doesn't fit the form, an action that isn't one (ADR 8.1, 4.5). |
| `capability_missing` | The agent doesn't advertise what was asked: forking, closing, resuming, listing or deleting sessions, steering, images, an MCP server's transport. |
| `command_invalid` | A socket request that isn't one: not JSON, no `cmd`, a field missing or of the wrong type, fields that don't go together. |
| `command_unknown` | A `cmd` the process doesn't have, which an older brnr's process says of a newer one's request (#149); or a `status` answer without a field this brnr reads (a count, `busy`), as an older brnr's has it (ADR 8.2). |
| `config_invalid` | The config file can't be read, isn't private, or doesn't load (ADR 6.1, 9.4). |
| `event_unknown` | An event the process doesn't have, or a note, in `subscribe`'s `events` or a bridge's (ADR 5.9). |
| `experimental_disabled` | An action on an editor's session its profile doesn't enable (ADR 7.1). |
| `input_invalid` | What the command reads besides its command line won't do: an empty prompt on stdin, stdin that isn't the form's fields, a file or image that can't be read or sent, a `--cwd` that isn't a directory. |
| `message_not_found` | No held message has that id (ADR 4.2). |
| `process_not_found` | No brnr process has that pid. |
| `process_owned_by_editor` | An editor's process: no session is opened or forked in it (ADR 7.1). |
| `process_panicked` | brnr itself failed, and its process ended (ADR 3.7): a bug to report. |
| `process_starting` | The process hasn't committed its start (ADR 3.4). |
| `process_stopping` | The process is stopping, or its agent takes no more input. |
| `process_unreachable` | The process doesn't answer, or can't be reached from here (another `BRNR_DIR`, ADR 3.9). |
| `profile_not_found` | The config has no profile of that name. |
| `request_not_found` | No permission request or elicitation of that handle is waiting (ADR 4.1). |
| `session_active` | The session is open in a process, where the command needs it not to be: close it first, or take it over (ADR 3.9). |
| `session_closed` | The session closed before the command was done. |
| `session_closing` | The session is closing; it is free once its records are written (ADR 5.5). |
| `session_inactive` | The session isn't open in any process; it can be resumed (ADR 3.11). |
| `session_not_found` | brnr knows no session of that id. |
| `session_opening` | The session is opening, and takes no requests until it has (ADR 3.4). |
| `setting_invalid` | A mode, model, thought level, config option or login method the agent doesn't offer, or a value it doesn't take (ADR 6.2, 4.7). |
| `start_timed_out` | The session didn't open in the time its start had (ADR 3.4). |
| `strict_mode_refused` | Strict mode refuses what isn't stable ACP (ADR 2.1). |
| `transcript_not_kept` | The process keeps no transcript to read, or no raw ACP (`log`, ADR 6.1). |
| `transcript_unsupported` | The transcript is in a format this brnr doesn't read (ADR 5.9, 5.10). |
| `turn_owned_by_editor` | The editor's session's turns are the editor's: a steer, an interrupt, or a prompt while its turn runs (ADR 7.1). |
| `unclassified` | Anything else. No promise: don't match it. |

src/error.rs has the list, docs/interface.md lists it for users, and a test
checks that the two agree and that a test reaches each code.

**Why.** At 8d4b1dd every error was text, at about 155 places: a reply was
`{"ok": false, "error": "<text>"}`, a failure event had a text `error`,
the CLI printed the text. A script or bridge that had to tell one failure
from another could only match the wording, which ADR 8.4 leaves out of the
promise so that it can get better (F5). A code inside the promise, beside
a message outside it, lets both hold. One shape everywhere is one concept
learned once (N4, ADR 8.2), and a field of one type (ADR 5.9): an
`error.code` that was a number in `turn_ended` and a word in a reply would
be two. A code for each failure a caller can act on keeps the list short
enough to learn and handle, which a code per call site wouldn't; the
catch-all is what lets the list stay short without a promise brnr can't
keep: naming a new failure later breaks no script, since none may match
`unclassified`. ACP's error, whole beside brnr's word, is what the agent
said, unchanged (F1), and brnr's word is what brnr knows: that the agent's
answer is what failed (P4). With `--json`, the code is in the JSON a script
already reads; on stderr, a script would parse lines written for people.

**Considered.**

- Kebab-case codes, as the issue had them, after the records' names then:
  ADR 5.9 makes every name in brnr's JSON snake_case.
- Numeric codes: JSON-RPC's are ACP's, and a word says what it is.
- One code per call site: about 155 codes, most of them handled alike.
- The agent's error as `error` itself, its `code` a number, beside brnr's
  with a word: one field, two types (ADR 5.9).
- Reading ACP's codes into brnr's (`-32002` as `session_not_found`): a
  second interpretation of what the agent said, and a guess where the
  agent's code is its own (P4, F1).
- brnr's code in the JSON-RPC errors it answers the editor with, in `data`:
  what the editor sees is ACP (P1), and the host log has the code.
- The code first on stderr (`brnr: session_not_found: no session s`): a
  person reads the message; the code is for looking up, and a script reads
  `--json`.
- No code in text: text and JSON would carry different data (P5).
- The error object on stderr with `--json`: stderr has other lines for
  people (warnings, a process not answering), so it would not be one JSON
  value; stdout is where `--json` is.
- An error object for usage errors too: exit 2 says it, and a usage error
  is found before `--json` is known to be one of the command's flags.
- `internal` for the catch-all: what falls under it isn't only brnr's bugs
  (a full disk, an agent's malformed answer), and brnr's bug has a code of
  its own, `process_panicked`.
- A code for `doctor`'s failed checks: its checks are the data, and each
  says what failed.

**Consequences.**

- Scripts match `error.code`, and ACP's `error.rpc_error.code` where the
  agent failed; never `message`, nor `unclassified`.
- A minor release can add a code, for a new failure or for one that was
  `unclassified`; removing or renaming one, or changing what it means,
  waits for a major (ADR 8.4).
- A new failure is given an existing code where a caller would act on it
  the same way, else a new one; `unclassified` only where no caller could
  act on it.
- The typed facade (#215) returns the code with its error (ADR 8.5).
- #149 answers a request an older process doesn't have with
  `command_unknown`.

## Tests

Run `cargo test --release adr_0008_`. Named claims, by section:

- 8.1: [tests/cli.rs](../../tests/cli.rs):
  `adr_0008_old_commands_are_unknown`,
  `adr_0008_old_socket_commands_are_unknown`,
  `adr_0008_profile_keys_and_actions_have_the_new_names`,
  `adr_0008_help_lists_the_groups_and_their_commands`,
  `adr_0008_prompt_send_takes_its_prompt_as_a_flag`,
  `adr_0008_a_flag_is_never_a_session_or_pid`,
  `adr_0008_a_single_dash_word_is_an_argument`,
  `adr_0008_an_unknown_option_is_said`,
  `adr_0008_config_get_lists_options_choices_and_modes`,
  `adr_0008_config_get_narrows_by_category_and_id`,
  `adr_0008_config_set_by_category_and_by_id`,
  `adr_0008_new_and_resume_replace_start`,
  `adr_0008_thought_level_is_the_option_of_its_category`,
  `adr_0008_response_timeout_flag_wins_over_the_profile`,
  `adr_0008_allow_and_reject_pick_by_kind`,
  `adr_0008_a_missing_or_doubled_kind_fails`,
  `adr_0008_option_must_be_on_the_verbs_side`,
  `adr_0008_a_permissions_option_is_option_id`,
  `adr_0008_a_permissions_option_id_is_a_string`,
  `adr_0008_a_durations_name_has_no_unit`, `adr_0008_queue_show`,
  `adr_0008_queue_clear_flags`,
  `adr_0008_list_without_an_agent_is_brnrs_index`,
  `adr_0008_list_joins_the_agents_sessions_on_id`,
  `adr_0008_list_reads_the_agents_times_in_utc`,
  `adr_0008_list_finds_a_cwd_however_it_is_spelled`,
  `adr_0008_list_include_filters_by_state`,
  `adr_0008_list_stops_an_agent_that_wont_go`,
  `adr_0008_delete_keeps_the_transcript`,
  `adr_0008_delete_a_session_only_the_agent_knows`, `adr_0008_delete_purge`,
  `adr_0008_delete_purge_fails_when_the_agent_does`,
  `adr_0008_delete_purge_says_what_it_couldnt_delete`,
  `adr_0008_purge_deletes_an_older_brnrs_files_too`,
  `adr_0008_delete_records_in_each_transcript_it_can`,
  `adr_0008_an_editors_deletion_in_every_folder_isnt_activity`,
  `adr_0008_delete_refuses_an_open_session`,
  `adr_0008_delete_needs_an_agent_that_can_delete`,
  `adr_0008_new_in_a_running_process`,
  `adr_0008_resume_in_a_running_process`,
  `adr_0008_resume_in_a_running_process_by_loading`,
  `adr_0008_pid_refuses_process_flags`,
  `adr_0008_pid_refused_when_it_could_never_close`,
  `adr_0008_pid_settings_that_fail_close_the_session`,
  `adr_0008_pid_session_that_cant_be_locked_is_closed`,
  `adr_0008_pid_given_up_before_its_commit_sends_no_prompt`,
  `adr_0008_pid_timeout_abandons_the_opening`,
  `adr_0008_pid_opening_keeps_the_process_running`,
  `adr_0008_pid_opening_in_a_stopping_process_fails`,
  `adr_0008_pid_take_over_refused_before_closing`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0008_an_editors_delete_is_recorded`,
  `adr_0008_pid_refused_in_an_editors_process`;
  [tests/security.rs](../../tests/security.rs):
  `adr_0008_timeout_rejects_once_else_cancels_the_turn`,
  `adr_0008_purge_deletes_only_that_sessions_files`;
  [src/render.rs](../../src/render.rs):
  `adr_0008_a_deletion_is_shown_with_who_deleted`;
  [src/paths.rs](../../src/paths.rs):
  `adr_0008_a_sessions_files_are_its_own_and_those_set_aside`;
  [src/log.rs](../../src/log.rs):
  `adr_0008_a_time_is_read_with_its_offset_and_written_in_utc`,
  `adr_0008_what_isnt_an_rfc3339_time_isnt_read`;
  [src/ctl/args.rs](../../src/ctl/args.rs):
  `adr_0008_a_flags_value_is_never_a_positional`,
  `adr_0008_every_flag_is_in_its_verbs_usage`,
  `adr_0008_positionals_and_unknown_options`,
  `adr_0008_a_prompt_is_a_flags_value`.
- 8.2: [tests/cli.rs](../../tests/cli.rs):
  `adr_0008_status_summarizes_the_session`,
  `adr_0008_stop_and_close_take_json`,
  `adr_0008_a_cancelled_turns_tool_calls_are_not_running`,
  `adr_0008_a_failed_turns_tool_calls_are_not_running`,
  `adr_0008_a_list_is_an_object`,
  `adr_0008_a_row_is_its_details_object_with_fewer_fields`,
  `adr_0008_dropped_is_a_list_of_messages_everywhere`,
  `adr_0008_a_count_is_named_so` (and, for `process list`'s `state` and
  `owner`, ADR 3's `adr_0003_ps_lists_the_processes` and
  `adr_0003_a_silent_process_keeps_its_session`);
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0008_the_agent_is_what_it_says_it_is` (#335),
  `adr_0008_a_turn_ending_in_a_replay_leaves_no_tool_call_running`,
  `adr_0008_a_failed_loads_history_leaves_no_tool_call_running`,
  `adr_0008_a_replays_call_with_a_finished_calls_id_is_not_running`;
  [src/ctl.rs](../../src/ctl.rs): `adr_0008_status_says_the_sessions_state`;
  [src/render.rs](../../src/render.rs):
  `adr_0008_text_shows_values_not_json`.
- 8.3: [tests/cli.rs](../../tests/cli.rs):
  `adr_0008_send_wait_prints_the_reply`,
  `adr_0008_start_wait_prints_the_reply_and_the_turns_result`,
  `adr_0008_send_wait_times_out`, `adr_0008_timeout_goes_with_wait`,
  `adr_0008_send_wait_reports_a_permission_request`,
  `adr_0008_wait_behind_exits_as_the_last_turn_ended`,
  `adr_0008_wait_returns_when_the_session_goes_idle`,
  `adr_0008_wait_on_an_idle_session_reports_the_last_turn`,
  `adr_0008_huge_timeouts_are_never`, `adr_0008_wait_for_permission`,
  `adr_0008_wait_for_exit`,
  `adr_0008_event_wait_prints_one_shape_for_each_condition`,
  `adr_0008_a_session_with_a_request_waiting_isnt_idle`,
  `adr_0008_a_usage_error_exits_2`,
  `adr_0008_help_exits_0_on_stdout`, `adr_0008_a_failed_operation_exits_1`;
  [src/ctl/talk.rs](../../src/ctl/talk.rs):
  `adr_0008_wait_for_exit_says_exited_only_once_the_process_has`,
  `adr_0008_a_count_the_status_doesnt_have_isnt_0`.
- 8.4: none: it is a promise that applies from 1.0.0.
- 8.5: [tests/cli.rs](../../tests/cli.rs):
  `adr_0008_run_is_the_command_line`: `brnr::run`, called in the test's
  process, ends with the exit status the binary exits with for the same
  words.
- 8.6: [tests/docs.rs](../../tests/docs.rs):
  `adr_0008_every_example_is_run_or_skipped_and_runs`,
  `adr_0008_every_toml_block_loads`; [tests/cli.rs](../../tests/cli.rs):
  `adr_0008_skill_prints_the_skill_and_its_references`,
  `adr_0008_skill_install_writes_it_for_claude_code_and_codex`.
- 8.7: [tests/cli.rs](../../tests/cli.rs):
  `adr_0008_a_failed_command_prints_its_error_as_json`,
  `adr_0008_no_error_is_reported_without_a_code`,
  `adr_0008_a_starting_process_refuses_another_session`,
  `adr_0008_a_failed_take_over_keeps_its_code`,
  `adr_0008_an_error_after_a_stream_is_on_stderr_alone`;
  [tests/docs.rs](../../tests/docs.rs):
  `adr_0008_every_error_code_is_listed_and_reached`;
  [src/error.rs](../../src/error.rs):
  `adr_0008_an_error_is_a_code_and_a_message`;
  [src/ctl/ownership.rs](../../src/ctl/ownership.rs):
  `adr_0008_a_refusal_has_its_places_code`. tests/common's `coded` checks
  every error in what each test's commands print and each record their
  processes write. A test asserts a code with `code_is`
  (tests/common's, on a command's output, its stderr, a reply, an event or
  a record; error.rs's on an `Error` in a unit test), and
  `adr_0008_every_error_code_is_listed_and_reached` counts a code as
  reached only where a test calls `code_is` with it written out: so each
  code is one some test has seen brnr report, or in its `UNREACHED`, with
  why. A code named only in a comment, a fixture or a request doesn't
  count.
