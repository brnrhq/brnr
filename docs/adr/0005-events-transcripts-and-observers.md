# 5. Events, transcripts and observers

Draft 2026-10-20. Implemented. Factor: F3, F5 (ADR 1). Brings together former ADRs 22, 55, 20, 23, 24, 48, 57, 35 and 36
(docs/adr/README.md maps them).

What a brnr process says about its sessions, and what it keeps: the events
the host emits, the transcripts it writes, and what observes them, `event
log`, `event watch`, `event notify` and bridges. ACP has none of these;
they are what brnr adds beyond it (F3). They behave as a Unix utility's
output should: an event chosen is shown, in text and JSON alike, and
nothing goes without a word (F5).

## 5.1 The host's events are the story, and transcripts are two files

Events:

- Every event the host emits to bridges (`user_message`,
  `agent_message`, `tool_call`, `permission_request`, `turn_ended`, …;
  `EVENTS` in src/host/control.rs) is also a record in the session's
  events file. `event log`, `event watch` and the foreground (ADR 3.5)
  render the same events, and a transcript read back is the stream a
  bridge saw live (P5).
- `process_exited`, the one event that belongs to the process, is written into
  every session's events file too, so `event log` keeps to one session's
  files.
- `session_changed` records what changed, not the whole list again. Of
  the config options and the commands it has a JSON merge patch
  (RFC 7396) by id and name: an option's new value (`{"model": "opus"}`),
  a command added (`{"review": {…}}`), `null` for what is gone. An answer
  to `set_config_option` that changes a value makes one too, as an agent
  needn't send an update for it.
- The `session_opened` record of a session the process opened as a fork
  (`session fork`'s, or an editor's `session/fork` through `brnr acp`),
  in its events file and in the host log, has `forked_from`, the session
  it was forked from (ADR 3.12). A session opened any other way has none:
  the process doesn't know whether it was a fork, and the first
  `session_opened` in the file says.
- `session delete` writes `session_deleted` into the session's events
  file itself, a record of no process's (`host_id` null), and with
  `--purge` deletes both files (ADR 8.1).
- Every record starts with its `format` (ADR 5.10), then carries `ts`,
  `host_id`, `pid` (the brnr process), `proxy_pid`, `agent_pid` and, in a
  session's files, `session`, for joining. Every event, and every raw ACP record, has `seq`, its position
  in its process (ADR 10.2), and every event `pid` too (ADR 5.9).
- A session's events file starts with `session_opened`, the same record as
  the host log's: `session`, `cwd`, `profile` and `agent` (the process's,
  ADR 3.11), `events_log`, `acp_log` (null with `log = "events"`),
  `host_log`, and `forked_from` for a fork. The host log names each
  session's files there.

Files:

- Each session has two files, keyed by project folder like the agents'
  own: `~/.brnr/projects/<folder>/<session id>.jsonl`, its events, and
  `<session id>.acp.jsonl` beside it, the raw ACP. `<folder>` is the cwd
  with every non-alphanumeric character replaced by `-` (as in
  `~/.claude/projects`), then `-` and a hash of the cwd, so each cwd has a
  folder of its own. The session id in the names is escaped, one name per
  id, so distinct sessions never share a transcript (ADR 9.2).
- One session id is one session, whatever folders its files are in
  (#121). A session resumed in
  another cwd (`session resume --cwd`) has files in each folder, and its
  transcript is all of them: brnr reads them as one (below), and a load
  finds it in any folder (ADR 5.6).
- Each process also has a host log, `~/.brnr/hosts/<run id>.jsonl`, for
  what belongs to no session (ACP before a session exists, the agent's
  stderr).
- The events file is always written: `event log`, `session list` and
  `session resume` read it. The raw file is written by default; a
  profile's `log = "events"` leaves it out, and `log = "off"` writes
  neither (ADR 6.1).
- A transcript is written only where only the user can read it: what
  others can reach is made private first, and a symlink is refused
  (ADR 9.3, P13).
- A process holds a session's two files only while it serves the session.
  A close closes them once its last record, `session_closed`, is written;
  a session opened again appends to them, after another `session_opened`.
- Records the logger skips behind a slow disk (ADR 10.1) leave a gap that
  a `records_skipped` note counts, in the session's events file and the
  host log. So do records a write fails to put in a file: the note goes in
  the file that lost them once it takes records again (for the raw file,
  in its events file).
- A load of a session brnr has no transcript of records the replay
  (ADR 5.6).

Reading:

- `event log` reads a record as the event `event watch` would have shown.
  With `acp` chosen it reads the raw file too, merged in order, each ACP
  message an `acp` event, and says so when there is none. `--events` and
  `--json` mean the same as for `event watch` (ADR 5.3), and `event log
  --json` is what a watcher got live. It shows a `records_skipped` gap
  whatever `--events` chose.
- A record in a format this brnr doesn't read, an older brnr's or a newer
  one's, isn't read (ADR 5.10, P9). A file of them is no transcript:
  `session list` and `session resume` pass it over, and a load records the
  history (ADR 5.6). Nothing goes without a word (P3): `event log` of it
  says `<file>: records an older brnr wrote, before transcripts had a
  format, not read (ADR 5.10)`, and `doctor` lists every such file,
  transcripts and host logs, as info.
- `event log` of a running session first waits for what its process has
  recorded (ADR 5.5).
- `event log` reads every file of the session, in every project folder,
  merged by their records' `ts` (#121), each file's records in the order
  it has them. One process owns a session at a time (ADR 3.9), and a record's
  `ts` is when its process recorded it, not when it was written, so the
  records of two files don't overlap in time. A closing process's last
  records can be written after the next process's, past ADR 5.5's 2 s;
  merged by `ts`, they still come before them. `--last` counts in what is
  merged; `--follow` follows the files the serving process writes. With
  `acp` chosen, each folder's raw file is merged with its events file,
  and a folder without one is said (`no raw ACP for <session> beside
  <file>`). A file of the session in another format, in any folder, is
  said and not read (above). Of a session open in a process that logs
  nothing (`log = "off"`), what the other folders hold is shown, and
  `event log` says the process keeps no transcript of it.
- What a transcript is for `session list`, `session resume` and `doctor`
  comes from the first and last records brnr can read in it. A line that
  isn't a JSON object, cut short or not, is passed over. The last record
  is read backwards, so a long transcript costs no more than its last
  records. Of a session with files in more than one folder, the first
  record is the oldest file's and the last is the newest record of them
  all: `session resume` takes where it was last served from that newest
  record (ADR 3.11), and `session list` its row, one per id (#121). That
  record is the newest a process recorded that isn't a deletion: a
  deletion isn't activity (ADR 8.1), neither `session delete`'s, which is
  no process's, nor an editor's, which its process records into every file
  of the session, opened there or not. Its cwd, profile and agent are those
  the newest `session_opened` of them all names (ADR 5.10), in whichever
  file it is. One process serves the session at a time (ADR 3.9), so that
  is the opening of the process that last served it; the newest record may
  be in another file, as an editor's deletion is recorded in every file of
  the session by a process that may never have opened it (ADR 8.1). A file
  whose newest record is in a format brnr doesn't read is passed over (ADR
  5.10). A
  line cut short is never taken for a record: a JSON object ends only at
  its last byte. So, for a transcript:
  - ending in a newline, as written: its last line is the last record;
  - ending partway through a line: its last whole record;
  - with a whole line that isn't a record, last or anywhere: the line is
    passed over;
  - empty, or with no line brnr can read: no session, as nothing says
    which, where or when (`event log` says `no session`).
- `event log` shows every record it can read, and says on stderr each
  line it can't (`brnr: <file>:<line> isn't a record brnr can read, not
  shown`). Once the transcript won't grow (its process has gone), it also
  says when the file ends partway through a line (`brnr: <file> ends
  partway through a record (<n> bytes), not shown`). Of a running
  session, what follows the last newline is a line still being written,
  and isn't mentioned. The exit status is still 0: the transcript asked
  for is shown (P3).
- A process that opens a transcript to append to it, and finds it ending
  partway through a line, starts its first record on a line of its own.
  The line cut short stays as it was.
- Nothing that reads a transcript writes to it: brnr never repairs,
  truncates or rewrites one (P3). Nor does it read one where the logger
  couldn't have written it: through a symlink, in a symlinked project
  folder, or what isn't a regular file (ADR 9.3).

**Why.** The raw ACP needs the host's interpretation to read as a history
(agent messages assembled, permission requests paired with their answers,
a turn's end named), and the CLI can't know the host's names for things
(permission handles like `p1`, message ids). Recording the events the host
already emits gives one story, live and read back (P5). The raw file is
what checks that interpretation. On the maintainer's machine in October
2026, 44 session files held 31 MB: 74% raw ACP, almost all of it the
agent's `session/update`s; `session_changed` was 5.5 MB in 176 records,
each repeating whole lists, hence the patch. A process killed mid-write
(SIGKILL, a power cut, a full disk) leaves its last line cut short, and
the logger ends a line its own failed write cut short before it writes
again, so a broken line can be anywhere in a file. Reading past it keeps
the session listed, logged and resumed, as every record before the cut is
there (#61).

A session's files in two folders are one session read as half of one:
`event log` and `session resume` take the most recent file, and say
nothing of the other (#121). An id is one session (ADR 9.2), so its
transcript is every file of it. Their records don't interleave in time,
since one process owns the session at a time (ADR 3.9), and `ts`, the
time a record was recorded, orders them, even those a closing process
writes late (ADR 5.5). The newest record is where the session was last
served, a fixed rule that doesn't depend on where the command runs (P4).

**Consequences.**

- Turning the raw file off saves most of the space, and leaves secrets
  (ADR 9.1) nothing to be redacted from but the host log.
- brnr doesn't replay a session to an editor. An editor that resumes a
  session with `session/resume`, which replays nothing (and ACP v2 has no
  `session/load`), shows what the agent gives it. Resending the agent's
  old `session/update`s from the raw file would make up for what the agent
  doesn't do (F1), and is beyond what a focused tool does (F5, N4). The raw
  file is for checking the interpretation.
- A process that forks and closes session after session keeps the files
  of each session it has open.
- A session killed mid-write is listed, logged and resumed as of its last
  whole record. The bytes of the cut record stay in the file, and
  `event log` points at them. A file whose lines are all cut short or
  broken is not a session; its bytes are still there to read.

**Considered.**

- One file with raw ACP and events together.
- Events only, raw ACP on request: a quarter of the space, but checking
  the interpretation, and any replay, would need it turned on in advance.
- Raw ACP only, the events derived when read: a second interpretation in
  the CLI, without the host's names.
- `event log` also reading the host log for the session's time span: two
  files to merge and to follow, and the host log has ACP and stderr that
  aren't the session's.
- Keeping a record dump for `event log`, with `--events` over the records:
  `--raw` would have kept two meanings, one for `event log` and another
  for `event watch`.
- Host-wide events copied into every session's file: the case that
  prompted it, `owner_changed`, went with ADR 3.1; `process_exited` is what
  remains.
- Truncating a file to its last newline when it is read or reopened:
  repairs it, but destroys what was there, and a reader would write.
- Failing `event log` on a line it can't read: the records around it are
  what the user asked for, and stay readable.
- Taking the session's id from the file name when no record can be read:
  the name is an escaped id (ADR 9.2), not the id, and there would be no
  cwd, agent or profile to resume it with.
- For one id with files in two folders (#121): keeping "the most recent
  file wins", which shows half a session and says nothing (P3); preferring
  the file in the current cwd, which makes the answer depend on where the
  command runs (P4); refusing and listing the folders, with `--cwd` to
  choose one, which treats one session as two.

## 5.2 Dropped messages and closed sessions are events

- `message_dropped {session, message, text, reason}`, `reason` one of `cancel`,
  `queue` (`queue drop`, `queue clear`), `close`, `exit` and `steer` (a
  steer the agent neither took nor sent back, ADR 4.2): every way a
  message brnr accepted can go unsent (P3). Unanswered steers are dropped
  with held messages by `prompt cancel`, a close and the exit. `process_exited`
  has no list of messages not delivered (P5, P9).
- `context_dropped {session, text, reason}`, `reason` one of `queue` (`queue clear
  --context`), `close` and `exit`: context held for a next prompt that
  won't come, one event per item (context has no message id, ADR 4.1).
  Context held when its session closes (`stop_when_idle` included) or the
  process exits is dropped so. `prompt cancel` keeps context: it is for
  the next prompt, whenever that comes.
- `session_closed {session, by, reason}`: `by` `control` with `reason`
  `close` (`session close`, `session resume --take-over`) or `idle`
  (`stop_when_idle`, ADR 3.8), or `by` `editor` with `reason` `close` (the
  editor's own `session/close`). Messages and context still
  held in the session are dropped first. A session whose load the agent
  failed (ADR 5.6), or one the agent opened that isn't the process's to
  serve (ADR 3.9), drops what it holds the same way, `reason` `close`, with
  no `session_closed` after: it never opened here. A process exiting is
  `process_exited`, which is in every session's file already, and isn't also
  `session_closed`.
- `prompt send --wait` and `session new --wait` whose message is dropped
  exit 1, saying `m3 was dropped (cancel)` on stderr and
  `dropped: "cancel"` in `--json`. `event wait` ends as ADR 8.3 says;
  `event watch` and `event notify` on a session end when it closes.
- All three are in `EVENTS` and the README's list for bridges, and in the
  default events of `event watch`, `event log` and bridges (ADR 5.3).
  `event notify`'s default is its own (ADR 5.8).

**Why.** A message brnr accepted and never sent, or a session that closed,
must be said where observers look (P3). Without these events a
`prompt send --wait` for a dropped message waited until the process
exited, and `event watch`, `event notify` and `event wait` on a closed
session waited for the whole process.

**Considered.**

- `turn_ended` with a `dropped` stop reason: misleading, since no turn ran.
- Only the response to the command that dropped them saying so.
- A distinct exit status for "never ran": one more code for scripts to
  learn; the difference is in stderr and `--json`.
- An `undelivered` list in `process_exited`: two kinds of record for one thing.

## 5.3 Choosing events, and what text shows

- `--events` alone chooses, for `event watch`, `event log` and `event
  notify` alike: event names, `all`, and `default`. An event chosen is
  shown, in text and JSON alike (P5). Unknown names fail in the CLI,
  before connecting, with the same message everywhere.
- Without `--events`, `event watch` and `event log` show every event but
  the quiet ones, `acp`, `agent_thought`, `usage` and `tool_progress`
  (`QUIET`), in text, JSON and the foreground alike. `default` names that
  set, so `--events default,agent_thought` adds thoughts. There is no
  `--raw` and no `--thoughts`.
- Whatever `--events` chose, `event log` also shows a gap in the
  transcript (`records_skipped`, ADR 5.1), and a command cut off what it
  missed (`events_missed`, ADR 10.2): notes, not events (ADR 5.9).
  `--events` takes their names too, so `--events records_skipped` alone
  shows a transcript's gaps.
- Bridges and the host's `subscribe` have their own default: every event
  but `acp`. `event notify`'s `default` is its own (ADR 5.8).
- Tool calls: `tool_call` when a call starts (`started: true`) and when it
  ends (`completed` or `failed`), shown as `tool: … (kind)`,
  `tool done: …`, `tool failed: …`; `tool_progress` for each change of
  status in between (`in_progress`), quiet, shown as
  `tool: … in_progress` when chosen. Progress that doesn't change the
  status (streamed output) is not an event. A `tool_call` again with the
  id of a call that is running is that call, not a second one (#164): it
  updates it, as a `tool_call_update` would, with no second start.
- `usage` (one per `usage_update` the agent sends; `session status` has
  the latest) is quiet. It has the context window and the cost, not
  tokens in and out, and its line, when chosen, is
  `usage: 12.3k of 200.0k tokens, cost 0.42 USD`.
- `turn_ended` has `tokens`, the tokens the agent reported for the turn
  (ADR 4.4), and its line shows each count given: `turn ended: end_turn
  (control); tokens: 1.2k total, 1.0k input, 234 output`.
- `session_changed` is shown for everything it reports: title, mode,
  config (`config: model=opus`) and commands (`commands: +review
  -compact`, those added or changed and those gone, as ADR 5.1's patch has
  them).

**Why.** One raw line per streamed chunk is not readable. A flag with two
meanings (`--raw` for `log` and for `watch`), and a second way to choose
an event that is already chosen (`--thoughts`), are two answers to one
question. Text that leaves out events JSON has breaks P5: `--events usage`
printed nothing in text while `--json` had them.

**Considered.**

- `event watch` showing everything by default, `acp` included: the
  firehose should be one flag away, not the default.
- A text line for every tool status change: noise.
- No event for status changes between start and end: bridges would lose
  `in_progress`.
- Text quietly leaving out events that JSON has: what `--events` chose
  wouldn't be what was shown.

## 5.4 `event log --last <n>`

`--last <n>` counts messages sent to the agent (`user_message` events),
and shows from the n-th last one on. `--last 0` shows nothing (with
`--follow`, only what comes next).

**Why.** What is wanted is the latest exchanges, and a turn still running
is the one most likely wanted.

**Considered.**

- Counting turns (`turn_ended`): it leaves out a turn still running.

## 5.5 The log of a running session waits for its transcript

- A request, `logged`, is answered once the process's logger has written
  everything the process recorded before it was asked, with the note of a
  gap of skipped records (ADR 10.1) that ends there. It says `lost` when a
  file has lost records it hasn't noted yet (ADR 10.2). The answer comes
  from the logger's thread; the event loop doesn't wait for it (P1). With
  `log = "off"` it is answered at once.
- `event log` of a running session sends it first, and reads the files
  once it is answered: what `session new --wait`, `prompt send --wait` or
  `event wait` has reported is in what it shows. A process that doesn't
  answer within 5 s (a stalled disk) is shown as far as it has written,
  and `event log` says so on its stderr (P3). `--follow` waits the same
  way before its first read.
- A process that exits waits, before brnr stops listing it and it lets
  go of its sessions, until its logger has written what it
  recorded, `process_exited` last, for 2 s at most. Once it is gone, `event log`,
  `session list` and `session resume` read its sessions' transcripts
  whole, and a resume doesn't start appending to one before its `process_exited`.
  A panic lets go of the process's sessions as an exit does, before it
  waits.
- Then, unlisted and holding no session, it waits for the rest without a
  time limit, as for the display's reader in the foreground (ADR 3.5),
  and exits once it is written. A signal to it (Ctrl-C, `kill`) gives up
  on it: the process says on its stderr how much it didn't write
  (`brnr: process 4466: 1234 bytes of records not written: the disk
  didn't take them`), and exits as it would have.
- A process that closes a session (`session close`, `session resume
  --take-over`, its idle time running out, the editor) holds its lock,
  and answers the close, until its logger has written what it recorded of
  the session, `session_closed` last, for 2 s at most, as on exit. So a
  resume in another process appends after it, and a load there finds its
  transcript (ADR 5.6). The event loop doesn't wait for it (P1). Past 2 s
  (a stalled disk) the session is let go of all the same, and its last
  records can come after the next process's, as they can after an exit.
  Opened in the same process again meanwhile, it keeps its lock there,
  and the close is still answered once its records are written.
- A session whose open the agent fails, after the process recorded
  something of it, is held the same way until that is written: a load's
  replay (ADR 5.6), or the close's records, when the open took back the
  lock of a session closing there. A bridge's open (`session resume
  --pid`) is answered with its error once they are; a start that fails
  lets go of the session with its exit's records, at the latest. A fork
  into a session the process couldn't lock has nothing to let go of: it
  took no lock, and opened no transcript.
- Until then the process's `status` lists the session in `closing`, so
  that brnr can tell it from one the process is opening. `event log` of it
  sends `logged` to that process and reads the transcript once it is
  answered, `session_closed` in it, as for a running session.
  `session resume` and `session delete` of it fail, saying it is closing
  in that process and free once its records are written, 2 s at most.
- What a process killed with SIGKILL hadn't written is lost: `session
  list` can lack the session, and `session resume` of it without an agent
  fails with `no agent: give one after -- or set agent in the profile`
  (ADR 3.11). That can't be helped without writing on the session's own
  thread.

**Why.** A process's records are written by a thread of their own
(src/log.rs), so the session never waits for the disk (P1, ADR 10.1).
Read as the files were, a transcript could lack what a command had just
seen: `session new --wait` printed the turn's reply, and `event log` right
after didn't have the turn, or failed because the events file wasn't there
yet, and nothing said so (P3). On CI's Linux runners about 1 in 15 such
waits returned before the turn was written. A process that exited also
stopped being listed, and let go of its sessions, before `process_exited` was
written.

**Consequences.**

- `event log` of a running session costs a round trip to its process, and
  up to 5 s when the disk is stalled.
- Tests that read a transcript, or kill a process whose transcript they
  need, wait for it the same way, by running `brnr event log`.
- A headless process whose disk stalls stays after it stopped being
  listed, holding no session and no agent, only its queue (64 MiB at
  most, ADR 10.1), until the disk takes it or a signal ends it (`kill`).
  Ended so, its host log can lack `process_exited`, and `brnr doctor` lists it as
  a death nothing recorded (ADR 3.7).

**Considered.**

- Answering `session new --wait` and `prompt send --wait` only once their
  turn is written: every waiting command would wait for the disk, and a
  stalled one would stall them, which ADR 10.1 rules out for observers.
- Writing records on the event loop: the session would wait for the disk
  (P1, ADR 10.1).
- Waiting in the tests only (for a record to appear in the file): every
  user's script that runs `event log` after `--wait` would have the same
  race.
- A time limit on exit (10 s, 30 s): a headless process would drop
  records with nobody told, its stderr unwatched and its host log, the
  record, on the stalled disk (P3), to free a process that by then holds
  nothing. A limit the user can't see is a guess (P4); the user's signal
  is the explicit choice.

## 5.6 A load's history is recorded once

- `session resume` tells the process whether brnr has a transcript of the
  session: one brnr's index lists as `inactive`, or one another process
  is serving (`--take-over`). That is the one thing brnr knows about the
  history it has; it doesn't compare the replay with the transcript to
  guess what is missing (P4).
- "brnr has a transcript of the session" means in any project folder,
  whatever cwd the session is resumed in, for a start's load as for an
  editor's and a bridge's (below): an id is one session, and its files
  in every folder are its transcript (ADR 5.1, #121).
- A transcript is an events file holding a record of the session beyond
  its openings. One holding only `session_opened` records, as a failed
  load leaves, isn't one: the next load, a start's, a bridge's or an
  editor's, records the history. brnr reads its own file for this when the
  load is asked for, and deletes nothing; `session list` still lists the
  file, which names the session's cwd and agent.
- With a transcript, the replay is neither recorded, raw or as events, nor
  shown; what it says the session is now (title, mode, config options,
  commands) is kept. Loading the session again and again adds nothing to
  the transcript but each load's `history` event.
- Without one, the replay is taken in as live updates would be: recorded
  in the raw file, and made into events (`agent_message`, `tool_call`,
  `session_changed`, …), each with `"replayed": true`. The user's
  messages, which live are the prompts brnr sent, come only from the
  replay here, as `user_message` events with `replayed` and without `by`
  or a message id. Text shows them as `(replayed) user: …`. A tool call
  the history left unfinished isn't counted as running (`session status`).
  The next load finds the transcript, and doesn't record the history
  again.
- Either way, once the agent answers the load, a `history` event says how
  many `updates` the agent replayed and whether they were `recorded`:
  `history: 3 updates replayed by the agent, recorded`, or `…, not
  recorded: brnr's transcript has the session`. Its `loads` is 1, but for
  an editor's loads that overlap (below). `recorded` says whether the
  replay was taken into the transcript at all; `recorded_updates`, always
  there, how many of the `updates` are in it: all of them when
  `recorded`, none when not, but for a session closed while its load was
  unanswered (below), where text says `history: 4 updates replayed by the
  agent, 3 recorded`.
- `session/resume` replays nothing, and makes no `history` event.
- An editor's `session/load` through `brnr acp` follows the same rule: its
  replay is recorded only when brnr has no transcript of the session, in
  any folder, and a `history` event says either way. The editor gets the
  replay as the agent sent it (ADR 3.1): only what brnr records and emits
  follows the rule.
- A bridge's load (`session resume --pid`, ADR 8.1) follows it too, and
  looks in any folder, whatever the cwd it is resumed in.
- A session a process has served, open or closed since, has its
  transcript in that process, for an editor's load and a bridge's alike,
  whether or not the logger, which writes behind (ADR 5.5), has written it
  yet, unless the process logs nothing (`log = "off"`). The process
  remembers each session it has handed the logger a record of beyond its
  openings, for its events file, as the disk has it once that is written.
  So loading it again there records nothing of the replay again.
- A load that failed after the agent replayed so leaves a transcript of
  what it recorded, which is all the replay, the agent message it ended
  with too (P3). One that failed before the agent replayed, or a session
  closed with all its records, its close's too, skipped by a disk too slow
  for them (ADR 10.1), leaves only openings, and the next load, in that
  process too, records the history.
- The editor's loads of a session that overlap (one sent before the agent
  has answered the one before) all reach the agent, and share one replay,
  as which load an update is for can't be told (P4). Until the last of
  them is answered, every update either replays is taken as the replay
  is: recorded and marked `replayed`, or not, as the first load found the
  transcript. Then one `history` event says how many `updates` they
  replayed between them, whether they were `recorded`, and how many
  `loads` it covers: `history: 10 updates replayed by the agent in 2
  loads, recorded`. A recorded replay can so hold the history once per
  load, each copy marked; nothing is left out (P3). A load that fails
  counts as answered. If none of them succeeds, there is no `history`
  event, as for a single load that fails, and a session they opened
  closes again, unless a `session/resume` of it succeeded meanwhile.
- The editor's `session/close` of a session whose load is unanswered
  closes it once the agent answers the close, with its `session_closed`,
  as any close does. If the agent then answers the load with success, its
  last answer wins: the session is open in the editor's process again,
  and a `session_reopened`, `by` `editor`, says so (`session reopened
  (editor): its load was answered after its close`). brnr takes the
  agent's last answer as how the session is, and doesn't guess otherwise
  (P4).
- The load's replay is kept through the close: what the agent replays
  meanwhile is counted, and goes to the host log, as the session's files
  are closed. Once the load succeeds, the replay goes on in the reopened
  session, and its `history`, after the `session_reopened`, has all the
  `updates` and, in `recorded_updates`, those the transcript has. A load
  that fails after the close leaves the session closed, with no
  `history`, as any failed load. Of loads that overlap, one that
  succeeded before the close has its `history` once the last is answered,
  with the session still closed, in the host log. A load of the session
  sent after the close, while one before it is unanswered, joins that
  replay, as overlapping loads do.

**Why.** An agent without `session/resume` resumes a session with
`session/load`, replaying its history as `session/update`s before it
answers. Leaving the whole replay out assumes brnr's transcript has the
history. That fails for a session brnr has never seen (`session list` with
an agent named lists the agent's own, and `session resume` takes any of
them, ADR 3.11), and for one whose transcript is gone: `event log` then
has the load's request and answer, none of the conversation, and nothing
says it is missing (P3, #60).

**Consequences.**

- The transcript of a session first loaded by brnr starts with its
  history, as the agent replayed it. Those records are as new as the
  load, and carry this process's ids.
- brnr's transcript is complete only for what happened through brnr:
  turns taken in the agent's own client, or while a profile had `log =
  "off"` (ADR 6.1), or records a slow disk skipped (ADR 5.1), aren't in
  it, and a load of a session brnr has a transcript of doesn't add them.
  The `history` event's `recorded: false` says that the replay was left
  out, so that what is missing is known to be missing.
- A session resumed in another cwd (`session resume --cwd`) starts a file
  in that folder, and its load records nothing of the replay there: the
  history is in the file in the first folder. `event log` reads both
  (ADR 5.1), so it shows the history, and the `history` event says the
  replay wasn't recorded.
- Deleting a session's transcript makes its next load record the history
  again, except in a process that served the session before the delete:
  that process remembers what it recorded of the session until it exits.
- A record the logger was handed and the disk then failed to take (a full
  disk) still counts in its process, which can't tell: until it exits, a
  load there records nothing of the history, where another process's
  would. The failed write is noted in the file (P3).

**Considered.**

- Always recording the replay, marked: every load of a session would add
  its whole history to the transcript again.
- Recording only what the transcript lacks, matching the replay against
  it: ACP's message ids are optional, and brnr's own records of a turn (an
  injected message's echo, a steer) aren't what an agent replays. A match
  would be a guess (P4).
- Recording the raw replay only, without events: `event log` reads events,
  so the history still wouldn't be in it.
- Not creating the events file until the load succeeds (#191): the agent
  replays before it answers, so the load's raw ACP and its replay would be
  held in memory until then (P6), or written elsewhere and moved; and the
  files failed loads have left already would still keep their history
  out.
- Removing, or marking, the file a failed load leaves: removing it deletes
  the raw record of the failed load (P5), and either way the files already
  left stay as they are.

## 5.7 Bridges

- A bridge is any process that speaks brnr's JSON-lines protocol.
  Requests (`command` in src/host/control.rs): `status`, `send`,
  `cancel`, `queue` (with `show`), `subscribe`, `pending` (answered with
  `pending` and `elicitations`, ADR 4.5), `allow` and `reject`
  `{session, request, always?, option_id?}`, `elicitation` `{session,
  request, action, content?}` (ADR 4.5), `set_config` (taking `mode`,
  `model`, `thought_level` and `options`, ADR 6.2), `new` and `resume` (a
  session in the running process, as `session new --pid` and `session
  resume --pid`), `fork`, `close`, `stop`, `logged` (ADR 5.5) and
  `was_cut_off` (ADR 10.2). Those about a session name it by its exact id.
  A `cmd` the process doesn't have, or a field of one, is refused,
  `command_unknown` (ADR 8.8).
  Events: as ADR 5.1, 5.2 and 5.3 list them, each naming its session;
  `process_exited` is the process's. `subscribe` and a profile bridge's
  `events` take events only: a note's name is refused, and fails the
  profile (ADR 5.9). `subscribe` leaves out an event the process doesn't
  have, and names it in `unknown_events`, so that a newer brnr's bridge
  gets the events an older process has; a profile that names one fails
  (ADR 8.8).
- A request line that isn't valid UTF-8 is answered with an error, and the
  bridge stays (#15).
- A started bridge's stderr goes to the host log (`bridge_stderr`, its
  `peer` the bridge's label, as `bridge_started` and `bridge_exited` name
  it), a line
  at a time, decoded lossily: a line that isn't valid UTF-8 doesn't stop
  the reading, which would leave the bridge's next write to die of SIGPIPE
  (#160). A line longer than 64 KiB goes in pieces of that size, as the
  agent's stderr does (ADR 10.3).
- Two transports: a connection to the process's control socket
  (`$BRNR_DIR/<pid>.sock`, which only the user can open, P13), or a child
  the process starts from the profile's `bridges`, whose stdout carries
  its requests and whose stdin carries events.
- A started bridge is subscribed from birth (every event but `acp`, or the
  bridge's `events` in the profile), so it sees everything from the
  process's start. Its environment has `BRNR_PID` and `BRNR_SOCKET`: a
  process's sessions come and go, and it has none when its bridges start.
- A started bridge ends when its process exits. Its stdout closing only
  means it has no more requests; it keeps getting events. When the brnr
  process stops, the bridge gets the last events (`process_exited`), its stdin
  closes (it should exit then), and one still running 2 s later
  (`BRIDGE_EXIT`) gets SIGTERM: time for a bridge to act on `process_exited`, as
  `event notify` does.
- A bridge has to keep reading: one 16 MiB behind is cut off (ADR 10.1).
- On an editor's session, a bridge's actions are experimental, as the
  CLI's are (ADR 7.1). `stop` isn't one: stopping the process is process
  management, never experimental and not refused in strict mode.
- From 1.0.0 the compatibility promise covers the protocol as a started
  bridge speaks it, on its stdin and stdout. The control socket is
  private, in the runtime directory, and not an interface: a client that
  connects to it directly has no promise (ADR 8.4).

**Why.** A bridge connects brnr's sessions to something else: Slack, push
notifications, a phone (F3). What another tool can do stays out of brnr
(F5), so brnr gives a protocol, not integrations. A bridge that only
listens must keep getting events: one that ran `exec cat > file`, and so
closed its stdout, was dropped in 4 ms.

**Considered.**

- A started bridge ending when its stdout closes: one that only listens
  would have to keep its stdout open.
- Started bridges getting no events on stdin until they subscribe: a
  change to the protocol, and they would miss the process's start.

## 5.8 `event notify`

- `brnr event notify (<session> | --pid <pid>) [--events …] -- <command>
  [args...]` runs the command once per event. The default events are
  `permission_request`, `elicitation_request`, `turn_ended` and `process_exited`;
  `--events` works as for `event watch` (ADR 5.3), where `default` means
  these four.
- The event is in the command's environment (`BRNR_EVENT`, `BRNR_TEXT`,
  `BRNR_TITLE`, `BRNR_MESSAGE` with the session's last agent message,
  `BRNR_SESSION`, `BRNR_REQUEST`, a permission request's or an
  elicitation's handle, `BRNR_PID`) and, whole, as JSON on its stdin.
  Nothing goes on its command line, so the agent's text can't become
  arguments (P8). The text, title and message are escaped as `event
  watch` shows them and cut at 32 KiB, so the command always starts.
- Commands run one at a time, in order, each waited for, in a process
  group of its own, with their stdout sent to stderr (as a bridge,
  `event notify`'s stdout is read by the process as requests). No event
  is read while one runs, so a command that hangs makes `event notify`
  fall behind, and the process cuts it off like any slow peer (ADR 10.1):
  a started bridge gets SIGTERM, a connection is closed. It exits when the
  process does, and on a session also when that session closes
  (`session_closed`).
- On the socket, cut off while its command runs, it lets the command
  finish, then catches up from the transcript and runs the command for
  each event it missed; one that can't catch up says what it missed and
  exits 3 (ADR 10.2).
- Cut off as a started bridge (SIGTERM, or its stdin ending before
  `process_exited`), its connection ending before `process_exited` otherwise (the process
  killed), or stopped by SIGHUP, SIGINT or SIGQUIT, `event notify` stops
  the command it is running (SIGTERM to its process group, then SIGKILL
  once it has exited, or 2 s later). It says on its stderr that it was
  cut off and that no more notifications come (`brnr: cut off (SIGTERM);
  stopped sh (turn_ended); no more notifications`), and exits non-zero
  (P3); `--stdin` cut off exits 1. A started bridge's stderr is in the
  host log (`bridge_stderr`). On the socket it watches for the connection
  closing while a command runs, and reads what was sent before it closed,
  no more than the socket held, to tell the process's end (`process_exited` among
  it) from being cut off.
- `brnr event notify --stdin [--events …] -- <command>` reads its events
  from its stdin, a started bridge's transport (ADR 5.7), instead of
  connecting. That is how it runs as a bridge: `command = "brnr"`, `args =
  ["event", "notify", "--stdin", "--", …]` in the profile, with no `sh -c`
  and no `$BRNR_PID`, and one subscription. A started bridge isn't caught
  up when cut off; one that must not miss events runs `brnr event notify
  <session> -- …` on the socket instead (ADR 10.2).

**Why.** Notifications are what most people want from a bridge: a message
when an approval is waiting or a turn has ended. One command that runs any
command serves every service, and keeps them out of brnr (F5). Run as
`sh -c 'exec brnr event notify --pid "$BRNR_PID" -- …'`, it subscribed
on the socket and never read its own stdin, where the process writes the
same events from birth: after 16 MiB the process cut it off, and the
notifications stopped without a word (P3).

**Considered.**

- Shipping scripts for a few services: one command that runs any command
  covers them all.
- Running commands concurrently: it would reorder notifications.
- `event notify` draining its stdin while subscribed on the socket: two
  subscriptions for one bridge.
- `event notify` reading stdin when it detects it's a bridge: a guess
  (P4); the flag says it.
- Never reading events from stdin, so that `event notify` behaves the same
  as a bridge and by hand: with an explicit `--stdin` the difference is
  asked for, not hidden, and it is the only option with one subscription
  per bridge.
- The process writing to a started bridge's stdin only once it
  subscribes: ADR 5.7.

## 5.9 Events, records and their fields follow one naming scheme

Resolves #151; implemented by #314. #150's `--json` shapes, #205's error
codes and #70's transcript contract take their names from it.

Names:

- Every name brnr makes in its JSON is snake_case, lowercase ASCII words
  joined by `_`: the names of events and records (their `event`), their
  fields, the values of a field with a fixed list (`by`, `reason`,
  `state`, `status`, `owner`), and #205's error codes. Kebab-case is the
  command line's, for commands and flags. A handle's value is the handle
  (`m3`, `socket#7`).
- Config is TOML, and its names are ADR 6.1's, which follow ADR 8.1 for
  #154: snake_case keys, `log`'s words `all`, `events` and `off`, and a
  command as `command` and `args`, a bridge's as an MCP server's.
- What brnr passes on from ACP keeps ACP's spelling (P1): the message in an
  `acp` event (`sessionId`), the ACP objects an event carries whole
  (`tool_call` in `permission_request`, `options`, `outcome`, `schema`),
  and ACP's values (`end_turn`, `allow_once`, `in_progress`). An `acp`
  event's `dir` is two parties joined by `->` (`editor->agent`); the
  agent's stderr, which has no second party, is `agent_stderr`.
- An event or a record is named subject first:
  - for what was said or asked, by the thing: `agent_message`,
    `tool_call`, `plan`, `usage`, `permission_request`, `history`, `acp`;
  - for what happened, or became true, by its subject and a past
    participle or a predicate: `turn_ended`, `session_closed`,
    `peer_cut_off`, `line_too_long`; a failure is `<subject>_failed`, with
    an `error`.

  Never a bare verb (`exited`, `send`) or the present tense (`stopping`).
  The subject is the noun the command line and the ADRs use for the
  thing, one noun for one thing: `process` for the brnr process (`host`
  stays only in `host_id` and `host_log`, its run and its file),
  `session`, `turn`, `message`, `context`, `peer`, `bridge`, `editor`,
  `agent`, … It is plural only where one record stands for several
  (`records_skipped`, `events_missed`, `capabilities_dropped`).

Kinds:

- Every name in an `event` field is in one namespace, of three kinds, told
  apart by the list a name is in, not by how it is spelled:
  - events (`EVENTS` in src/host/control.rs): what the host emits to
    bridges and records in transcripts (ADR 5.1);
  - notes (`NOTES` beside it): what brnr adds to a stream of events and
    the host never emits: `records_skipped`, the logger's, in transcripts,
    and `events_missed`, a command's, on its stdout (ADR 10.2);
  - records: the other lines of the events files and the host log, for
    brnr's own reading (`session_opened`, `process_started`, …).
- `--events` takes events and notes, so every name `event log --json` or
  `event watch --json` can print is one it takes, and `--events
  records_skipped` alone lists a transcript's gaps. A note is still shown
  whatever `--events` chose (P3). `subscribe` and a bridge's `events` take
  events only, and refuse a note (P7): the host never sends one. An event
  the process doesn't have, `subscribe` leaves out and names (ADR 8.8).
- Every line `event log --json` and `event watch --json` print has
  `event`, `ts`, `host_id` and `pid`; a session's also has `session`, and
  an event its `seq`. The logger writes them into its notes, as the host
  does into its events. `pid` is new on events: the brnr process's, as
  `--pid`, `BRNR_PID` and every `--json` have it; `host_id` stays, to tell
  runs apart, as pids are reused.
- Host-log records follow the scheme too, though ADR 8.4 doesn't cover
  them: the host log holds events as well, and `doctor` and the catch-up
  (ADR 10.2) read it.

Fields:

- A field means one thing, and has one type, wherever it is: a word isn't
  an object somewhere else, an id isn't an object, text isn't a word. A
  name is a count or a list, never both: a count of what is listed
  elsewhere is `<what>_count` (`acp_count`, `held_count`), and `dropped`
  is a list everywhere (ADR 8.2, #150).
- A thing the commands take as an argument is named by the bare noun, and
  holds its handle: `session`, `message`, `request`, `pid` (the brnr
  process's) and `peer` (a connection's label, a started bridge's too).
  Any other id is `<thing>_id`: `host_id` (the run), `tool_call_id`,
  `elicitation_id`, `option_id` (a permission option's, as the socket's
  `allow` and `reject` take it, ADR 8.1), `scope_request_id`, `rpc_id` (a
  JSON-RPC message's `id`). Any other process's pid is `<whose>_pid`
  (`agent_pid`, `proxy_pid`, `bridge_pid`).
- `by` is who acted: `editor`, `agent` or `control` (brnr's commands, its
  bridges, and its own timers). Beside it, `peer` is the connection that
  asked, where brnr knows one. `reason` is why, a word from the event's own
  list. Text for people is an error's `message` (ADR 8.7), never `reason`.
  `by: agent` says the agent ended its own request with
  `$/cancel_request`: its `permission_resolved` or `elicitation_resolved`
  has `peer` and `reason` null (ADR 4.5, #308).
- A position is `seq`, or ends in `_seq` (`first_seq`, `last_queued_seq`).
  A time is an RFC 3339 time in UTC to the microsecond: `ts` is the
  record's own, any other is named for its moment (`since`, `until`,
  `last_active`). A duration is in whole seconds, its name with no unit,
  as ADR 8.1 decides for #154: `uptime`, `turn_elapsed`, `timeout_left`,
  `stop_when_idle`. A size is in bytes by the same rule (`line_too_long`'s
  `limit`). A depth is in levels of nesting, and its name says so
  (`line_too_deep`'s `max_depth`), as it is no size.
- A file brnr writes is `<which>_log` (`events_log`, `acp_log`,
  `host_log`); any other file or directory is `path`. `log` alone is the
  profile's setting (ADR 6.1), never a file.
- A boolean is an adjective or a past participle (`recorded`, `relayed`,
  `answerable`), with no `is_`.

What takes its names from this:

- #205: an error code is snake_case, subject first, as a record's name is:
  `session_not_found`, `session_closing`, `config_invalid`. Its kebab-case
  examples followed the records' names, which this replaces. An agent's
  JSON-RPC error is ACP's, its `code` a number (F1); ADR 8.7 has it whole
  as `rpc_error`, beside brnr's code, `agent_failed`.
- #150: `--json` fields follow these rules: `session`, never `session_id`;
  ids, `by`, `reason`, positions, times and files as above. ADR 8.2
  settles each concept's shape.
- #70: the record heads and the records named here are the names its
  transcript contract holds to; ADR 5.10 decides what each record
  promises, and how a format is marked.
- A name added later, of an event, a note, a record, a field or a value,
  is made by these rules: #229's subagent fields, #308's `by: agent`.

The renames below are all of them, so that #314 is mechanical: each is
made outright, with no alias (P9), and is **Breaking:** in the changelog.
Where ADR 8.1 or 6.1 (#154) names a thing, this follows it, and its
renames are #311's, not in these tables: the durations in `--json` and the
socket's replies (`uptime_seconds`, `turn_seconds`, `timeout_seconds`),
`allow`'s and `reject`'s `option`, and config's `log` and `command`.

### Renames

Events (`EVENTS`). The other 24 keep their names.

| Old | New | |
|---|---|---|
| `exited` | `process_exited` | the one bare verb; it is the process's (ADR 5.1) |

Notes (`NOTES`), new to `--events`:

| Old | New |
|---|---|
| `records-skipped` | `records_skipped` |
| `events-missed` | `events_missed` |

Records in a session's events file (and, for some, the host log):

| Old | New |
|---|---|
| `session-opened` | `session_opened` |
| `send` (`mode` `prompt`, `steer`, `interrupt`) | `message_accepted` |
| `send` (`mode` `context`) | `context_accepted` |
| `cancel` | `cancel_requested` |
| `open-abandoned` | `open_abandoned` |
| `idle-timeout` | `idle_timed_out` |
| `context-attached` | `context_attached` |
| `editor-response-dropped` | `editor_response_dropped` |
| `steer-refused` | `steer_refused` |
| `steer-after-turn` | `steer_after_turn` |

Records in the host log only (not covered, ADR 8.4):

| Old | New |
|---|---|
| `started` | `process_started` |
| `stopping` | `stop_requested` |
| `signal` | `signal_relayed` |
| `host-signal` | `signal_received` |
| `panic` | `process_panicked` |
| `exited` (a start abandoned) | `process_exited` |
| `host-stderr` | `process_stderr` |
| `start-abandoned` | `start_abandoned` |
| `start-failed` | `start_failed` |
| `test-ready-held` | `test_ready_held` |
| `drop-held-failed` | `drop_held_failed` |
| `display-stopped` | `display_stopped` |
| `bridge-started` | `bridge_started` |
| `bridge-stderr` | `bridge_stderr` |
| `bridge-exited` | `bridge_exited` |
| `editor-closed-stdin` | `editor_stdin_closed` |
| `editor-stopped-reading` | `editor_stopped_reading` |
| `editor-disconnected` | `editor_disconnected` |
| `editor-cancel-dropped` | `editor_cancel_dropped` |
| `editor-request-refused` | `editor_request_refused` |
| `not-sent-to-editor` | `editor_message_dropped` |
| `agent-stdout-ended` | `agent_stdout_ended` |
| `capabilities-dropped` | `capabilities_dropped` |
| `lock-failed` | `lock_failed` |
| `session-shared` | `session_shared` |
| `open-failed` | `open_failed` |
| `session-log-failed` | `session_log_failed` |
| `made-private` | `path_made_private` |
| `peer-dropped` | `peer_dropped` |
| `timeout-failed` | `timeout_failed` |

Fields and values, by the new names:

| Where | Old | New |
|---|---|---|
| every record's head | `session_id`, `host_pid` | `session`, `pid` |
| every event | — | `pid` |
| `message_dropped`, `context_dropped` | `by` | `reason`, its values as they were |
| `session_closed` | `by`: `close`, `idle`, `editor` | `by: control, reason: close`; `by: control, reason: idle`; `by: editor, reason: close` |
| `session_deleted` | `by: delete` | `by: control` |
| `permission_resolved`, `elicitation_resolved` | `by`: a peer's label, `timeout`, `cancel` | `by: control, peer: <label>`; `by: control, reason: timeout`; `by: control, reason: cancel`; `peer` and `reason` null where none |
| `process_exited` | `status`, `reason` | `exit_status`, `error` |
| `peer_cut_off`, `peer_dropped`, the `was_cut_off` reply | `last_queued` | `last_queued_seq` |
| `elicitation_request`, `pending`'s `elicitations`, `elicitation show` and `elicitation requests --json` | `message`, `tool_call` (an id), `scope_request` | `text`, `tool_call_id`, `scope_request_id` |
| `records_skipped` | `acp` | `acp_count`; it gains `ts`, `host_id`, `pid`, and `session` in a session's file |
| `events_missed` | `after`, `until` | `after_seq`, `until_seq`; it gains `pid` |
| `session_opened` | in the events file `cwd`, `host_log`, `acp_log`; in the host log `session_id`, `file`, `acp_file` | in both: `session`, `cwd`, `events_log`, `acp_log`, `host_log`, and `forked_from` for a fork |
| `cancel_requested` (a timeout's) | `by: timeout` | `by: control, reason: timeout` |
| `context_attached` | `to: editor-prompt` | `to: editor_prompt` |
| `steer_refused` | `answer` (the agent's response) | `response` |
| `editor_response_dropped`, `editor_request_refused`, `editor_message_dropped` | `id` | `rpc_id` |
| `editor_cancel_dropped` | `request_id` | `rpc_id` |
| `editor_request_refused`, `editor_message_dropped`, `lock_failed`, `session_shared`, `open_failed`, `session_log_failed` | `session_id` | `session` |
| `editor_message_dropped` | `reason: "the agent's stdout has ended"` | `reason: agent_stdout_ended` |
| `peer_dropped` | `reason: "fell behind"` | `reason: fell_behind` |
| `capabilities_dropped` | `dropped` | `capabilities` |
| `session_log_failed` | `file` | `path` |
| `process_started` | `request` (the start's) | `start_request` |
| `bridge_started`, `bridge_stderr`, `bridge_exited` | `bridge` | `peer` |
| `bridge_started` | `pid` | `bridge_pid` |
| `bridge_exited` | `status` (a code or null) | `exit_status`, as `process_exited`'s: `{code}`, `{signal}` or null |
| `acp`'s `dir`, a raw record's `dir` | `agent-stderr` | `agent_stderr` |
| the bridge `status` reply's `sessions` | `session_id`, `log` | `session`, `events_log` |
| `event notify`'s environment | `BRNR_SESSION_ID` | `BRNR_SESSION` |
| `permission requests --json`'s `options` | `{option, kind}` | ACP's options whole, `{optionId, name, kind}`, as `permission show` has them (ADR 8.2) |

`BRNR_EVENT`, `--events`, a bridge's `events`, and `event wait`'s and
`event notify`'s endings use the new names.

**Why.** At b41a8c5 the 24 events are snake_case, and 35 record names are
kebab-case and 6 are one bare word, in the same `event` field and often the
same file. So:

- `event log --json` prints `records-skipped`, which `--events` refuses,
  and `event watch --json` ends with `events-missed`.
- Events name the session `session`; the record heads, the host-log
  records, the bridge `status` reply and `event notify` (`BRNR_SESSION_ID`)
  say `session_id`. Events carry `host_id` but no `pid`, which every other
  `--json` names the process by.
- `by` is a party in `user_message` and `turn_ended` (`editor`, `control`),
  a cause in `message_dropped` (`cancel`, `queue`, `steer`), both in
  `session_closed` (`idle`, `editor`), and a peer's label or a timer in
  `permission_resolved` (`socket#7`, `timeout`).
- `until` is a time in `records-skipped` and a position in
  `events-missed`; `tool_call` is ACP's object in `permission_request` and
  an id in `elicitation_request`; `message` is a handle (`m3`) in
  `message_dropped` and the agent's text in `elicitation_request`;
  `status` is a word in replies and an object in `exited`; `reason` is
  text in `exited` and `peer-dropped` and a word nowhere.
- `session-opened` names a session's files `host_log` and `acp_log` in the
  events file, `file` and `acp_file` in the host log, and the `status`
  reply names one `log`.

A script that reads `event log --json`, or a bridge, has to know two
spellings, and which field means what in which event (F5, P4). One stream
is one vocabulary (P5), and a rule a reader can learn once is fewer
concepts (N4). From 1.0.0 events, transcripts and `--json` are covered
(ADR 8.4), and every one of these would wait for 2.0; before it, a rename
is made outright (P9). snake_case, because the events, every field and
ACP's values already are, and because jq, Python and Rust read it as a name
without quoting (in jq `.records-skipped` is a subtraction).

**Consequences.**

- Every bridge, `event notify` command and script that reads events
  changes once, before 1.0.0. The changelog says how, entry by entry.
- A transcript an older brnr wrote keeps its names, and brnr doesn't read
  them (P9), so `session list` and `session resume` pass it over. Nothing
  is lost silently (P3): `doctor` lists it, and `event log` of it says an
  older brnr wrote it. ADR 5.10 marks a transcript's format, so that is a
  check of the format, not of a field:
  - A record is an older brnr's by its head: it has no `format`
    (`transcript::readable`), whatever its fields. Such a record isn't
    read anywhere, so a file of them is no transcript to a load either,
    which records the history (ADR 5.6). A file is listed as one by its
    first record, or its newest.
  - `doctor` lists them as info under `file formats`, transcripts and
    host logs alike, and passes such a host log over where it looks for
    a death nothing recorded (ADR 3.7) or for the report's logs (ADR
    11.5): read in this brnr's names, every one would lack
    `process_exited`. A line of one in the report would be `(a record in
    a format this brnr doesn't read, not shown)`: what an older brnr
    didn't redact isn't shown either.
  - A process doesn't append to an older brnr's transcript, nor to any
    file in a format it doesn't read (ADR 5.10): ADR 9.2's rule that went
    on in a file of the folder before the hash goes, as every such file is
    an older brnr's, and what was appended would be read without what came
    before it. No writer does (`log::appends_to`, the one test of it, is
    the check `doctor`'s `file formats` makes: a file whose first or
    newest record is in another format): a session opened where such a
    file is in the way (one in its cwd's folder, written there before
    #314, or a resume of a session brnr has no transcript of) renames that
    file beside it, `<name>.older.jsonl` (`older-2`, … if taken; the raw
    file `<name>.older.acp.jsonl`), and starts a file of its own, and the
    host log's `session_log_moved` (`session`, `path`, `to`) says so (P3);
    `doctor` still lists the renamed file. An event for a session no
    process has open (`session delete`, an editor's deletion) passes such
    a file over. A newer brnr's file is set aside the same, under the same
    name: `older` says it is no file of this brnr's to go on with. Renamed
    rather than left with the session's records in the host log, as a
    refused file is (ADR 9.3): the session then has a transcript, which
    its next resume finds, and a load records its history in it (ADR 5.6).
    `session delete --purge` deletes the files set aside too, as every
    file of the session's id (ADR 8.1, N1).
- The CLI reads the bridge `status` reply's `sessions` in the new names,
  and `session list`'s rows of a transcript the same (`session`,
  `events_log`).
- `subscribe`'s answer has the process's `pid`, beside `host_id`, so that
  a command's `events_missed` has it as every line does.
- Text shows who acted as before: the `peer` if there is one, else `by`
  if it isn't `control`, else the `reason` (`permission p1 allowed …, by
  socket#7`, `session closed (idle)`, `session closed (editor)`).
- A bridge's `events` naming a note, which a profile may have now as
  `records-skipped`, fails to load.

**Considered.**

- kebab-case for everything, as the records and #205's proposal have it:
  the events, which bridges use and ADR 8.4 covers, and every field are
  snake_case already, and kebab-case names aren't names to jq, Python or
  Rust.
- Two spellings on purpose, snake_case for events and kebab-case for
  notes and records, to tell the kinds apart: the reader would have to
  know what the spelling means (P4), and a name that moves between kinds,
  as `records_skipped` does into `--events`, would be renamed. The lists
  say the kind.
- camelCase, as ACP: brnr's events have been snake_case from the start
  (ADR 4.4 kept them so), ACP's own values are snake_case, and ACP's names
  stay inside the ACP objects brnr passes on, so the two never meet in one
  object.
- `session_id` everywhere, as the records and ACP's `sessionId`: the
  events, every `--json` and the commands' `<session>` say `session`, and
  more readers know those than the record heads.
- A `kind` field, or a prefix (`note.records_skipped`, `host.started`):
  one more field for every reader, when the file and the lists say it.
- Verb first (`end_turn`, `close_session`): reads as a command, and
  `end_turn` is ACP's stop reason. Present tense (`turn_end`): a noun,
  read as a thing.
- `agent_exited` for `exited`: its `exit_status` is the agent's, but the
  event is the process's (ADR 5.1), comes after a panic with no status,
  and the command line names processes (`process list`, `--pid`).
- Keeping `by` for both who and why, documented per event: `by ==
  "editor"` and `by == "idle"` are answers to two questions, and #308 adds
  `agent`.
- Leaving the host-log names as they are, as ADR 8.4 doesn't cover them:
  two rules in one file, which holds events too and is read by `doctor`
  and the catch-up.
- `acp_message` for `acp`, `history_replayed` for `history`: a thing is
  named by a noun, and these are short and in every example.
- One record for `peer_dropped` and `peer_cut_off`: the record is written
  when the peer is cut off, the event once the others have the line that
  cut it off, which an exit can prevent, and the catch-up reads the
  record. Two records at two times keep two names.
- A unit in the names of durations and sizes (`_seconds`, `_bytes`): ADR
  8.1 decided against it for #154, one rule said once, and this follows
  it.

## 5.10 What a transcript promises, and how its format is marked

Resolves #70. ADR 5.1 to 5.6 decide what is recorded and how it is read;
this section says what of it 1.x promises (ADR 8.4), and how a reader
tells a format it reads from one it doesn't.

What each stream is for, and what it leaves out:

- **Events** (ADR 5.1 to 5.3) are the host's interpretation of ACP, the
  one story (P5): what a bridge gets live, what `event watch --json` and
  `event log --json` print, and, as the `event` of a record, what the
  events file keeps. They are what scripts and bridges read. They are
  summaries, not the conversation: `agent_message` and `agent_thought`
  carry the text of the agent's chunks, whole, and nothing of a chunk
  that isn't text (an image, audio, a resource); `user_message` carries a
  prompt's text, other blocks as `[image]` or `[resource <uri>]`;
  `tool_call` and `tool_progress` carry a call's id, title, kind, status
  and locations, not its content (a diff, a terminal's output) nor its raw
  input and output, and streamed output is no event (ADR 5.3).
  `permission_request` carries ACP's `tool_call` and `options` whole. No
  event carries what ADR 9.1 redacts; the agent's own text is as the agent
  sent it, a secret in it included.
- **Text** (`event watch`, `event log`, the foreground) is the events
  shown for people, one line each, escaped (src/render.rs). It has nothing
  the events don't (P5), and its wording isn't covered (ADR 8.4).
- **Raw ACP** (`<id>.acp.jsonl`, `acp` events) is every ACP message of the
  session, as it went, in order: each line embedded as it was (`raw`, a
  string, for one that wasn't JSON), but for what ADR 9.1 redacts and a
  line too long to read (ADR 10.3). It is what checks the interpretation,
  and has what the events leave out. `log = "events"` leaves it out.
- **The conversation** is the agent's (P2): what a resume or a load gives
  back is what the agent keeps. brnr's transcript is a record of what
  happened through brnr, not a history to restore: it isn't replayed to an
  agent or an editor (ADR 5.1). What brnr reads back from it to resume a
  session is the session's id, cwd, profile and agent (ADR 3.11).
- **The host log** is the process's own record (ADR 5.1), for `doctor`
  and the catch-up (ADR 10.2), and isn't covered (ADR 8.4). Nothing a
  covered command reads of a session comes from it: `session_opened`
  names the profile and agent in the events file too.

What is known to be missing, and how:

- Records a slow disk skipped, or a failed write lost: a `records_skipped`
  note where they would have been (ADR 5.1, ADR 10.1).
- A replay a load left out, as the transcript had the history: the
  `history` event's `recorded: false`; one recorded is `replayed` (ADR 5.6).
- Turns taken in the agent's own client, or under `log = "off"`: not
  known, as brnr didn't see them (ADR 5.6). `event log` of a session open
  in a process that logs nothing says so.
- Records a process killed with SIGKILL hadn't written: not known (ADR
  5.5); the file ends where its writes ended, partway through a line or
  not, and is read to its last whole record (ADR 5.1).

The format:

- Every record's head starts with `format`, a number: `{"format":1,"ts":…,
  "host_id":…,"pid":…,"proxy_pid":…,"agent_pid":…[,"session":…],…}`, in
  the events files, the raw ACP files and the host logs alike, and in the
  record `session delete` writes (ADR 8.1). It is the format of that
  record, not of its file: a file is appended to by every process that
  serves the session, of whatever brnr, and is read a record at a time,
  backwards too (ADR 5.1).
- Format 1 is ADR 5.1's and 5.9's: a record a JSON object on a line of its
  own; its head as above, `ts` an RFC 3339 time in UTC to the microsecond,
  always that wide, so that records are merged by comparing it as a string
  (ADR 5.1); then `event`, an event as a bridge got it, a note or a record
  (ADR 5.9), or, for an ACP message, `seq` where it has one, `dir` and
  `msg` (or `raw`). `session_opened` names `session`, `cwd`, `profile`,
  `agent` (the command the process ran, as a profile's `agent` and the
  socket's `status` have it, ADR 6.1, ADR 8.2), `events_log`, `acp_log`,
  `host_log` and `forked_from` for a fork. Where the files are is ADR
  3.9's and 9.2's.
- The format stays 1 for all of 1.x. What a 1.x release may add to it, as
  ADR 8.4 allows (a field, an event, a record), is still format 1, and a
  reader passes over what it doesn't know. What a format-1 reader couldn't
  read (a field renamed, removed or of another type, another head, another
  `ts`) is a new format, and waits for a major release.
- A 1.x brnr reads format 1, whichever 1.x wrote it: `event log`, `session
  list`, `session resume` and a load's "brnr has a transcript of the
  session" (ADR 5.6), as a file mixing records of several 1.x releases.
  `doctor` and the catch-up read the host logs by the same rule.
- A record in a format brnr doesn't read, without `format` (an older
  brnr's, before 1.0) or with another (a newer brnr's), isn't read, and
  nothing goes without a word (P3):
  - `event log` shows the records of the file it can read and says, once
    per file, `<file>: records in format 2; this brnr reads format 1, not
    shown (ADR 5.10)`, or `records an older brnr wrote, before transcripts
    had a format`. Of a session none of whose files it can read, it fails,
    saying so.
  - `session list` passes over a file without a record it can read, and a
    file whose newest record is in another format: where the session was
    last served is in a record it can't read, and it doesn't guess from an
    older one (P4). `session resume` of such a session says which files it
    doesn't read, on stderr, and resumes the id as one brnr doesn't know
    (ADR 3.11).
  - A load counts only records it can read as a transcript (ADR 5.6): a
    file of none records the history.
  - `doctor` lists, as info under `file formats`, every transcript and
    host log whose first or newest record is in another format.
  - The catch-up of a process writing another format fails, saying so, and
    the command says what it missed (ADR 10.2).
- Whether a file has records in another format, for `session list`,
  `session resume`, `doctor` and whether brnr appends to it (ADR 5.9's
  `log::appends_to`), is told by its first and newest records, not by
  reading every record. brnr appends only to a file whose first and newest
  records it reads, and one process serves a session at a time (ADR 3.9):
  a file a 1.x brnr wrote changes format only at its end, where a newer
  brnr went on with it, and is then set aside (below). A record in another
  format between records of this brnr's is no record a brnr appended after
  them; a reader that reads every record (`event log`, the catch-up) says
  it all the same, and `session list` takes the file's newest record, one
  it reads, guessing nothing (P4).
- brnr doesn't migrate or rewrite a file in another format (ADR 5.1, P3),
  nor keep a reader for one before 1.0 (P9). A major release that changes
  the format decides whether it reads format 1 too.

Identity: one session id is one session, whatever the agent, the folder,
the process or the file (ADR 3.2, ADR 9.2): one lock (`sessions/<name>.lock`,
ADR 3.9), its files in every project folder read as one (ADR 5.1), every
record naming it in `session`, and every command taking it exact. A process
is `pid` with `host_id`, its run, as pids are reused (ADR 5.9). An id is not
scoped by the agent: two agents that gave one id would share a lock and a
transcript, as ADR 9.2 says of any one id.

Bridges: a bridge gets events, not records, and the format is the files'.
What a started bridge gets is held to ADR 8.4's rule within a major
(fields and events are added, never renamed or removed, and a bridge
passes over what it doesn't know); which brnr it talks to is #149's to
say.

**Why.** A record must say what it is to be read safely. Before this, a
reader knew an older brnr's record only by a field (`session_id`, ADR 5.9):
that tells one format from one before it, needs a new rule for every
change, and reads a newer brnr's records as this brnr's, a renamed field
read as a missing one, a session resumed in the wrong cwd or with no
agent, without a word (P3, P4). A number in every head is the one check,
for any format, older or newer. In every record, as a file is appended to
by every brnr that serves the session in its format (each 1.x, and a
newer major that reads format 1), and read a record at a time: the last record backwards, the
catch-up by position, a deletion's record that no process wrote. `session
resume` took the profile and agent from the host log, which ADR 8.4
doesn't cover and which a user can prune: pruning `~/.brnr/hosts` made a
resume fail with `no agent` (#70). Named in `session_opened`, they are in
the transcript, which is covered.

**Consequences.**

- Each record is 11 bytes longer.
- A file written before this has no `format`, and isn't read (P9):
  `doctor` lists it; ADR 5.9's check by field goes.
- A session downgraded from a newer brnr, its newest records in a newer
  format, is no session to `session list` here; `event log` shows what of
  it this brnr can read. Opened here, it is set aside, `<name>.older.jsonl`,
  and starts a file of this brnr's (ADR 5.9's `log::appends_to`).
- The fixtures in tests/fixtures/transcripts are format 1 as 1.0 writes it,
  frozen: every 1.x is tested against them.

**Considered.**

- A marker per segment, on `session_opened`: fewer bytes, but a record
  read alone (the last, backwards; one the catch-up finds by position; a
  deletion's) would need its segment's opening found first.
- A first line per file: a file is appended to by several processes and
  releases, and its first line says nothing of what came after.
- brnr's version in the head: a reader would need a table from versions to
  formats; which brnr a process is, is #149's.
- Keeping the field check (`session_id`) and adding one per change: a rule
  per format, and a newer brnr's records read as this one's.
- Reading what can be read of a record in another format: a renamed field
  read as missing is a guess (P4).
- `session list` taking the newest record it can read, in a file a newer
  brnr went on with: it would show, and resume, where the session was
  before the newer brnr served it, and say nothing.
- The profile and agent from the host log, as before: a resume would
  depend on a file that isn't covered and can be pruned.
- A session id scoped by its agent: the agent's command changes with its
  path and version, and the same session would become two (ADR 3.2).
- Migrating older files to format 1: brnr doesn't rewrite what it has
  written (ADR 5.1, P9).

## Tests

Run `cargo test --release adr_0005_`. Named claims, by section:

- 5.1: [tests/headless.rs](../../tests/headless.rs):
  `adr_0005_transcripts_are_two_files`,
  `adr_0005_log_events_leaves_out_the_raw_acp`,
  `adr_0005_a_transcript_cut_short_is_still_its_sessions`;
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_log_shows_the_conversation`,
  `adr_0005_log_merges_the_raw_acp_in_time`,
  `adr_0005_log_reads_an_inactive_session`,
  `adr_0005_log_follows_until_the_host_exits`,
  `adr_0005_log_follows_the_raw_acp_too`,
  `adr_0005_session_changed_says_what_changed`,
  `adr_0005_closing_sessions_closes_their_files`;
  [src/log.rs](../../src/log.rs):
  `adr_0005_a_closed_session_has_its_records_and_then_no_file`;
  [src/ctl/ownership.rs](../../src/ctl/ownership.rs):
  `adr_0005_a_session_without_a_transcript_has_none_to_read`;
  [src/transcript.rs](../../src/transcript.rs):
  `adr_0005_the_first_and_last_records_are_ones_brnr_can_read`,
  `adr_0005_the_end_of_a_file_is_its_last_lines`,
  `adr_0005_a_record_is_the_event_watch_showed`,
  `adr_0005_a_sessions_files_are_read_merged_as_they_grow`,
  `adr_0005_a_sessions_files_in_every_folder_are_read_as_one`; one
  session's files in two folders read as one transcript (#121):
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_log_reads_a_sessions_files_in_every_folder_as_one` (text,
  `--json`, `--events acp`, `--last`, and one `session list` row), and,
  for its `session resume`, ADR 3.11's
  `adr_0003_resume_takes_the_newest_records_cwd_agent_and_profile`.
- 5.2: [tests/headless.rs](../../tests/headless.rs):
  `adr_0005_held_messages_are_reported_on_exit`,
  `adr_0005_editor_close_is_an_event`,
  `adr_0005_a_failed_load_drops_its_held_context`;
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_send_wait_on_a_dropped_message`,
  `adr_0005_queue_lists_and_drops`,
  `adr_0005_close_ends_what_follows_the_session`,
  `adr_0005_idle_close_is_an_event`,
  `adr_0005_dropped_context_is_an_event`.
- 5.3: [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_every_event_chosen_is_shown_in_text`,
  `adr_0005_log_shows_only_the_events_asked_for`,
  `adr_0005_thoughts_are_shown_when_asked`,
  `adr_0005_watch_is_readable_by_default`,
  `adr_0005_a_tool_call_again_updates_the_one_running`;
  [src/render.rs](../../src/render.rs):
  `adr_0005_every_event_has_a_line`,
  `adr_0005_lines_of_the_quiet_and_new_events`.
- 5.4: [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_last_counts_user_messages_including_an_unfinished_turn`.
- 5.5: [tests/headless.rs](../../tests/headless.rs):
  `adr_0005_an_exit_is_written_before_the_process_goes`,
  `adr_0005_a_signal_gives_up_on_a_stalled_disk_at_exit`,
  `adr_0005_a_foreground_exit_given_up_on_says_what_it_didnt_write`,
  `adr_0005_a_panic_lets_go_of_its_sessions_before_it_waits`,
  `adr_0005_a_close_is_written_before_the_session_is_resumed`,
  `adr_0005_a_take_over_is_written_before_the_session_is_resumed`,
  `adr_0005_a_last_session_closed_is_written_before_it_is_resumed`,
  `adr_0005_a_take_over_finds_the_transcript_it_continues`,
  `adr_0005_a_close_waits_for_a_stalled_disk_2_s_at_most`,
  `adr_0005_a_close_resumed_in_its_process_is_answered_once_written`,
  `adr_0005_a_closing_session_is_read_once_written_and_said_to_be_closing`,
  `adr_0005_a_failed_load_is_written_before_the_session_is_resumed`,
  `adr_0005_a_start_whose_load_failed_is_written_before_the_session_is_resumed`,
  `adr_0005_an_editors_failed_resume_holds_its_lock_until_written`,
  `adr_0005_an_editors_failed_load_holds_its_lock_until_written`;
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_log_shows_what_the_process_has_recorded`;
  [src/host/sessions.rs](../../src/host/sessions.rs):
  `adr_0005_nothing_is_let_go_of_without_a_lock_or_an_answer`. That a
  session closing in its process is said to be closing, not opening, by every
  command that refuses it is ADR 3.9's
  `adr_0003_the_lock_decides_and_closing_comes_before_opening` and
  `adr_0003_each_place_is_refused_in_the_same_words`
  ([src/ctl/ownership.rs](../../src/ctl/ownership.rs)).
- 5.6: [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_a_first_load_records_the_replayed_history`,
  `adr_0005_a_load_without_the_transcript_records_the_history`,
  `adr_0005_a_load_of_an_empty_history_says_so`,
  `adr_0005_a_bridges_load_finds_the_transcript_in_another_folder`,
  `adr_0005_a_load_in_another_cwd_leaves_the_history_in_the_log` (#121's
  trace: `session resume --cwd` records none of the replay, and `event
  log` shows the history from the first folder),
  `adr_0005_a_load_after_a_failed_load_records_the_history`,
  `adr_0005_a_bridges_load_after_a_failed_load_records_the_history`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0005_an_editors_load_records_the_history_only_without_a_transcript`,
  `adr_0005_an_editors_load_of_a_session_open_here_records_nothing_again`,
  `adr_0005_overlapping_loads_in_an_editor_have_one_history`,
  `adr_0005_a_failed_load_leaves_a_resumed_session_open`,
  `adr_0005_a_load_answered_after_its_close_reopens_the_session`,
  `adr_0005_a_loads_history_is_counted_across_its_sessions_close`,
  `adr_0005_a_load_that_fails_after_its_sessions_close_has_no_history`,
  `adr_0005_overlapping_loads_closed_meanwhile_have_one_history`,
  `adr_0005_a_bridges_load_of_a_session_served_here_records_nothing_again`,
  `adr_0005_an_editors_load_of_a_session_served_here_records_nothing_again`,
  `adr_0005_an_editors_load_after_a_failed_load_records_the_history`,
  `adr_0005_a_load_that_replays_and_fails_counts_its_replay`,
  `adr_0005_an_editors_failed_load_records_its_last_replayed_message`,
  `adr_0005_a_bridges_failed_load_records_its_last_replayed_message`,
  `adr_0005_a_starts_failed_load_records_its_last_replayed_message`,
  `adr_0005_a_session_closed_with_nothing_recorded_is_no_transcript`;
  [src/transcript.rs](../../src/transcript.rs):
  `adr_0005_only_openings_are_no_transcript`;
  [src/log.rs](../../src/log.rs):
  `adr_0005_a_record_beyond_the_opening_counts_at_once`;
  [src/host/sessions.rs](../../src/host/sessions.rs):
  `adr_0005_an_editors_failed_load_closes_the_session_it_opened`,
  `adr_0005_a_replay_outlasts_its_sessions_close_until_its_last_load`. That
  a session brnr has a transcript of keeps the replay out of it however often it
  loads, with a `history` event each time, is ADR 3.11's
  `adr_0003_resume_by_loading_keeps_the_replay_out_of_the_transcript`
  ([tests/cli.rs](../../tests/cli.rs)).
- 5.7: [tests/security.rs](../../tests/security.rs):
  `adr_0005_a_bridge_gets_no_raw_acp_unless_it_asks`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0005_bad_request_line_is_answered`,
  `adr_0005_a_bridge_whose_stderr_isnt_utf8_carries_on`;
  [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_a_bridge_that_closes_its_stdout_gets_events`;
  [tests/chaos.rs](../../tests/chaos.rs):
  `adr_0005_killed_bridge_mid_turn_leaves_owner_and_lock_intact`.
- 5.8: [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_notify_runs_a_command_per_event`,
  `adr_0005_notify_reads_events_as_watch_does`,
  `adr_0005_notify_works_as_a_bridge`,
  `adr_0005_notify_reads_stdin_as_a_bridge`,
  `adr_0005_notify_stdin_ends_with_its_input`,
  `adr_0005_notify_fails_when_cut_off`,
  `adr_0005_notify_cut_off_as_a_bridge_stops_its_command`,
  `adr_0005_notify_stopped_stops_its_command`,
  `adr_0005_notify_cuts_what_the_environment_cant_hold`.
- 5.9: [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_every_line_printed_is_named_and_placed_alike` (`event log
  --json` and `event watch --json` print snake_case names, each one
  `--events` takes, and every line has `ts`, `host_id` and `pid`),
  `adr_0005_a_note_is_taken_by_events_and_refused_by_subscribe`,
  `adr_0005_who_acted_and_why_are_by_peer_and_reason` (`session_closed`,
  `message_dropped`, and `permission_resolved` answered by a peer, a
  cancel and the timeout),
  `adr_0005_a_transcript_in_another_format_isnt_read` (5.10's),
  `adr_0005_brnr_doesnt_append_to_an_older_brnrs_transcript` (set aside
  by a resume, an older brnr's and a newer one's, passed over by
  `session delete`), and
  `adr_0005_notify_reads_stdin_as_a_bridge` (`BRNR_SESSION`, and
  `BRNR_EVENT=process_exited` at the end);
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0005_a_sessions_opening_is_the_same_in_both_files`,
  `adr_0005_an_editors_delete_passes_an_older_transcript_over`, and
  `adr_0010_a_stalled_disk_skips_records_and_says_so` (`--events
  records_skipped` shows the one gap, with `session`, `ts`, `host_id` and
  `pid`);
  [src/log.rs](../../src/log.rs):
  `adr_0005_a_gap_note_has_the_fields_of_an_event`.
- 5.10: the fixtures in
  [tests/fixtures/transcripts](../../tests/fixtures/transcripts), format 1
  as 1.0 writes it, frozen; [tests/cli.rs](../../tests/cli.rs):
  `adr_0005_every_record_brnr_writes_has_its_format` (events, raw ACP, the
  host log and a deletion's record; `session_opened`'s profile and agent),
  `adr_0005_a_format_1_transcript_is_read_by_every_1x_brnr` (`session
  list`, `event log`, its raw ACP, `doctor`, and `session resume` from the
  transcript alone, with no host log, its load's replay not recorded),
  `adr_0005_a_transcript_in_another_format_isnt_read` (an older brnr's
  without `format`, format 2, and a file a newer brnr went on with:
  `session list`, `event log`, `session resume` and `doctor`),
  `adr_0005_a_record_in_another_format_amid_a_file_is_said` (a format-2
  record between format-1 ones: `event log` says it, `session list` lists
  the file),
  `adr_0005_a_transcript_cut_short_is_read_to_its_last_whole_record`
  (partial writes: a broken line and a cut end, and a resume appending on
  a line of its own),
  `adr_0005_a_transcript_of_openings_only_gets_the_history` (missing
  history: the next load records it); [src/transcript.rs](../../src/transcript.rs):
  `adr_0005_a_record_is_read_by_its_format`. The replay rules are 5.6's
  tests.
