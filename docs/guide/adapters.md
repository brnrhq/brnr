# Adapters

The ACP adapters aren't brnr's work. `adapters/` packages two existing
open-source adapters, unchanged, as standalone executables
(`bun build --compile`). Each carries its own JavaScript runtime, so it needs
no Node.js (or bun) installed, unlike the npm packages:

| Command | Builds | By | License |
|---|---|---|---|
| `brnr-claude-adapter` | [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp) | Zed Industries, Inc. and contributors | Apache-2.0; it includes Anthropic's [Claude Agent SDK](https://github.com/anthropics/claude-agent-sdk-typescript), under Anthropic's [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) |
| `brnr-codex-adapter` | [codex-acp](https://github.com/agentclientprotocol/codex-acp) | JetBrains s.r.o. | Apache-2.0 |

brnr adds only a few lines in front of each (`adapters/claude.ts`,
`adapters/codex.ts`, `adapters/find.ts`): finding the user's own agent, and
`--version`. The commands have names of their own so they don't clash with
the npm packages' (`claude-agent-acp`, `codex-acp`) when both are installed.

They don't include the agents. Each runs the user's own `claude` or `codex`
(from `PATH` or the usual install locations, or `CLAUDE_CODE_EXECUTABLE` /
`CODEX_PATH`). `build.sh` copies every package's license file (`LICENSE`,
`license`, `License.txt`, …), nested packages' too, to `licenses/`. The Claude
Agent SDK's license isn't an open-source one; check Anthropic's terms before
redistributing a build of brnr-claude-adapter.
