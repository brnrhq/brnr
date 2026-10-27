# skill

## `brnr skill`

The skill for agents that use brnr
([ADR 8.6](../adr/0008-the-command-line-and-compatibility.md#86-a-skill-for-agents-that-use-brnr)),
embedded in brnr, so that it is always the one for this brnr: print it, one
of its references, or install it.

    brnr skill [<reference> | install [--dir <dir>]...]

| Argument or flag | Meaning |
|---|---|
| (none) | Print `SKILL.md`, as Markdown. |
| `<reference>` | Print one of its references: `orchestrate`, `approvals`, `observe` or `setup`. Another word is a usage error. |
| `install` | Write the skill into each directory's `brnr/` subdirectory, and say where. |
| `--dir <dir>` | With `install`: this directory, in place of the defaults, `~/.claude/skills` (Claude Code) and `~/.agents/skills` (Codex). Repeatable. |

It has no `--json`. What it prints is not covered: it is text for agents to
read.

**Exit codes:** 0; 2 for a usage error; 1 if a file can't be written.
