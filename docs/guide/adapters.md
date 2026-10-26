# Adapters

Agents that speak ACP themselves, such as Junie and Cursor, need no adapter:
see [Agents with ACP of their own](native-agents.md).


The ACP adapters aren't brnr's work. `adapters/` packages three existing
open-source adapters as standalone executables. Claude Code and Codex use
`bun build --compile` and carry their own JavaScript runtime; Muse Code is
built with Cargo from upstream Rust source. None needs Node.js or bun at runtime:

| Command | Builds | By | License |
|---|---|---|---|
| `brnr-claude-adapter` | [claude-agent-acp](https://github.com/agentclientprotocol/claude-agent-acp) | Zed Industries, Inc. and contributors | Apache-2.0; it includes Anthropic's [Claude Agent SDK](https://github.com/anthropics/claude-agent-sdk-typescript), under Anthropic's [Commercial Terms](https://www.anthropic.com/legal/commercial-terms) |
| `brnr-codex-adapter` | [codex-acp](https://github.com/agentclientprotocol/codex-acp) | JetBrains s.r.o. | Apache-2.0 |
| `brnr-muse-adapter` | [muse-acp](https://github.com/BrokkAi/muse-acp) | BrokkAi and contributors | Apache-2.0 |

brnr adds only a few lines in front of each (`adapters/claude.ts`,
`adapters/codex.ts`, `adapters/find.ts`): finding the user's own agent, and
`--version`. The commands have names of their own so they don't clash with
the npm packages' (`claude-agent-acp`, `codex-acp`, `muse-acp`) when both are installed.
Muse's Rust sources are unchanged; its own `--version` and agent lookup are
used. Its version pin is the scoped `@brokkai/muse-acp` devDependency, used
only to select a source tag and receive Dependabot updates. Its npm binaries
aren't installed by the source build. Build only Muse with
`adapters/build.sh target-dir muse`; bun isn't needed for that build.

They don't include the agents. Each runs the user's own `claude`, `codex` or `muse`
(from `PATH` or the usual install locations, or `CLAUDE_CODE_EXECUTABLE` /
`CODEX_PATH` / `MUSE_CLI`). Muse also searches `MUSE_INSTALL_DIR`, then
`~/.local/bin`, when `PATH` has no `muse`; run `muse login` first.
The alternative npm package is **`@brokkai/muse-acp`**: the unscoped
`muse-acp` is an unrelated project.

Muse uses ACP v1, including brnr's existing steering, session list/resume,
approvals and config options. Approval Mode (`approval_mode`) and Auto-review
(`auto_review`) remain the adapter's policy; brnr doesn't enable either for you.
Muse-specific extensions and metadata pass through the editor connection and
are preserved in raw ACP transcripts; dedicated headless commands are separate work.

`build.sh` copies every package's license file (`LICENSE`,
`license`, `License.txt`, …), nested packages' too, to `licenses/`. The Claude
Agent SDK's license isn't an open-source one; check Anthropic's terms before
redistributing a build of brnr-claude-adapter. Muse's upstream LICENSE and NOTICE
are copied alongside them.
