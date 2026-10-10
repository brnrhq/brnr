# 46. A skill for agents that use brnr

Proposed 2026-10-08. Implemented: `skills/brnr/`, printed and installed by
`brnr skill`, its examples run as tests. Not yet: evals with a real agent
(Consequences). Resolves #26.

Amended by 63: the skill's commands are grouped: `start` is
`session new`; `list` and `status` are `session list` and
`session status`; `send` is `prompt send`; `pending`, `show`, `approve` and
`deny` are `permission requests`, `permission show`, `permission allow` and
`permission reject`; `close` is `session close`, and `stop` `process stop`;
`wait`, `log`, `watch` and `notify` are `event wait`, `event log`,
`event watch` and `event notify`.

## Context

Agents won't only help people set brnr up. They will use it to run other
agents: start headless workers, send them follow-ups, wait for their turns,
read what they did, and bring their approvals to the user. Nothing tells an
agent how to do that well, and what it does by default is what a script
would: pick "the only session", poll `status` in a loop, approve what blocks
it, retry what failed, and leave workers running. Each of those breaks a
principle brnr holds itself to (P3, P4, P14), now done by brnr's user.

Agents learn a tool from a skill: a `SKILL.md` with a `name` and a
`description` saying when to use it, loaded when the description matches,
with further files read only when needed. Claude Code reads skills from
`~/.claude/skills/<name>/` (and `.claude/skills/` in a project); Codex from
`~/.agents/skills/<name>/` (and `.agents/skills/` from the working directory
up to the repository's root), as its documentation has it in October 2026.

How one agent should manage another is a decision about brnr's contract, so
it is made here, as principles the skill states (S1 to S7), each checked
against ADR 1.

## Decision

### Principles

- **S1. The CLI is the interface.** The skill teaches brnr's commands as
  they are, and adds nothing between them and the agent: no scripts, no MCP
  server, no wrapper. The headless CLI is already a contract that behaves the
  same with every agent (P12), and it prints the same data in text and JSON
  (P5, ADR 34); a wrapper would be a second interpretation of it, with its
  own drift. Every command the skill shows is one `brnr --help` lists, with
  `--json` wherever it prints data, and runs as a test (Testing, below).
- **S2. Exact ids, never guesses.** A session is the `session` that
  `start --json` printed, or a row of `list --json`; a request the `request`
  of `pending --json` or `wait --for permission --json`; a message the
  `message` of `send --json`. Never "the only session", never an id read out
  of the text output, never one remembered from another conversation without
  finding it in `list --json` (P4, ADR 13, ADR 17).
- **S3. Wait, don't poll.** `send --wait`, `start --wait` and
  `wait --for idle|turn|permission|exit`, each with a `--timeout`, so that a
  stuck worker can't hang its supervisor; never a loop of `sleep` and
  `status`. The exit status is the result (ADR 21): 0 the turn ended
  normally, 1 it failed, stopped, or its message was dropped, 124 the
  timeout passed, which says nothing about the worker: its turn goes on, and
  waiting again, cancelling or asking the user is a choice to make.
- **S4. Approvals belong to the human.** `approve` and `deny` are the
  user's answers. An agent gives one only where the user delegated it,
  explicitly and for that scope: which sessions, and which kinds of request
  (`edit` in `src/`, `execute` of `cargo test`), for the task at hand. It
  never widens a delegation, and never changes a worker's mode to one that
  asks less (`acceptEdits`, `bypassPermissions`) to avoid asking, unless the
  user asked for that mode: how much the agent asks is its mode (P2,
  ADR 27), and choosing it is the user's. By default the agent shows the
  request (`brnr show`), says what it is in a sentence, and waits for the
  user's answer. A request on an editor's session is answered in the editor
  (ADR 4).
- **S5. Clean up what you start; touch nothing else.** Workers start with
  `--stop-when-idle`, so that a supervisor that dies or forgets leaves no
  orphan (P14, ADR 12), and are closed (`close`) when their work is done;
  `stop <pid>` only for a process the agent started. Sessions it didn't
  start, an editor's above all, are observed (`list`, `status`, `log`,
  `watch`), never acted on (P11, ADR 4), and never taken over
  (`--take-over`) unless the user asks for it in so many words (ADR 3).
- **S6. Nothing dropped silently.** A turn that failed, a message dropped
  (`dropped` in `--json`, a `message_dropped` event), a timeout, an agent
  that exited: each is told to the user, with what brnr said, and not
  retried quietly. Retrying is the user's decision, or one the user
  delegated (P3).
- **S7. A short core, details on demand.** `SKILL.md` has when to use brnr,
  S1 to S6 as rules, and the dozen commands most uses need, short enough to
  load whole every time. The rest is in references, read when the task needs
  them: orchestrating workers, approvals, observing, setting brnr up.

Checked against ADR 1, the principles hold brnr's user to what brnr holds
itself to (P3, P4, P14), and leave to the user what brnr leaves to the agent
or the user (P2, P11). None breaks a principle. The skill is documentation
and a CLI command: outside ACP, and none of the kinds of "beyond" P1 marks.

### What it covers

- `skills/brnr/SKILL.md`: frontmatter (`name: brnr`, a `description`
  saying to use it when asked to run, supervise or talk to coding-agent
  sessions through brnr); when to use brnr; the rules; the core commands.
- `references/orchestrate.md`: a worker's life (start, send, wait, read,
  close), several workers at once, the exit statuses, `--stop-when-idle`,
  steering and interrupting, held messages.
- `references/approvals.md`: `pending`, `show`, `approve`, `deny`, and S4
  in full: what a delegation is, what isn't one, and what to show the user.
- `references/observe.md`: `status`, `log`, `watch`, `notify`, choosing
  events, and the events' JSON, as brnr prints it.
- `references/setup.md`: installing brnr and the adapters, an editor's
  agent command, profiles, `doctor`, installing the skill.

### How it ships

- In the repository as `skills/brnr/` (`SKILL.md`, `references/*.md`), and
  in the binary, embedded at build time (`include_str!`), so the skill an
  agent reads is the one for the brnr it runs.
- `brnr skill` prints `SKILL.md`; `brnr skill <reference>` prints one
  reference (`orchestrate`, `approvals`, `observe`, `setup`), so that an
  agent that only has the binary can read them. Neither takes `--json`: they
  print a document, not data.
- `brnr skill install [--dir <dir>]...` writes the skill to `<dir>/brnr/`,
  for each `--dir`, and by default to both `~/.claude/skills/brnr/` and
  `~/.agents/skills/brnr/`: a fixed, documented rule that doesn't depend on
  which agents are installed (P4). It replaces the files it writes, removes
  references a newer brnr no longer has, and prints each directory it
  wrote. A directory that can't be written fails, saying which (P7).
  Installing again after upgrading brnr updates it.

### Testing

The skill's Markdown files are in `DOCS` in `tests/docs.rs` (#25), so every
line of their `sh` blocks runs against the fake agent or is skipped with a
reason, and every `toml` block loads as a config. `brnr skill` and
`skill install` are tested in `tests/cli.rs`.

### Left out

- An MCP server or scripts (S1).
- A Claude Code plugin: `skill install` covers it for now; a plugin can
  package the same files later.
- Writing bridges: the bridge protocol is for programs, and the README has
  it; the skill points there.
- Acting on an editor's session (ADR 4's experimental actions): the skill
  says not to (S5).
- `doctor` checking an installed skill against the binary's.

## Consequences

- The issue's later step stays: evals with a real agent, checking that it
  used exact ids, waited rather than polled, didn't approve without a
  delegation, and cleaned up. The examples' tests check that the skill's
  commands are right, not that an agent follows its rules.
- A change to the CLI that breaks an example in the skill fails the tests,
  as one in the README does.

## Considered

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

Run `cargo test --release adr_0046_`. Named claims and their assertions:

- [tests/docs.rs](../../tests/docs.rs)
  - `adr_0046_every_example_is_run_or_skipped_and_runs`.
  - `adr_0046_every_toml_block_loads`.
- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0046_skill_prints_the_skill_and_its_references`.
  - `adr_0046_skill_install_writes_it_for_claude_code_and_codex`.
