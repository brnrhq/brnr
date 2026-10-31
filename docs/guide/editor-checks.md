# Real-editor checks

The editor contract in [ADR 2.7](../adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)
and [ADR 7.1](../adr/0007-experimental-actions-on-an-editors-session.md) can
only be checked in real editors: an editor can't be faked, and the synthetic
suite (`tests/fake_agent.py`) shows what brnr sends, not what an editor
makes of it. This page is the checklist and the results matrix for those
checks (#216). Check 11 is step 1 of #217.

Nothing here has been run yet. Every cell in the matrix below is
`untested`, and no editor results are claimed here. Run the checks by hand,
replace `untested` with `pass` or `fail` as you go, and give each failure an
issue of its own. Until a cell is filled in, it says nothing about that editor.

## What to record

One cell of the matrix is one editor with one adapter:

- Editors: Zed, and IntelliJ IDEA with the AIR plugin. Add others as
  they come.
- Adapters: `brnr-claude-adapter`, `brnr-codex-adapter`.

For each cell, record:

- the editor's version, from the editor's own About or version display;
- the adapter's version (see below);
- brnr's version;
- the OS and its version;
- for each check, `pass`, `fail` or `untested` in each way of running
  it, with a note of what you saw and the raw ACP that shows it.

Get the program versions from a terminal:

```sh
brnr --version
brnr-claude-adapter --version
brnr-codex-adapter --version
```

A `fail` is any difference from what the check says passing looks like.
The direct run (below) is the baseline: a difference from it is the finding,
even when the turn itself succeeds.

## The three ways to run each check

Run each check in each editor with each adapter, in each of these ways
that applies to it:

1. **Through brnr**, the default: the editor's ACP agent command is
   `brnr acp` with the adapter.
2. **Through brnr, strict**: the editor's command is `brnr acp
   --strict` with the adapter. Stable ACP v1 to the letter: the
   editor's `fs` and `terminal` capabilities reach the agent, and no
   experimental action is available ([ADR 2.1](../adr/0002-acp-faithfully.md)).
3. **Direct, the baseline**: the editor's command is the adapter
   itself, with no brnr. What the editor and the agent do here is what
   the two brnr runs are compared against.

The editor commands for the Claude adapter are:

```sh
brnr acp -- brnr-claude-adapter
brnr acp --strict -- brnr-claude-adapter
brnr-claude-adapter
```

For Codex, put `brnr-codex-adapter` in place of
`brnr-claude-adapter` in each line. Checks 1 to 3 have a fourth run,
through brnr with a profile that names the editor's capabilities;
their steps say where. For brnr-only operations in checks 4, 9, 10 and 11,
record any strict-mode refusal or direct-run limitation separately; leave an operation
that cannot be exercised `untested`, with the reason. Do not call a
refusal a successful take-over or a missing stimulus a pass.

## Before you start

Use a scratch project with no secrets. Raw ACP and host logs can contain
prompts, file contents and terminal output **unredacted**: only MCP server
`env` and `headers` are redacted (see the threat model's
[redaction limits](../threat-model.md#gaps)). Read and trim
excerpts before pasting them into the public matrix or an issue, and replace
your home directory with `~`. Use `brnr doctor --report` for host-log lines;
its path replacement does not remove sensitive content.
Work with one small text file open in the editor. Keep one brnr build for
every run in a cell, and record its version before you start.

### Profiles

Keep `exp` and `fsterm` in a scratch config selected with
`BRNR_CONFIG=/absolute/path/to/scratch/config.toml` in both the editor
agent environment and the terminal, or remove these profiles when done.

The checks need two profiles in the config file
(`$BRNR_CONFIG`, else `$XDG_CONFIG_HOME/brnr/config.toml`, falling back to
`~/.config/brnr/config.toml`; the file must be yours and only you can
read and write it, and its folder must be yours and only you can write it,
as [profiles](profiles.md) says). Add:

```toml
[profiles.fsterm.editor]
client_capabilities = ["fs", "terminal"]

[profiles.exp.editor]
experimental = ["send", "context", "cancel", "permission", "elicitation", "config", "close"]
```

`fsterm` passes the editor's file system and terminal to the agent
([ADR 2.7](../adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)).
`exp` enables every experimental action by name
([ADR 7.1](../adr/0007-experimental-actions-on-an-editors-session.md)).
Neither profile sets `log`, so transcripts keep the raw ACP (the
default, `log = "all"`); a profile with `log = "events"` or `"off"`
leaves nothing for the raw-ACP step of each check.

The editor commands with a profile are:

```sh
brnr acp --profile fsterm -- brnr-claude-adapter
brnr acp --profile exp -- brnr-claude-adapter
```

Check what a run will use before you start it:

```sh
brnr profile show fsterm
brnr profile show exp
```

For the default run, use a profile whose editor part names no
`client_capabilities` and no `experimental` actions (the empty profile
you get with no `--profile` and no `default` profile is one). Confirm
with `brnr profile show`.

### In the editor

Use absolute executable paths in editor settings. A command and its arguments
are separate fields, not one shell command. Keep the same scratch project,
login, model and permission settings for all three runs.

1. **Zed:** open Settings JSON and add a custom entry under `agent_servers`.
   Set `type` to `"custom"`, `command` to the absolute path to brnr and `args` to
   `["acp", "--", "/absolute/path/to/brnr-claude-adapter"]`.
   Open the Agent Panel, select this external agent and start a new thread.
   See Zed's [external-agent setup](https://zed.dev/docs/ai/external-agents).
2. **IntelliJ with AIR:** open AIR's Manage Agents and its custom ACP
   configuration (`~/.jetbrains/acp.json`). Add a named entry under
   `agent_servers` with the same `command` and `args` above. Select that
   custom agent in AIR's New Session menu after saving the file and restarting
   the IDE, and open a session in the scratch project. See JetBrains' [custom-agent setup](https://www.jetbrains.com/help/air/air-in-ides-manage-agents.html).
3. In each editor, register separate named entries for default, strict and
   direct runs. Strict adds `"--strict"` after `"acp"`; direct sets
   `command` to the adapter path and `args` to `[]`. Profile runs add
   `"--profile", "fsterm"` (or `"exp"`) before `"--"`.
4. Repeat with the Codex adapter path. Start a fresh thread/session after
   changing configuration. Record the exact settings and any missing UI
   control rather than substituting the editor's built-in agent.

After the editor starts the agent, confirm from a terminal that brnr
has the session (not needed for the direct run):

```sh
brnr session list
brnr process list
```

Note the session's id from `brnr session list`; the steps below call
it `<session>`.

### Seeing the raw ACP

After each check, look at what crossed. `<session>` is the id above:

```sh
brnr event log <session> --events acp
brnr event log <session>
```

The first prints the raw ACP messages, each as an `acp` event; the
second prints brnr's story of the session (messages, tool calls,
approvals, how the turn ended). Raw ACP and host logs leave prompts, file
contents and terminal output unredacted; only MCP server `env` and `headers`
are redacted. Read and trim the excerpts before posting them in a public
matrix or issue (see [redaction limits](../threat-model.md#gaps)).
`event log` does not replace your home directory with `~`; do that yourself.
Quote only the lines needed to show the result in the cell's note.
Use `brnr event log <session> --events acp --json` to find the run's `host_id`; its host log is
`$BRNR_HOME/hosts/<host_id>.jsonl` (by default under `~/.brnr/hosts/`); checks
that name a host-log record say what to look for there.

## The matrix

Fill in the versions first, then one check at a time. Every result
starts as `untested`. For checks 1–3 put the additional `fsterm` result
in the notes alongside the default result. For checks 4 and 10, the
`brnr acp` column uses `exp`, and the strict column uses `exp` plus
`--strict`. Keep per-action results for check 10 and the three separate
observations for check 11 in the notes.

### Zed with `brnr-claude-adapter`

- Editor: Zed (version: ____)
- Adapter: `brnr-claude-adapter` (version: ____)
- brnr: (version: ____)
- OS: ____

| Check | `brnr acp` | `brnr acp --strict` | Direct |
|---|---|---|---|
| 1. Unsaved buffers | untested | untested | untested |
| 2. Writes | untested | untested | untested |
| 3. Terminal | untested | untested | untested |
| 4. Take-over | untested | untested | untested |
| 5. Prompts and cancel | untested | untested | untested |
| 6. Approvals | untested | untested | untested |
| 7. Settings | untested | untested | untested |
| 8. Disconnect | untested | untested | untested |
| 9. Resume | untested | untested | untested |
| 10. Experimental actions | untested | untested | untested |
| 11. Held-session load | untested | untested | untested |

Notes: (date, what you saw, raw-ACP lines, failure issues)

### Zed with `brnr-codex-adapter`

- Editor: Zed (version: ____)
- Adapter: `brnr-codex-adapter` (version: ____)
- brnr: (version: ____)
- OS: ____

| Check | `brnr acp` | `brnr acp --strict` | Direct |
|---|---|---|---|
| 1. Unsaved buffers | untested | untested | untested |
| 2. Writes | untested | untested | untested |
| 3. Terminal | untested | untested | untested |
| 4. Take-over | untested | untested | untested |
| 5. Prompts and cancel | untested | untested | untested |
| 6. Approvals | untested | untested | untested |
| 7. Settings | untested | untested | untested |
| 8. Disconnect | untested | untested | untested |
| 9. Resume | untested | untested | untested |
| 10. Experimental actions | untested | untested | untested |
| 11. Held-session load | untested | untested | untested |

Notes: (date, what you saw, raw-ACP lines, failure issues)

### IntelliJ with AIR and `brnr-claude-adapter`

- Editor: IntelliJ IDEA with AIR (versions: ____)
- Adapter: `brnr-claude-adapter` (version: ____)
- brnr: (version: ____)
- OS: ____

| Check | `brnr acp` | `brnr acp --strict` | Direct |
|---|---|---|---|
| 1. Unsaved buffers | untested | untested | untested |
| 2. Writes | untested | untested | untested |
| 3. Terminal | untested | untested | untested |
| 4. Take-over | untested | untested | untested |
| 5. Prompts and cancel | untested | untested | untested |
| 6. Approvals | untested | untested | untested |
| 7. Settings | untested | untested | untested |
| 8. Disconnect | untested | untested | untested |
| 9. Resume | untested | untested | untested |
| 10. Experimental actions | untested | untested | untested |
| 11. Held-session load | untested | untested | untested |

Notes: (date, what you saw, raw-ACP lines, failure issues)

### IntelliJ with AIR and `brnr-codex-adapter`

- Editor: IntelliJ IDEA with AIR (versions: ____)
- Adapter: `brnr-codex-adapter` (version: ____)
- brnr: (version: ____)
- OS: ____

| Check | `brnr acp` | `brnr acp --strict` | Direct |
|---|---|---|---|
| 1. Unsaved buffers | untested | untested | untested |
| 2. Writes | untested | untested | untested |
| 3. Terminal | untested | untested | untested |
| 4. Take-over | untested | untested | untested |
| 5. Prompts and cancel | untested | untested | untested |
| 6. Approvals | untested | untested | untested |
| 7. Settings | untested | untested | untested |
| 8. Disconnect | untested | untested | untested |
| 9. Resume | untested | untested | untested |
| 10. Experimental actions | untested | untested | untested |
| 11. Held-session load | untested | untested | untested |

Notes: (date, what you saw, raw-ACP lines, failure issues)

## Checklist

Run the numbered steps in **both editors**, for **both adapters**, using
all three run configurations. Start with direct to establish the baseline.
After every check use `brnr event log <session> --events acp` for brnr runs
and save the relevant request, response and updates alongside the visible
result. Direct runs have no brnr transcript: use the editor's ACP diagnostics
(Zed: Command Palette → `dev: open acp logs`) or the adapter's diagnostic
log, and record if raw traffic is unavailable. Do not infer a protocol call
from a successful answer alone. If an adapter or editor cannot expose a
required capability, leave that part untested and explain why.

### 1. Unsaved buffers

1. Create `editor-check.txt` containing `disk-marker`, save it, then replace
   that text with `buffer-marker` in the editor **without saving**.
2. Ask: “Read editor-check.txt using the editor file tool and quote its
   contents. Do not change or save it.” Record the answer.
3. Repeat in default, strict, direct and the additional `fsterm` profile run,
   resetting the saved and unsaved markers each time.

**Pass:** strict, `fsterm` and a capable direct baseline read `buffer-marker`
through `fs/read_text_file`, with the editor's answer in ACP. Default reads
`disk-marker` from disk, sends no `fs/*` or `terminal/*`, and its host log
records `capabilities_dropped` when those capabilities were advertised.
Record this deliberate default difference, rather than treating it as a
regression. If the agent chooses its own disk tool despite being offered
`fs`, the unsaved-buffer path remains untested.

### 2. Writes

1. Save and open `editor-check.txt` with `before-write`.
2. Ask: “Use the editor file tool to replace editor-check.txt with
   after-write.” Observe the buffer or diff view before saving it.
3. Accept any editor diff, save, and open the file from a separate terminal
   or file viewer to verify its disk contents. Reset and repeat, including
   the `fsterm` profile run.

**Pass:** strict, `fsterm` and a capable direct baseline show the change,
ACP contains `fs/write_text_file` and its response, and saving puts
`after-write` on disk. Default writes directly to disk without `fs/*`;
record how the editor refreshes the buffer and the deliberate difference.

### 3. Terminal

1. Ask: “Use the editor terminal to run `printf 'terminal-marker\n'`,
   report its output and exit status, then release the terminal.”
2. Ask it to run `sleep 60` in the editor terminal. Cancel the turn with
   the editor's Stop/Cancel button while it is running.
3. Verify the command is gone in the editor terminal and the OS process
   viewer. Repeat, including the `fsterm` profile run.

**Pass:** strict, `fsterm` and a capable direct baseline show the terminal;
ACP has `terminal/create`, `terminal/output`, `terminal/wait_for_exit` and
`terminal/release`, output `terminal-marker` and exit status 0. Cancellation
kills or releases the running command (`terminal/kill` or release).
Default sends no `terminal/*` and runs its own child processes instead;
check cancellation cleans those up too and record the difference.

### 4. Take-over

1. Start an editor session with profile `exp`. Leave an unsaved marker in
   the scratch file. Note the session id and start a long turn.
2. In a terminal run the commands for step 2 below.

3. Check the editor's notice, then try a prompt in the old pane. Close the
   headless session with `brnr session close <session>` before continuing.
4. Repeat with strict added to the editor's `exp` command: take-over must
   be refused. Direct has no brnr owner to take over; record that limitation.

Commands for step 2:

```sh
brnr session resume <session> --take-over
brnr prompt send <session> --prompt "Read editor-check.txt from disk and quote it." --wait
```

**Pass:** opted-in non-strict take-over cancels the old turn, tells the
editor “Session taken over by brnr”, and continues headless using disk text.
The old editor prompt gets an error naming where the session continues.
ACP shows the notice and close; strict refuses without taking ownership.

### 5. Prompts and cancel

1. Send “Reply with first-marker only” in the editor and wait for completion.
2. Ask it to run `sleep 60`, then cancel from the editor while it runs.
3. Send “Reply with after-cancel-marker only” and wait for completion.

**Pass:** both short prompts complete, cancellation ends the long turn and
its command, and the pane remains usable without a stuck spinner or a
second answer. Compare all three runs. ACP shows `session/prompt`, the
editor's cancellation and the next prompt and response in order.

### 6. Approvals

1. Select an agent mode that asks permission; avoid an always-allow setting.
2. Ask for a scratch-file edit or harmless command that triggers permission.
   Choose allow once in the editor; check the operation completes.
3. Trigger a fresh request and choose reject once. Check it is not executed.

**Pass:** the editor shows each request once, the decision reaches the agent
once, and the turn continues or reports rejection as in direct. ACP contains
`session/request_permission` and the matching selected option. If no request
can be triggered, record the mode and leave approval testing untested.

### 7. Settings

1. Record the initial mode and model selectors. Pick a different offered
   mode and model in the editor, then send a short prompt.
2. In brnr runs use `brnr config get <session>` to compare the reported
   settings. Inspect ACP for `session/set_mode` or
   `session/set_config_option` and their responses.
3. If the agent offers a native command that changes settings, invoke it
   in the editor and observe its `current_mode_update` or
   `config_option_update`. Record the command used. If it offers none,
   leave that direction untested; check 10 separately tests side-channel changes.

**Pass:** supported selectors, reported agent state and updates agree in
both directions, as in direct. Missing selectors or unsupported models are
recorded as limitations, not guessed from the agent's prose.

### 8. Disconnect

1. Start a long command (`sleep 60`), and record proxy, host, adapter and
   child PIDs in the OS process viewer; `brnr process list` helps identify
   the brnr process. Find its host log using the `host_id` as described above.
2. Close just the thread; observe whether the editor actually disconnects
   its ACP agent. Repeat separately closing the window and quitting the editor.
3. Restart the agent in the editor. In IntelliJ also use AIR's agent restart
   control if available. Check the old PIDs are gone, not merely absent from
   brnr's list. Record any SIGKILL evidence and which process sent it if known.

**Pass:** whenever the editor disconnects/stops its agent, the old proxy,
host, adapter and children end, and restarting leaves no old processes.
If thread closure keeps a shared agent connected, record that behavior and
compare direct. ACP may end abruptly; correlate its last messages with the
host log and OS observations instead of expecting a final ACP response.

### 9. Resume

1. Complete a short turn and record its session id. Close the editor and
   verify its agent process exits as in check 8.
2. Run the commands for step 2 below (add `--strict` to resume for the strict run).

3. Reopen the editor, select the same configured agent and load that session
   from its history. Send “Reply with editor-again-marker only”.
4. For direct, close and reopen the editor and try its own history load;
   there is no brnr headless middle step. Record that limitation.

Commands for step 2:

```sh
brnr session resume <session>
brnr prompt send <session> --prompt "Reply with headless-marker only." --wait
brnr session close <session>
```

**Pass:** headless continuation succeeds, releasing it allows the editor
load, and the editor shows history and accepts a new prompt. ACP shows
resume/load, history updates and the new prompt without duplicate turns.
If the adapter or editor offers no history load, leave that part untested.

### 10. Experimental actions

Use a new `exp` session for each destructive step below. These are the
current [ADR 7.1](../adr/0007-experimental-actions-on-an-editors-session.md#71-side-channel-actions-on-an-editors-session-are-experimental-for-all-of-1x)
actions (the issue calls them old ADR 4's Consequences). Keep a separate
result and ACP excerpt for **each** action in the matrix notes.
`<request>` comes from `permission requests` or `elicitation requests`;
`<mode>` and `<model>` are ids offered by `config get`.

1. **send:** while idle run `brnr prompt send <session> --prompt "Reply with side-marker only"`.
   Check the “Message via brnr” completed tool call appears between turns,
   and the answer appears despite the editor not starting the turn. Try
   prompting from the editor while it runs and record what happens.
2. **context:** run `brnr prompt send <session> --context --prompt "Include context-marker in your next reply"`.
   Send a short prompt in the editor. Check “Context via brnr” and that
   context reaches this next prompt. Repeat after adding `echo_as =
   "user_message_chunk"` under `[profiles.exp.editor]` and restarting;
   test both send and context and record out-of-turn rendering for each form.
3. **cancel:** trigger an approval, then run `brnr prompt cancel <session>`.
   Repeat with an elicitation if the adapter can produce one. Check the
   turn ends and pending dialogs are withdrawn.
4. **permission:** use the permission commands below, triggering a fresh
   approval for each decision.

   Check withdrawal (`$/cancel_request`), tool status `in_progress` for
   allow or `failed` for reject, and “Allowed via brnr” / “Rejected via brnr”.
   If a stale dialog remains, answer it and check only one decision reaches
   the agent and the editor shows “Already allowed/rejected via brnr”.
5. **elicitation:** use the elicitation commands below, triggering separate
   agent questions for each decision.

   Use fields required by the actual question (`elicitation show` gives
   its schema). Check withdrawal and accepted/declined notices, and try a
   late editor answer if possible. Unsupported elicitation stays untested.
6. **config:** run `brnr config get <session>`, then
   `brnr config set <session> --mode <mode>` and
   `brnr config set <session> --model <model>` with different offered ids.
   Check unsolicited mode/config updates move the editor selectors.
7. **close:** run `brnr session close <session>` during a long turn. Check
   the cancellation, “Session closed via brnr” notice and an error on a
   subsequent editor prompt. Load the released session again and prompt.
   Also perform check 4 for take-over.
8. If the editor supports steering, send a message during a long turn and
   inspect whether it sends `_session/steering`; record its behavior.
   Include IntelliJ restart observations from check 8.

Permission commands for step 4:

```sh
brnr permission requests <session>
brnr permission allow <session> <request>
brnr permission reject <session> <request>
```

Elicitation commands for step 5:

```sh
brnr elicitation requests <session>
brnr elicitation accept <session> <request> --field <key>=<value>
brnr elicitation decline <session> <request>
brnr elicitation cancel <session> <request>
```

**Pass:** each available action produces the ADR 7.1 compensation described
above and the editor stays usable; record rendering, dialog withdrawal and
any `-32800` cancellation acknowledgement in ACP. Repeat with `exp` plus
strict: side-channel actions must be refused without injecting updates.
For direct, exercise the editor's equivalent prompt, cancel, approval and
setting controls as the baseline; brnr injections have no direct equivalent.
A partial run leaves the aggregate cell untested with subresults in notes;
a reproduced failure can be marked fail with the untested parts named.

### 11. Held-session load (#217 step 1)

1. Create a session in the editor, complete a short turn, record its id,
   then close the editor. Run `brnr session resume <session>` in a terminal
   so a headless process holds it. Keep `shared_sessions` disabled in the
   editor profile (the default).
2. Reopen the editor and try loading that same id from history. Record the
   refused-load error and whether the pane stays open.
3. If the pane allows input, type “Take over”. Record whether the editor
   sends `session/prompt` for that id after the refused load. This text is
   an observation probe, **not an implemented take-over command**.
4. Inspect `brnr event log <session> --events acp`. The refused load can
   belong to the editor process rather than the owner's transcript; also
   use editor ACP diagnostics to see the request/error and any later prompt.
5. Record separately whether a `session/request_permission` arriving while
   load is pending is shown. Current brnr refuses the held load immediately
   and does not emit #217's proposed permission request. There is no existing
   CLI switch to generate this stimulus. Leave this subcheck untested with
   that reason unless a separately supplied protocol probe is available;
   record its source and exact traffic if used. Do not infer “not shown”
   from no request having been sent.
6. Repeat for strict. Direct has no brnr lock/refusal or permission probe;
   record that limitation. Clean up with `brnr session close <session>`,
   then confirm the editor can load it normally.

**Expected current behavior:** the load is refused without taking ownership;
record the pane-open and subsequent-prompt answers as yes/no observations,
not a preference for #217 option A or C. This check gathers evidence for a
future decision, not proof that editor take-over is implemented. Keep the
aggregate untested while the pending-load permission subcheck is untested.
