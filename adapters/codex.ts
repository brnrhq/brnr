// brnr-codex-adapter: codex-acp as a single executable, running the user's own Codex CLI (as
// `codex app-server`) rather than the one @openai/codex would bundle.
import { requireAgent } from "./find";

requireAgent("brnr-codex-adapter", "codex", "CODEX_PATH");
await import("@agentclientprotocol/codex-acp/dist/index.js");
