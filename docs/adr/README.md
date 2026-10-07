# Architecture decision records

What brnr decided, why, and what else was considered: one record per
decision, measured against the principles in ADR 1.

They replace the former decision log (`docs/decisions.md`) and the questions
the code review of `brnr acp` and `brnr start` left open
(`docs/to-resolve.md`), after every entry of both was reviewed in October
2026. Both files are in git history up to commit 5b585a3. In the records,
"former decision N" is an entry of that log and "review item N" a question
of to-resolve; the map at the end says where each went.

## Writing one

- A file `NNNN-short-title.md`, the next number, titled `# N. What was
  decided`.
- Under the title: when it was accepted, whether it is implemented (and if
  only partly, what isn't), and what it replaces or resolves.
- Then `Context` (what made a decision necessary), `Decision`,
  `Consequences` where there are any worth saying, and `Considered` (the
  options not taken, and why).
- A decision is changed by a new record. The old one gets a line under its
  title, `Superseded by N` or `Amended by N: …`, and is otherwise left as it
  was. When a record's implementation lands, its status line says so.

## Records

| | Decision | Implemented |
|---|---|---|
| [1](0001-principles.md) | Principles | — |
| [2](0002-the-editors-process.md) | The editor's process: `brnr acp` relays, the host owns the agent | mostly |
| [3](0003-ownership-is-a-per-session-lock.md) | Ownership is a per-session lock | no |
| [4](0004-side-channel-actions-on-an-editors-session-are-experimental.md) | Side-channel actions on an editor's session are experimental | no |
| [5](0005-injected-messages-are-shown-as-a-tool-call.md) | Injected messages are shown to the editor as a tool call | yes |
| [6](0006-how-far-behind-a-reader-may-fall.md) | How far behind a reader may fall | partly |
| [7](0007-a-start-is-atomic.md) | A start is atomic and commits at ready | yes |
| [8](0008-one-resolved-start-request.md) | A process is started with one resolved request | yes |
| [9](0009-start-foreground.md) | `start --foreground` | partly |
| [10](0010-the-agents-stderr.md) | The agent's stderr | partly |
| [11](0011-a-process-death-is-recorded.md) | A process's own death is recorded | no |
| [12](0012-stopping-when-idle.md) | Stopping when idle | mostly |
| [13](0013-sessions-are-ids-processes-are-pids.md) | Sessions are the agent's ids, processes are pids | yes |
| [14](0014-resuming.md) | Resuming | mostly |
| [15](0015-list-and-sessions.md) | `list` and `sessions`, and one row shape | yes |
| [16](0016-fork-and-close.md) | Fork and close | partly |
| [17](0017-message-ids-and-turns.md) | Message ids and turns | yes |
| [18](0018-what-send-does.md) | What `send` does | no |
| [19](0019-what-cancel-does-with-held-messages.md) | What `cancel` does with held messages | yes |
| [20](0020-dropped-messages-and-closed-sessions-are-events.md) | Dropped messages and closed sessions are events | mostly |
| [21](0021-waiting-and-exit-status.md) | Waiting, and exit status | yes |
| [22](0022-the-hosts-events-are-the-story.md) | The host's events are the story, and transcripts are two files | yes |
| [23](0023-choosing-events.md) | Choosing events, and what text shows | yes |
| [24](0024-log-last.md) | `log --last <n>` | yes |
| [25](0025-secrets-are-redacted.md) | Secrets are redacted in what brnr records | yes |
| [26](0026-lines-brnr-cant-parse.md) | Lines brnr can't parse | mostly |
| [27](0027-approvals.md) | Approvals: the agent's mode is the policy | mostly |
| [28](0028-mode-model-and-config.md) | Mode, model and config options | partly |
| [29](0029-requests-the-agent-answers.md) | Requests the agent answers | yes |
| [30](0030-authentication.md) | Authentication | yes |
| [31](0031-mcp-servers.md) | MCP servers for headless sessions | yes |
| [32](0032-files-and-images.md) | Files and images in a message | yes |
| [33](0033-profiles.md) | Profiles: shared, headless and editor parts | mostly |
| [34](0034-same-data-in-text-and-json.md) | Same data in text and JSON; `--json` wherever a command prints data | yes |
| [35](0035-bridges.md) | Bridges | yes |
| [36](0036-notify.md) | `notify` | yes |
| [37](0037-adapters-through-homebrew.md) | Adapters through Homebrew: compiled on the user's machine, one formula each | yes |
| [38](0038-adapter-names-and-lookup.md) | The adapters' names, and finding them next to brnr | yes |
| [39](0039-adapter-versions.md) | Which adapter version is built, and how it shows | yes |
| [40](0040-release-sh.md) | `release.sh` | yes |
| [41](0041-strict-mode.md) | Strict mode | no |
| [42](0042-feature-flags.md) | Feature flags for process management | no |
| [43](0043-acp-schema-types.md) | ACP types from the official schema crate | no |

## Where the former entries went

Former decisions: 1 → 22; 2 → 23; 3 → 17; 4 → replaced by 7; 5 → 21;
6 → 19; 7 → 27; 8 → 34; 9 → 28 and 4; 10 → 14 and 3; 11 → 16 and 4;
12 → 27; 13 → 31; 14 → 12; 15 → 32; 16 → 30; 17 → 36; 18 → 23; 19 → 23;
20 → 24; 21 → 21 and 7; 22 → 29; 23 → 36; 24 → replaced by 18;
25 → 6 and 35; 26 → 37; 27 → 38; 28 → 38; 29 → 37; 30 → 39; 31 → 40;
32 → 2; 33 → 1 (P9); 34 → 22 and 23; 35 → 22; 36 → 14, 15 and 34;
37 → 13 and 35; 38 → 13; 39 → 13; 40 → 2; 41 → 12 and 7; 42 → 34;
43 → 15; 44 → 34 and 21; 45 → 9, 8 and 27; 46 → 15; 47 → 27; 48 → 5.

Review items: 1 → 6; 2 → 26; 3 → 9 and 6; 4 → 35 and 36; 5 → 20;
6 → 4 and 28; 7 → 7; 8 → 3; 9 → 10; 10 → 25; "to investigate" → 11.
