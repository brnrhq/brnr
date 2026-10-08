# 37. Adapters through Homebrew: compiled on the user's machine, one formula each

Accepted (former decisions 26 and 29); reviewed 2026-10-07. Implemented.

## Context

brnr runs the community's ACP adapters for Claude Code and Codex. They are
Node packages; claude-agent-acp compiles in Anthropic's Claude Agent SDK,
under Anthropic's Commercial Terms, so brnr's tap can't ship prebuilt
adapters.

## Decision

- `brnr-claude-adapter` and `brnr-codex-adapter`, formulae in brnr's tap,
  compile the adapters on the user's machine from brnr's `adapters/` at the
  same tag, pinned by `bun.lock`, with `bun build --compile` (bun, in
  homebrew-core, is a build-only dependency). Nothing prebuilt is
  distributed; the user's machine
  fetches and compiles the packages, as `adapters/build.sh` does for anyone
  building from source. The npm packages are mentioned as the alternative.
- One formula per adapter: install what you use (each is about 60 MB and its
  own build); licenses that fit (only the Claude adapter compiles in the SDK,
  `license :cannot_represent` with a caveat; the Codex adapter is
  Apache-2.0); one adapter's upstream breaking doesn't stop the other
  installing; a third agent's adapter is one more formula.
- `adapters/` packages the adapters unchanged. brnr adds only finding the
  user's own agent, and `--version` (`adapters/claude.ts`, `codex.ts`).

## Considered

- Telling Homebrew users to install the npm packages: that needs Node 22 on
  the editor's PATH, and the packages likely bring their own copies of the
  agents through optional dependencies (`build.sh` leaves those out).
- One formula for both (`brnr-adapters`): replaced before it was released.
- A tap per adapter: more `brew tap`s for users and more release plumbing,
  for nothing a formula per adapter doesn't give.
- deno as the compiler (also in homebrew-core): `build.sh` already used
  `bun build --compile` with a frozen lockfile, its output is what brnr's
  tests ran against, and both adapters are written for Node; deno's npm
  compatibility would be one more thing to verify for code that spawns and
  talks to subprocesses.
