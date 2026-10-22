# 8. The command line and compatibility

Draft 2026-10-20. Implemented, but 8.4 is a promise that applies from
1.0.0, 8.5's typed facade waits for #150, and 8.6's evals with a real agent
aren't done. Factor: F5 (ADR 1). Brings together former ADRs 63, 15, 34,
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
  `agent`, `source`, `last_active`, `forked_from`, `cwd` in `--json`).
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
  it), the session it was forked from (`forked_from`, ADR 3.12), and what
  the agent says it is (`agentInfo`). The control socket's `status` stays
  the full report, which bridges use; brnr shapes what it prints from it.
- Running tools are the running turn's (#164). A tool call the agent left
  unfinished, in a turn that was cancelled or failed, isn't running once
  the turn has ended; nor is one from a load's replayed history that ended
  or failed, nor a replayed call with a finished call's id. ACP has no
  status for such a call, so brnr gives it none.
- Text shows values, not JSON.
- Every command that prints data takes `--json`: one JSON value, or one
  event per line for `event log`, `event watch` and
  `session new --foreground`. Errors stay on stderr, with the same exit
  status. `process stop` and `session close` take it too, though their
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

**Why.** Scripts and agents read `--json`, people read text (F5). If the two
carried different data, brnr would have two interpretations of one session
where P5 allows one, and a script could not see what a person sees.

**Considered.**

- `session list --json` as every process's whole status report, while the
  text showed six columns.
- `session status` as a summary in text and the raw report in `--json`.
- `--json` on a few commands only.

## 8.3 Waiting, and exit status

- `brnr event wait <session> [--for idle|turn|permission|elicitation|exit]
  [--timeout <s>]`:
  - `idle` (the default): no turn running and nothing held. It returns at
    once if the session is idle already, so `prompt send` followed by
    `event wait` can't race. A closed session counts as idle.
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
  reported for the turn, or null (ADR 4.4).

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
  `.acp.jsonl`), and their records. A 1.x brnr reads what an earlier 1.x
  wrote (`event log`, `session list`, `session resume`). What each record
  promises in detail is #70's.
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
runtime directory is, not as what is in it. Stable error codes (#205) are
covered once they land.

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

The promise starts at 1.0.0. Before 1.0.0-rc.1, #150 (`--json` shapes)
still settles names and shapes, breaking as P9 allows. #151 (event and
transcript record names) is decided and made (ADR 5.9, #314), as is #153
(the exit code of a usage error, `--help`: ADR 8.3, #310), and #154 (flags
and config keys spelled two ways: ADR 8.1 and 6.1, #311). What they settle
is what 1.0.0 promises.

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
- Scripts match exit codes and `--json` fields, not messages. Until stable
  error codes (#205) land, there is nothing finer to match.
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
  major. Stable error codes (#205) let the wording change freely.
- Covering what host logs contain: they are what a process records for
  itself and for `doctor`, not for others to read; the session's story is
  in its events file (ADR 5.1).
- A support period for older majors now: there is only one.
- Keeping the library public: every internal change would be a semver
  change (ADR 8.5).

## 8.5 The library is private, and its facade mirrors the command line

Implemented, but for the typed facade, which waits for #150: `brnr::run` is
the library's one public item.

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
  An error carries the stable code the command's error has (#205) once
  those land; its message, like the command's, is not covered. It isn't
  built yet: its return types are the `--json` shapes, which #150 makes
  one name and one shape per concept. It follows #150, by a record of its
  own if it departs from this shape.
- The crate's own tests and fuzz targets reach internals through
  `brnr::internals`, a `#[doc(hidden)]` module behind the `internals` Cargo
  feature, off by default. It re-exports what they use (`frame`'s reading
  and writing, `json`'s parsing and spans, `schema`'s types, `log::rfc3339`
  and `host::fuzz`'s harness), and nothing more. The integration tests
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
  `adr_0008_a_failed_turns_tool_calls_are_not_running`;
  [tests/headless.rs](../../tests/headless.rs):
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
  `adr_0008_wait_for_exit`, `adr_0008_a_usage_error_exits_2`,
  `adr_0008_help_exits_0_on_stdout`, `adr_0008_a_failed_operation_exits_1`.
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
