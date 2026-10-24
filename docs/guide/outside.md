# Talk to it from outside

```sh
brnr session list                  # brnr's sessions: open ones and ended ones (--include active: open only)
brnr session status $s             # what it's doing: turn, tools, plan, usage, last turn and message
brnr prompt send $s --prompt "also update the changelog"               # held while a turn runs
brnr prompt send $s --steer --prompt "and the tests too"               # into the running turn
brnr prompt send $s --interrupt --prompt "stop, wrong branch"
brnr prompt send $s --context --prompt "the API key is in .env.local"  # added to the next prompt; --replace: instead of the last
brnr prompt send $s --wait --prompt "what did you change?"             # prints the reply
brnr prompt send $s --file src/api.rs --image screenshot.png --prompt "why does this look wrong?"
brnr prompt cancel $s              # stop the running turn (held messages are dropped and listed)
brnr queue list $s                 # held messages and context; brnr queue show $s m3: one in full
brnr queue clear $s                # drop them (--messages, --context: only those); brnr queue drop $s m3: one
brnr event watch $s                # live: messages, tools, plan, approvals (--events all: everything)
brnr event log $s                  # the story so far; --last 2, --follow, and event watch's flags
```

A `<session>` is the session's id, as the agent gave it: `brnr session list`
shows them, with the agent's title for each, and `session new --json` prints
the new one, with its process's `pid` and the prompt's id as `message`
(`s=$(brnr session new --json … | jq -r .session)`). `event log` works on a session
that has ended too; commands that need it running say so when it isn't. On an
editor's session, those that act on it are [experimental](editor.md#experimental-actions).

The message is `--prompt`'s value, as for `session new`, taken as given:
`brnr prompt send $s --prompt -h` sends `-h`. `--prompt -` reads it from
stdin. A word that is no flag's is a usage error.

brnr never sends a second prompt while a turn runs: ACP doesn't say what one
means, and agents differ. `prompt send` holds the message and sends it as a
turn of its own when the running one ends, in order. `--steer` puts it into
the running turn instead (`_session/steering`, refused if the agent doesn't
offer it), and `--interrupt` cancels the turn and sends the message ahead of
what is held. On an idle session each is sent at once. `prompt send` says
which: `delivered`, `held`, `steered` or `interrupting`. Every message gets an
id (`m<n>`), which `prompt send --wait` uses to find the turn that answers it.
A turn that ends before the agent has answered the steers into it has its
`turn_ended` wait for those answers, so it lists every message the agent took
into it.

A message that never goes out is a `message_dropped` event, saying why:
`prompt cancel` (unless `--keep-held`), `queue drop` or `queue clear`,
its session closing, the agent exiting, or a steer the agent didn't take.
Context that never joins a prompt is a `context_dropped` event the same way:
`queue clear`, its session closing, or the agent exiting
(`prompt cancel` keeps it for the next prompt).

Every command that prints data takes `--json`, with the same data as its
text: one JSON value, or one event per line for `event log`, `event watch`
and `session new --foreground`. `session close --json` is `{session}`, and
`process stop --json` `{pid, status}`; `event notify`, whose output is its
command's, has none. One thing has one name and one shape in all of them
([ADR 8.2](../adr/0008-the-command-line-and-compatibility.md#82-same-data-in-text-and-json)):
a list is an object (`{"sessions": [...]}`, `{"processes": [...]}`,
`{"requests": [...]}`, `{"checks": [...]}`), its rows the detail's objects
with fewer fields; `dropped` is always the messages dropped; `agent` is
always `{program, name, version}`; a count is `<what>_count`.

## Waiting, for scripts

```sh
brnr event wait $s                 # until no turn is running and nothing is held
brnr event wait $s --for permission   # until an approval is waiting
brnr event wait $s --for elicitation  # until the agent asks for input
brnr event wait $s --for turn      # the next turn's end; --for exit: its process's
brnr session new --wait --stop-when-idle 0 --prompt "fix the failing tests" -- brnr-claude-adapter > answer.md
```

`event wait`, `prompt send --wait` and `session new --wait` exit 0 when the turn
ended normally (`end_turn`); 1 when it failed or stopped for another reason,
when its message was dropped (`brnr: m3 was dropped (cancel)`), when the agent
exited first, or when the session closed before the turn, approval or
elicitation waited for; 124 on `--timeout <s>`; and 3 when brnr cut the
command off for falling behind and the transcript can't say what it missed
(see [when a reader falls behind](events.md#when-a-reader-falls-behind)). `event wait`
on a session that is idle already, or that closes while it waits, exits as
its last turn ended. While `prompt send --wait` and `session new --wait`
wait, approvals and elicitations are announced on stderr; `session new --wait`
says `started …` there too, so stdout is the reply and nothing else. With `--json`, the turn is one object at its end:
`session`, `message`, `reply`, `stop_reason`, `error`, `tokens`, `dropped`
(and `pid`, for `session new` and `resume`). `event wait --json` prints one
shape for each `--for`, whether the wait ended at once or later: for `idle`
`{session, state, last_turn}`, for `turn` the `turn_ended` event, for
`permission` and `elicitation` the request as `permission show` and
`elicitation show` print it, for `exit` the `process_exited` event.

A command that fails exits 1 and says why on stderr, its error's code after
the message (`brnr: no session s (see brnr session list) [session_not_found]`);
with `--json` it prints the error as its JSON, `{"error": {"code": …,
"message": …}}`. A wait that got its turn prints the turn instead, a
failed one too (exit 1): its `error` says why. Scripts match the `code`, never the message:
[the interface reference](../reference/errors.md#codes) lists the codes, and
`unclassified` promises nothing. Where the agent's answer failed, a turn's
`error` too, the code is `agent_failed`, and `rpc_error` is the agent's
JSON-RPC error as it sent it.

# Settings

```sh
brnr config get $s [--model]       # the options, their values and choices, and the modes
brnr config set $s --mode plan     # the option of category mode (or the agent's modes)
brnr config set $s --model opus    # the option of category model
brnr config set $s --option effort=high   # any of the agent's config options, by id
brnr prompt commands $s            # the agent's slash commands (send them as text)
```

`--mode`, `--model` and `--thought-level` find the option of that category
(`mode`, `model`, `thought_level`), whatever its id, and `--option <o>=<v>` an
option by id; they go together, and `config get` takes the same, without
values, to list only those options; it lists only options the agent
advertised, so one it doesn't have fails. A mode, model or thought level the
agent has no option for fails ("the agent offers no thought level") before
anything is sent, as two values for one setting do
(`--model large --option llm=small`, when `llm` is the model); each is sent
once, the mode first. An `--option` id the agent hasn't advertised is sent
too, for the agent to take or refuse; when it refuses, the error says what
was already set. A mode is the agent's own `session/set_mode` only for an
agent with modes and no mode option. `config get` lists each option's choices
under it, the current one marked `*`, with their names and descriptions, and
has the agent's modes as a row with no option (`-`, `null` in `--json`).

A boolean option (one an editor's session has when the editor advertises
boolean config options) takes `true` or `false`, and is sent as a boolean;
anything else for it fails before it reaches the agent.

# Sessions and processes

```sh
brnr session fork $s               # a copy of the session, in the same process; status and list name its source
brnr session new --pid 4466 --prompt "review the diff"   # a session of its own, in a running process
brnr session resume --pid 4466 <id>                      # or one carried on there
brnr session close $s              # cancel a running turn, then close the session
brnr session delete $s             # the agent deletes its copy; brnr's transcript stays
brnr session delete $s --purge     # and brnr's transcript of it goes too
brnr session prune --older-than 2592000 --dry-run   # brnr's transcripts last active over 30 days ago; without --dry-run, deleted
brnr session list -- brnr-claude-adapter   # this folder's (--cwd), joined with the agent's own list
brnr session resume <id> -- brnr-claude-adapter   # any of them, even one brnr never saw
brnr session resume $s --take-over # one another process serves: closed there, resumed here
brnr process list                  # brnr's processes: pid, state, owner, agent, sessions
brnr process stop 4466             # stdin closed, then SIGTERM, then SIGKILL, 5 s apart
```

A process holds a lock for each session it serves
(`~/.brnr/sessions/<id>.lock`, beside the transcripts, so two processes that
write one transcript contend for one lock however each found its runtime
directory), so which process has a session is known without asking it. A
process that doesn't answer (within 5 seconds) still has its sessions:
`session list` and `process list` show them `unreachable` (`state`), commands on them
say the process isn't answering, and `session resume` refuses them, naming
the process, as it does any session that is running. A process started with
another runtime directory (`BRNR_DIR`; a cron job without `XDG_RUNTIME_DIR`,
say) can't be reached from this one: `session list` shows its sessions
`unreachable` too, and a refusal names its runtime directory and that
`BRNR_DIR` set to it reaches the process. A process that is gone, even one
whose pid another process has now (nobody listens on its socket), isn't shown
or counted as running, and its metadata and socket are removed. `--take-over`
asks the owner to close the session (an editor's process does only if its
profile enables `close`) and resumes it in a new headless process; sessions
beside it keep running.

A session whose lock can't be taken (`sessions/` isn't private, or a directory
is where its lock file goes) isn't served headless: `session new` fails with the
cause and the process stops before the agent gets any prompt, and a
`session fork` into it is refused. An editor's process serves it all the same,
as the editor would be served without brnr; `session status` says it isn't
locked, and `doctor` what is in the way.

`session close` ends one session; a headless process with none left stops.
`session fork` is refused in an editor's process, in strict mode, when the
agent can't fork, and with `stop_when_idle` when the agent can't close
sessions.

`session new --pid <pid>` opens a session in a running process, with its
agent, profile and MCP servers, in `--cwd` (or here), and
`session resume --pid <pid> <id>` resumes one there (in its recorded cwd),
taking its lock as any resume does, with `--take-over` as without `--pid`.
What starts a process (`--profile`, `--auth`, `--strict`, `--stop-when-idle`,
`--response-timeout`, `--foreground`, `--quiet`, `-- <agent>`) is an error
with it. The session, its settings (over the profile's) and its prompt commit
as one, as a start does, and the prompt goes only then. A setting that fails
after the agent opened the session closes it again, and the error says so (an
agent that can't close sessions keeps it, and the error names it); so does
the start timeout passing first, which the process keeps too. A process
stopping meanwhile fails it, and the session ends with the process. A command
gone first (Ctrl-C) leaves no session. A session being opened counts as one:
the process doesn't stop as its others close or go idle meanwhile. It is
refused where `session fork` is, but for strict mode:
`session/new` and `session/resume` are stable ACP. With `--take-over`, what
the process would refuse (it is still starting or stopping, or its agent
can't resume sessions) is refused before the session is closed where it
runs.

`brnr session list` on its own is brnr's index, with nothing started: every
session open in brnr's processes and every one it has a transcript of, in
every folder (`--cwd` for one). With an agent named (`-- <agent>` or
`--profile`), it shows only this folder's (or `--cwd`'s), starts the agent
just to ask it (`session/list`, every page) and joins the two on the session
id, so it includes sessions started outside brnr; an agent that can't list
fails it. A folder is the same however it is spelled (a trailing slash, a
symlink), and an `unreachable` session is a folder's if the agent lists it
there or brnr has its transcript there. SESSION, TITLE, STATE, PID, AGENT, SOURCE, LAST ACTIVE, FORKED FROM and CWD, in
text and `--json` alike, most recently active first: STATE is `idle`, `busy`
or `waiting` for one open in a process, `unreachable` for one whose process
holds it but doesn't answer, `inactive` for one no process has open; SOURCE
says who knows it, `brnr`, `agent` or `both`; FORKED FROM is the session a
fork came from (`forked_from`), also once it has ended, as its transcript
records it; TITLE and LAST ACTIVE are the agent's where it gives them. LAST ACTIVE is in UTC, an agent's converted
from its offset to brnr's own form (`2026-10-11T06:00:00.000000Z` in
`--json`, `2026-10-11 06:00:00Z` in text), and the order is by the time it
names; one that isn't an RFC 3339 time is shown as the agent gave it, said
on stderr, after the sessions with a time and before those with none. `--include active` keeps the sessions open in a
process, `--include inactive` the others; both by default.

`session resume` takes an id brnr knows, or any id the agent knows, which it
resumes in `--cwd` (or here) with the agent after `--` (or the profile's).

`session delete` is `session/delete`, asked of the agent brnr recorded for
the session (or `--profile` or `-- <agent>`, for a session brnr has no
transcript of), started for that as `session list` starts one. A session
open in a process is refused (close it first), as is an agent that can't
delete sessions (`sessionCapabilities.delete`). Only the agent's copy goes:
brnr's transcript stays, still listed by `session list`, with a
`session_deleted` in it, a record of no process's (its `host_id` null).
`--purge` deletes the transcript too, the session's events file and raw ACP
in each project folder under `BRNR_HOME`, an older brnr's too, with those set
aside beside them (`<id>.older.jsonl`, …), and nothing else: the host logs
are shared and stay, and does so whatever the agent answered. With
`--purge`, the command exits 0 only if the agent deleted the session or
doesn't have it (ACP's `resource_not_found`, said on stderr); any other
error, or no answer, is said and exits non-zero: the agent may still have
the session. Only `resource_not_found` counts as not having it, and the
adapters, at the versions brnr pins
([adapters/package.json](https://github.com/brnrhq/brnr/blob/main/adapters/package.json)), differ:
`brnr-codex-adapter` (codex-acp) answers a session it doesn't have as
deleted, so `--purge` succeeds, but `brnr-claude-adapter` (claude-agent-acp)
answers with another error (`-32603`, internal error), so `--purge` of a
session it has already deleted exits non-zero, brnr's transcript deleted all
the same. A file that can't be deleted, or have `session_deleted` recorded
in it, is said (`not deleted: …`, `session_deleted isn't recorded: …`) and
exits non-zero, the others done all the same; a transcript made private
first is said too on stderr (`path_made_private: <path> (its mode was …)`): there
is no host log for it. An editor's `session/delete` through `brnr acp` goes
to the agent as it is; once the agent has deleted the session, the editor's
process records `session_deleted` too, and closes the session if it had it
open.

`brnr process stop` signals the agent's whole process group, so whatever the
agent started in that group goes with it. The same cleanup runs when the agent
exits or crashes on its own. A SIGKILL of the brnr host itself bypasses
cleanup: agents and bridges that ignore their closed pipes, or their
descendants, can survive it; `doctor` reports the unrecorded death.
