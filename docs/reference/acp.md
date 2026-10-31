# acp

## `brnr acp`

What an editor runs as its ACP agent. It starts a brnr process that runs the
agent, and relays ACP between the editor and that agent
([ADR 3.1](../adr/0003-processes-sessions-and-ownership.md#31-the-editors-process-brnr-acp-relays-the-host-owns-the-agent)).
The editor initializes the protocol and owns its sessions.

    brnr acp [--profile <p>] [--strict] [--remote <target>] [-- <agent> [args...]]

| Flag | Meaning |
|---|---|
| `--profile <p>` | The profile, loaded on the machine that runs the agent: without `--remote`, this one; with `--remote`, the destination, and source config isn't read. Without `--profile`, that machine's `default` profile is used when present. |
| `--remote <target>` | Execute through SSH on `[user@]host` or an SSH alias. See [remote hosts](../guide/remote.md). |
| `--strict` | Stable ACP only ([ADR 2.1](../adr/0002-acp-faithfully.md)): what isn't stable ACP is refused, and the experimental actions on the editor's session are off. |
| `-- <agent> [args...]` | The agent to run, in place of the profile's. |

See [which machine loads the profile](../guide/remote.md#which-machine-loads-the-profile).
Without `--remote`, `brnr acp` starts the agent locally; profiles do not
select SSH routing.

It has no `--json`: stdout is ACP.

### The editor's streams

- **Stdin and stdout** carry the editor-agent ACP JSON-RPC stream, not the
  [bridge protocol](bridge.md). Don't write messages for people to this
  stdout. Stdout ends when the agent's stdout does, even if the agent runs on;
  brnr writes nothing more there itself.
- **Stdin** closes when the agent's does, so the editor's next write fails.
  When the agent closed its stdin itself, brnr finds out on the write that
  fails to reach it, which is recorded as sent, and the writes after it
  fail.
- **Stderr** carries brnr's diagnostics and the agent's stderr, as it is
  written. It ends when `brnr acp` exits. brnr says its errors there with no
  code: the editor gets none as data.
- Both streams share one connection and a 16 MiB buffer, so an editor that
  stops reading one eventually holds up the other
  ([ADR 2.6](../adr/0002-acp-faithfully.md#26-the-editors-stdin-and-stdout-end-when-the-agents-do-and-its-stderr-comes-as-written)).

### What brnr changes

- The editor's `initialize` reaches the agent without its `fs` and
  `terminal` client capabilities, as ACP v2 has neither, but for those the
  profile's `client_capabilities` names. In strict mode both reach it.
- The agent's `fs/*` and `terminal/*` requests, where it was offered them,
  go to the editor, which answers them. A headless process offers neither,
  and answers them with "method not found"
  ([ADR 2.7](../adr/0002-acp-faithfully.md#27-the-editors-file-system-and-terminal-reach-the-agent-in-strict-mode-or-when-its-profile-names-them)).
- What brnr answers in the agent's place is a JSON-RPC error as ACP has it
  (`-32603`, brnr's message).
- Actions on an editor's session from outside (`prompt send`, `permission
  allow`, …) are experimental for all of 1.x, each enabled by the profile's
  `editor.experimental`; strict mode disables them. See
  [editor setup and deviations](../guide/editor.md#use-it-from-an-editor),
  [strict mode](../guide/editor.md#strict-mode-and-feature-flags) and
  [experimental actions](../guide/editor.md#experimental-actions).

**Covered** from 1.0.0: ACP version 1
([ADR 2.2](../adr/0002-acp-faithfully.md#22-brnr-speaks-acp-version-1)), and
what passes between the editor and the agent: unchanged, but for the changes
ADRs 3.1, 3.9, 4 and 7.1 name, with the defaults they have at 1.0.0.

**Exit codes:** 2 for a usage error; 1 for a config error or a process that
fails to start; after the start, the agent's exit status, or its signal.
