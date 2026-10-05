// brnr-claude-adapter: claude-agent-acp as a single executable, running the user's own Claude Code rather
// than the native binary its SDK would otherwise bundle.
import { requireAgent, version } from "./find";
// By path rather than through the package's exports: its version, compiled
// in, for --version.
import pkg from "./node_modules/@agentclientprotocol/claude-agent-acp/package.json";

version("brnr-claude-adapter", pkg);

requireAgent("brnr-claude-adapter", "claude", "CLAUDE_CODE_EXECUTABLE", ["~/.claude/local"]);
await import("@agentclientprotocol/claude-agent-acp/dist/index.js");
