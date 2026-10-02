// claude-agent-acp as a single executable, running the user's own Claude Code rather
// than the native binary its SDK would otherwise bundle.
import { requireAgent } from "./find";

requireAgent("claude-agent-acp", "claude", "CLAUDE_CODE_EXECUTABLE", ["~/.claude/local"]);
await import("@agentclientprotocol/claude-agent-acp/dist/index.js");
