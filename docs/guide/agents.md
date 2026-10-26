# For agents

An agent can run other agents through brnr: start headless workers, send
them work, wait for their turns, read their logs and bring their approvals
to you. brnr ships a skill that teaches it how
([skills/brnr](https://github.com/brnrhq/brnr/blob/main/skills/brnr/SKILL.md), [ADR 8.6](../adr/0008-the-command-line-and-compatibility.md#86-a-skill-for-agents-that-use-brnr)):
exact ids, waiting rather than polling, never approving what you didn't
delegate, and cleaning up what it starts.

```sh
brnr skill                         # SKILL.md; brnr skill orchestrate, approvals, observe, setup: its references
brnr skill install                 # into ~/.claude/skills/brnr (Claude Code) and ~/.agents/skills/brnr (Codex)
brnr skill install --dir .claude/skills   # or a project's, for each --dir
```

The skill is built into the binary, so it is always the one for the brnr
you run; install it again after upgrading. Every command in it runs as a
test, as the README's do.
