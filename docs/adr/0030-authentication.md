# 30. Authentication

Accepted (former decision 16); reviewed 2026-10-07. Failing with a hint is
implemented; `auth` is not.

## Context

ACP's `authenticate` runs one of the login methods the agent offers in
`initialize`. Most log in through a terminal or a browser, which a headless
process has neither of. Not all: codex-acp's `api-key` reads
`OPENAI_API_KEY` from the environment. Nothing in ACP tells the two kinds
apart for the adapters brnr ships (neither uses ACP's typed methods), and
codex-acp's other method, `chat-gpt`, opens a browser.

## Decision

- By default the host doesn't run `authenticate`. A start that fails because
  the agent needs a login fails with the agent's methods and a hint to log in
  with the agent's own CLI first.
- `auth = "<method id>"` in a profile's headless part, or
  `start --auth <id>`, runs `authenticate` with that method after
  `initialize`, before the session opens. The user names a method they know
  works without a terminal; brnr never picks one (P4), and needs to know
  nothing about any adapter (P2). A method that tries a browser anyway runs
  into the start's timeout.
- With an editor, logging in is the editor's business.

## Considered

- Never authenticating (former decision 16 as it was): headless codex with
  an API key, in CI say, needed an interactive login first.
- brnr choosing a method that looks non-interactive: it can't tell, and a
  guess could open a browser on a server.
