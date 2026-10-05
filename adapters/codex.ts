// brnr-codex-adapter: codex-acp as a single executable, running the user's own Codex CLI (as
// `codex app-server`) rather than the one @openai/codex would bundle.
import { requireAgent, version } from "./find";
// By path rather than through the package's exports: its version, compiled
// in, for --version.
import pkg from "./node_modules/@agentclientprotocol/codex-acp/package.json";

version("brnr-codex-adapter", pkg);

requireAgent("brnr-codex-adapter", "codex", "CODEX_PATH");
await import("@agentclientprotocol/codex-acp/dist/index.js");
