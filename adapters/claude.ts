// brnr-claude-adapter: claude-agent-acp (by Zed Industries, Inc. and
// contributors, Apache-2.0; https://github.com/agentclientprotocol/claude-agent-acp),
// unchanged, as a single executable that needs no Node.js. What brnr adds is
// this launcher: it runs the user's own Claude Code rather than the native
// binary the adapter's SDK would otherwise bundle, and answers --version.
import { requireAgent, version } from "./find";
// By path rather than through the package's exports: its version, compiled
// in, for --version.
import pkg from "./node_modules/@agentclientprotocol/claude-agent-acp/package.json";

version("brnr-claude-adapter", pkg);

requireAgent("brnr-claude-adapter", "claude", "CLAUDE_CODE_EXECUTABLE", ["~/.claude/local"]);
await import("@agentclientprotocol/claude-agent-acp/dist/index.js");
