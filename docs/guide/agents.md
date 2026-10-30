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

## brnr from a restricted agent

An agent that may run `brnr` may run any command, as you: brnr is not a
permission boundary between an agent and the agents it starts
([P13](../adr/0001-principles.md); the
[threat model's Gaps](../threat-model.md#gaps) trace each path). It can
start one in a mode that asks nothing (`--mode bypassPermissions`, Codex's
`agent-full-access`), answer that one's approvals (`permission allow`), or
name any program as the agent (`session new -- sh -c '…'`). The skill's rule
4 asks it not to; nothing enforces that, and an agent that reads a prompt
injection may not follow it. If an agent you restrict may run brnr, put the
restriction where brnr can't change it.

**Claude Code: managed settings.** Set
`permissions.disableBypassPermissionsMode` to `"disable"` in managed
settings (`/Library/Application Support/ClaudeCode/managed-settings.json`
on macOS, `/etc/claude-code/managed-settings.json` on Linux, or MDM), not in
your user settings:

```json
{ "permissions": { "disableBypassPermissionsMode": "disable" } }
```

The adapter then doesn't offer `bypassPermissions`, and a start that asks for
it fails. In `~/.claude/settings.json` the same line is no boundary: an agent
that can write files can change it, and Claude reads the started agent's
project settings, `.claude/settings.local.json` in the cwd the agent chose,
over the user's. Only the managed tier can't be overridden
([Claude Code's settings precedence](https://code.claude.com/docs/en/settings#settings-precedence)).

**Codex: managed requirements.** Codex's `requirements.toml`
(`/etc/codex/requirements.toml`, or MDM or cloud-managed requirements) holds
what a user, and so an agent, can't override
([managed configuration](https://developers.openai.com/codex/enterprise/managed-configuration)).
Leave `never` out of the approval policies and `danger-full-access` out of
the sandbox modes:

```text
allowed_approval_policies = ["untrusted", "on-request"]
allowed_sandbox_modes = ["read-only", "workspace-write"]
```

The Codex adapter still lists `agent-full-access`, but Codex refuses a turn
that asks for an approval policy or sandbox the requirements leave out (in
its source, at 0.160.1, `turn/start` answers `invalid thread settings
override`), so the session can't be made to ask nothing. Codex 0.138.0 and
later prefer `allowed_permission_profiles` for the sandbox, leaving
`:danger-full-access` out; check the requirements against the Codex you run.

**An OS sandbox: keep the runtime directory out of it.** A brnr process an
agent in a sandbox starts runs in the sandbox, and so does its agent. One
started outside it, by you or your editor, serves whoever reaches its
socket, and opens sessions, sets modes and answers approvals outside the
sandbox. Keep the runtime directory (`BRNR_DIR`; by default
`$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>`) out of the sandbox's
reach, or run no brnr process outside it.

**Command rules: allow what it needs, or deny what starts or answers.** In
the agent's own command rules (Claude Code's
[permission rules](https://code.claude.com/docs/en/permissions), Codex's
[rules](https://developers.openai.com/codex/exec-policy)), deny `brnr
session new`, `session resume`, `permission allow`, `elicitation accept`,
`config set` and `event notify`, and `brnr acp`, `session list` and `session
delete` with `-- <agent>`, which run that program. Safer is to allow only
the verbs that read and observe: `session list` with no `--`, `session
status`, `event log`, `event watch`, `event wait`, `permission requests` and
`show`, `elicitation requests` and `show`, and `config get`. A deny list has
to match every way of spelling a command; a short allow list doesn't.
