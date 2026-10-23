# Events

`event watch`, `event log`, `event notify`, the foreground and bridges all
show the same events, live or read back from the transcript:

| Event | |
|---|---|
| `user_message` | a message sent to the agent, `by` the `editor` or `control`, with its id |
| `agent_message`, `agent_thought` | the agent's text, whole |
| `tool_call` | a tool call starting (`started`) and ending (`completed`, `failed`) |
| `tool_progress` | a tool call's status changing in between (`in_progress`) |
| `plan`, `usage` | the plan; the context window and cost, as the agent reports them |
| `session_changed` | the title, mode, config options or commands: what changed |
| `permission_request`, `permission_resolved` | an approval waiting; its answer (`allowed`, `rejected` or `cancelled`), the chosen option's kind, and who gave it |
| `elicitation_request`, `elicitation_resolved`, `elicitation_completed` | the agent asking for input: its `text`, `mode`, `scope` (with its `tool_call_id` or `scope_request_id`), a form's `schema` or a `url`; the `action` sent (`accept`, with its `content`, `decline` or `cancel`) and who sent it; the agent saying a URL's flow is done |
| `turn_ended` | its `stop_reason` or `error` (`agent_failed`, with the agent's as `rpc_error`), the `tokens` the agent reported for it, and the `messages` it carried |
| `message_dropped` | a message that never went, its `reason` `cancel`, `queue`, `close`, `exit` or `steer` |
| `context_dropped` | context that never joined a prompt, its `reason` `queue`, `close` or `exit` |
| `session_closed` | `by` `control`, its `reason` `close` (`brnr session close`, `--take-over`) or `idle`; or `by` `editor`, `reason` `close` |
| `session_reopened` | an editor's load the agent answered with success after it answered the session's close: the session is open again, `by` `editor`, and the load's `history` follows. None if another process took the session meanwhile, which the editor's process then doesn't serve (ADR 3.9) |
| `session_deleted` | the agent deleted the session, `by` `control` (`brnr session delete`) or the `editor`; brnr's transcript stays |
| `history` | a load's replay: how many `updates`, whether they were `recorded` (as events with `replayed`), how many of them are (`recorded_updates`: fewer only when the session closed while its load was unanswered), and over how many `loads` (more than 1 only for an editor's loads that overlap) |
| `line_too_long` | a line `from` the `agent` or `editor` past the `limit` (32 MiB), unread: `relayed` to the other side with an editor, dropped headless |
| `line_too_deep` | a line `from` the `agent` or `editor` nested past `max_depth` (128 levels), unread and unrecorded: `relayed` to the other side with an editor, dropped headless |
| `peer_cut_off` | a `peer` (a watcher, a bridge) cut off for falling behind, and the last event it was sent (`last_queued_seq`) |
| `process_exited` | the agent exited, and the process with it: its `exit_status`, and an `error` when brnr itself crashed |
| `acp` | every ACP message, with its direction |

Each has `ts`, `host_id` (the process's run) and `pid` (the process), and
names its session; `process_exited`, `line_too_long` and `line_too_deep` are
the process's, and are in every session's transcript; `peer_cut_off` is the process's too,
in its host log. Each has its position, `seq`: one more than the last
event's or ACP message's of its process, live and in the transcript. Who
acted is `by`: `editor`, `agent` or `control` (brnr's commands, bridges and
timers); beside it, `peer` is the connection that asked, where brnr knows
one (`socket#7`, a bridge's name), and `reason` is why, a word
(`permission_resolved` by `control`: `peer` for `brnr permission allow`,
`reason` `timeout` or `cancel` otherwise; by `agent` when the agent gave the
request up with `$/cancel_request`). Every name is snake_case (ADR
5.9). For config options and commands, `session_changed` is a JSON merge
patch by id or name: `{"model": "opus"}`, a command added, `null` for one gone.

A turn's `tokens` are the counts the agent put in its answer to the prompt
(`usage`, unstable in ACP): `{"total", "input", "output", "thought",
"cached_read", "cached_write"}`, each the agent's number, or `null` where it
gave none that is a count (a JSON integer that fits a `u64`; not `1e3`), and
`tokens` itself `null` when the answer has no `usage` object. brnr adds
nothing up and doesn't say what they cover: whether subagents or earlier model
calls are in them is the agent's to say. `session status` has the last turn's,
in `last_turn` (ADR 4.4).

`--events` chooses, for `event watch`, `event log` and `event notify` alike:
names, `all`, and `default` (`--events default,agent_thought` adds thoughts).
It takes the names of brnr's two notes too, `records_skipped` (a gap in a
transcript) and `events_missed` (what a command cut off couldn't catch up
on), which are shown whatever it chose: `--events records_skipped` alone
shows only a transcript's gaps. A bridge's `events` and `subscribe` take
events only; a note's name fails a profile's bridge, as the process never
sends one. `subscribe` leaves out an event the process doesn't have (an
older brnr's), and names it in its answer.
Without it, `event watch`, `event log` and the foreground leave out the quiet
ones, `acp`, `agent_thought`, `usage` and `tool_progress`, in text and JSON
alike. Bridges get every event but `acp` unless they choose. Watching a
session ends when it closes.

# Notifications

`brnr event notify` runs a command for each event of a session, or of every
session in a process (`--pid`): by default `permission_request`,
`elicitation_request`, `turn_ended` and `process_exited`; `--events` as for `event
watch`, where `default` means these four. The event is in its environment (`BRNR_EVENT`, `BRNR_TEXT`,
`BRNR_TITLE`, `BRNR_MESSAGE`, `BRNR_SESSION`, `BRNR_REQUEST`, `BRNR_PID`)
and, as JSON, on its stdin; nothing is put on its command line, so what the
agent writes can't become arguments. The text, title and message are escaped
as `event watch` shows them and cut at 32 KiB; the event on stdin is whole.
Commands run one at a time, in order, each in a process group of its own.
`event notify` exits when the process does (or the session closes). Cut off
for falling behind, it lets the command finish, then catches up, running the
command for each event it missed, as `event watch` does; unable to, it says
what it missed and exits 3. Its connection ending otherwise first, or
stopped by a signal, it stops the command it is running, says on stderr that
no more notifications come, and exits non-zero.

```sh
brnr event notify $s -- sh -c 'curl -s -d "$BRNR_TEXT" ntfy.sh/my-agents'
```

In a profile it is a bridge, for every process the profile starts, and reads
the events the process writes on its stdin:

```toml
[[profiles.work.bridges]]
command = "brnr"
args = ["event", "notify", "--stdin", "--",
        "sh", "-c", "terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\""]
```

If the bridge's `events` limit what it gets, they must include
`agent_message`, `session_changed` and `process_exited`, which `event notify` needs
for its environment and to know when to stop. What it says on stderr, that it
was cut off included, is in the host log (`bridge_stderr`). A started bridge
doesn't catch up: its stdin is closed, and it is sent SIGTERM. For
notifications that never miss a turn, run `brnr event notify <session>` on
the socket instead.

# When a reader falls behind

Nothing is buffered without limit, and who gives way depends on who is
reading. The session's own pipes get backpressure, as a direct pipe would give
them: an editor that stops reading holds the agent back on its stdout for as
long as it does, with no timeout of brnr's own (and observers see nothing new
meanwhile), and an agent that stops reading its stdin holds back the editor's
writes. A signal to `brnr acp`, or its stdout closing, doesn't wait behind
them: it goes to the brnr process on a link of its own, and reaches the agent
at once. Observers never slow the session: a bridge or watcher 16 MiB behind
is cut off (a connection closed, a started bridge sent SIGTERM), not counting
the one line, however long, that took it past 16 MiB, nor (up to 64 MiB of
them) lines longer than 16 MiB that came after it, so a reply bigger than
that reaches every peer that keeps up, and the
foreground's display skips events. Nor does brnr's own record: past 64 MiB
waiting for a slow disk, records are skipped, and
[the transcript](transcripts.md#transcripts) says so.

A burst bigger still (a thought and a message each over 16 MiB, to a
watcher that asked for `acp`) can cut off a peer that keeps up. brnr's
commands catch up: `event watch`, `event notify`, `event wait` and the
`--wait`s, cut off, ask the process whether it cut them off
(`was_cut_off`), subscribe again, and read what they missed from the
transcript, by the events' positions, before going on live; a peer cut off
is a `peer_cut_off` event to the others. Where the transcript can't have
it all (`log = "off"`, `log = "events"` for `acp`, records skipped), or
when cut off 4 times in a row while catching up, the command says what it
missed, an `events_missed` note (`after_seq`, `until_seq`, `error`), and exits 3.
Started bridges are cut off as before (ADR 10.2).

However long the lines: the session's pipes hold back past 16 MiB queued,
and brnr reads a line of up to 32 MiB, held beside that while it waits for
its newline. A longer one goes on as it comes,
unread and unrecorded, to the editor or the agent unchanged, or is dropped
headless, and a `line_too_long` event says so. The agent's stderr
goes on as it is written, newline or not, 64 KiB at most at a time, every
byte. A request on the control socket, or on a started bridge's stdout, is
read up to 32 MiB too: a longer one is answered with an error, the host log
has `peer_dropped` (`reason` `line_too_long`), and the peer is cut off.

However many clients: a process serves 128 connections at once, and tells
the next one so (`too many connections: 128 open`) and closes it. brnr's
commands ask every process for its status at once, so processes that don't
answer cost about 5 s in all, not 5 s each; each request then waits 5 s for
its answer (120 s for one the agent answers, as `config set` is). One with
no answer by then, or whose write the process doesn't take, fails with `not
answering`; an error the process says to no request (no `req_id`), before
it closes the connection, is the waiting command's error. Waiting for events isn't such a request: `event wait`
and `--wait` wait as long as `--timeout` says, or the session runs, and a
start (`session new`, `--pid`) as long as its own timeout. But a command
that catches up waits 5 s for the process to have written its transcript
(a stalled disk), and gives up on catching up after that, saying so
(ADR 10.4, ADR 10.2).

However deep the lines: brnr reads a line nested up to 128 levels (`[[1]]`
is 2), about what serde_json, and so ACP's Rust SDK, reads. A deeper one is
treated as a long one: it goes on unread and unrecorded with an editor, is
dropped headless, and a `line_too_deep` event says so; brnr tracks nothing
of it, so a permission request that deep isn't listed or answerable from
outside. brnr's commands read what brnr itself wrote (answers, events,
transcripts) up to 256 levels, and say so, and fail, past that (ADR 2.5).
