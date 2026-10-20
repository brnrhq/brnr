# Architecture decision records

What brnr decided, why, and what else was considered, measured against the
critical success factors and the principles in ADR 1. One record per topic;
each decision is a numbered section of it, cited as `ADR T.k` (ADR 3.9 is
topic 3's ninth decision).

## Writing one

- A decision goes in the topic it belongs to, as the next section,
  `## T.k What is decided`, a sentence that says what is true. A new topic
  is a new file, `NNNN-short-title.md`, the next number, titled
  `# N. Topic`.
- Under a topic's title, a status line: `Draft` and the date (`Accepted`
  from 1.0.0), whether its decisions are implemented (and if only partly,
  which aren't), and the factors it serves. When a decision's
  implementation lands, the status line says so.
- A section states the decision as it stands, and the issues it resolves,
  then `**Why.**` (what made it necessary, citing factors and principles),
  `**Consequences.**` where any are worth saying, and `**Considered.**`
  (the options not taken, and why).
- A topic ends with its `## Tests`.

## Changing one

- Until 1.0.0 every record is a draft (P9). A decision is changed where it
  is written: a record is rewritten, renumbered, merged into another or
  removed, so that the records say what is decided now. No `Amended by` or
  `Superseded by` lines are needed. A decision renumbered or removed takes
  its references with it: `ADR T.k` in the code and the docs, and
  `adr_NNNN_` in test names. The pull request says what changed, and git
  keeps what was.
- At 1.0.0 the records are accepted, and their status lines say so. From
  then a decision is changed by a new section in its topic, the next
  `T.k`. The old section gets a line under its heading, `Superseded by T.m`
  or `Amended by T.m: …`, in the order of the sections' numbers, and is
  otherwise left as it was.

## Records

| | Topic | Decisions | Implemented |
|---|---|---|---|
| [1](0001-principles.md) | Critical success factors and principles | P1 to P15, F1 to F5, N1 to N4 | — |
| [2](0002-acp-faithfully.md) | ACP, faithfully | 2.1 Strict mode, and the conventions brnr follows by default; 2.2 brnr speaks ACP version 1; 2.3 ACP's types come from the official schema crate; 2.4 Every request to the agent has an id of the host's; 2.5 Lines brnr can't parse; 2.6 The editor's stdout ends when the agent's does, and its stderr comes as written; 2.7 The editor's file system and terminal reach the agent in strict mode, or when its profile names them | yes |
| [3](0003-processes-sessions-and-ownership.md) | Processes, sessions and ownership | 3.1 The editor's process: `brnr acp` relays, the host owns the agent; 3.2 Sessions are the agent's ids, processes are pids; 3.3 A process is started with one resolved request; 3.4 A start is atomic and commits at ready; 3.5 A headless session in the foreground; 3.6 The agent's stderr; 3.7 A process's own death is recorded; 3.8 Stopping when idle; 3.9 Ownership is a per-session lock; 3.10 Feature flags for process management; 3.11 Resuming; 3.12 Fork and close | partly: 3.11 for an id with files in two folders waits for #309 |
| [4](0004-turns-messages-and-requests.md) | Turns, messages and the agent's requests | 4.1 Message ids, and when a turn ends; 4.2 What sending a message does; 4.3 What cancelling does with held messages; 4.4 A turn's tokens are what the agent reported; 4.5 Approvals and elicitations: the agent's mode is the policy; 4.6 Requests the agent answers; 4.7 Authentication | partly: 4.5 for a request the agent cancels waits for #308 |
| [5](0005-events-transcripts-and-observers.md) | Events, transcripts and observers | 5.1 The host's events are the story, and transcripts are two files; 5.2 Dropped messages and closed sessions are events; 5.3 Choosing events, and what text shows; 5.4 `event log --last <n>`; 5.5 The log of a running session waits for its transcript; 5.6 A load's history is recorded once; 5.7 Bridges; 5.8 `event notify`; 5.9 Events, records and their fields follow one naming scheme | partly: 5.1 for one session's files in two folders waits for #309 |
| [6](0006-settings-and-profiles.md) | Settings and profiles | 6.1 Profiles, and a start's flags over them; 6.2 Mode, model and config options; 6.3 MCP servers for headless sessions; 6.4 Files and images in a message | yes |
| [7](0007-experimental-actions-on-an-editors-session.md) | Experimental actions on an editor's session | 7.1 Side-channel actions on an editor's session are experimental, for all of 1.x; 7.2 How a message sent through brnr is shown to the editor | yes; 7.1's real-editor checks aren't run yet |
| [8](0008-the-command-line-and-compatibility.md) | The command line and compatibility | 8.1 Commands are grouped by what they act on, with ACP's verbs; 8.2 Same data in text and JSON; 8.3 Waiting, and exit status; 8.4 What the compatibility promise covers; 8.5 The library is private, and its facade mirrors the command line; 8.6 A skill for agents that use brnr | partly: 8.5's typed facade (#215) isn't built; 8.4 applies from 1.0.0; 8.6's evals with a real agent aren't done |
| [9](0009-security-and-privacy.md) | Security and privacy | 9.1 Secrets are redacted in what brnr records; 9.2 One file name per session id, one folder per cwd; 9.3 Transcripts are written only where only you can read them; 9.4 The config file is read only if only you can change or read it | yes |
| [10](0010-bounded-resources.md) | Bounded resources | 10.1 How far behind a reader may fall; 10.2 A peer cut off catches up from the transcript; 10.3 Lines too long to read | yes |
| [11](0011-building-releasing-and-support.md) | Building, releasing and support | 11.1 Adapters through Homebrew; 11.2 The adapters' names, and finding them next to brnr; 11.3 Which adapter version is built; 11.4 `release.sh`; 11.5 Bug reports; 11.6 brnr on crates.io; 11.7 Reproducible builds and an SBOM | yes |

## Where the former records went

Until 2026-10-19 each decision was a record of its own, numbered 1 to 71;
the released CHANGELOG entries, issues and commits cite those numbers. Each
former record, and the section it is now (old→new):

1→1, 2→3.1, 3→3.9, 4→7.1, 5→7.2, 6→10.1, 7→3.4, 8→3.3, 9→3.5, 10→3.6,
11→3.7, 12→3.8, 13→3.2, 14→3.11, 15→8.1, 16→3.12, 17→4.1, 18→4.2, 19→4.3,
20→5.2, 21→8.3, 22→5.1, 23→5.3, 24→5.4, 25→9.1, 26→2.5, 27→4.5, 28→6.2,
29→4.6, 30→4.7, 31→6.3, 32→6.4, 33→6.1, 34→8.2, 35→5.7, 36→5.8, 37→11.1,
38→11.2, 39→11.3, 40→11.4, 41→2.1, 42→3.10, 43→2.3, 44→P15, 45→11.5, 46→8.6,
47→11.6, 48→5.5, 49→10.1, 50→3.9, 51→10.3, 52→11.7, 53→9.2, 54→2.2, 55→5.1,
56→4.1, 57→5.6, 58→6.1, 59→9.3, 60→10.1, 61→2.4, 62→2.6, 63→8.1, 64→8.4,
65→3.9, 66→8.5, 67→2.7, 68→4.5, 69→10.2, 70→7.2, 71→4.4.

Before them, the decision log (`docs/decisions.md`) and the questions the
code review left open (`docs/to-resolve.md`) are in git history up to
commit 5b585a3.

## Tests that name their claims

Runtime regression tests use `adr_NNNN_<claim>` names, `NNNN` the topic.
Each topic's Tests section links the assertions, by section; `cargo test
--release adr_0007_` runs one topic's claims. A test can establish a
specific claim without proving every sentence in a decision, especially
real-editor rendering, external releases, and operational guarantees. The
names state the scope of the evidence.

CI first runs the suite, then `.github/scripts/adr-tests.py` lists the tests
registered by Cargo (including unit and integration tests). It prints every
topic, its claim tests, and topics without Rust tests. An implemented topic
without a registered test or an explicit `Test exemption:` line fails CI, as
does an ignored claim test or a test naming a nonexistent topic. A topic
with tests has no `Test exemption:` line; a decision in it that no Rust
test covers says so (`Not covered by a Rust test: …`), with the external
check or the remaining gap. ADR 1's principles are reported too, without pretending
that a passing finite suite proves universal design principles.

When changing a claim, update its test and its topic's Tests section. Name a
new decision's tests by its topic's number; don't satisfy the inventory with
a test that merely searches source code or repeats the ADR text.
