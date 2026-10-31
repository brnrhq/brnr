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

<a id="an-os-sandbox"></a>

**An OS sandbox.** A brnr process an agent in a sandbox starts runs in
the sandbox, and so does its agent. One started outside it, by you or
your editor, serves whoever reaches its socket, and opens sessions, sets
modes and answers approvals outside the sandbox. Keep the runtime
directory (`BRNR_DIR`; by default `$XDG_RUNTIME_DIR/brnr`, else
`$TMPDIR/brnr-<uid>`) out of the sandbox's reach, or run no brnr process
outside it.

The sandbox must also keep the agent from writing files that can be run
or loaded as code (executables, shared libraries, scripts), or brnr links,
anywhere, and from writing SSH configuration.
These are examples of required restrictions, not a complete sandbox policy.
The programs brnr starts inherit its OS sandbox; command rules cannot
replace that boundary. See the
[program-starting Gap](../threat-model.md#program-starts) for examples of
inputs that can change what runs.

**Command rules: allow what it needs, or deny what starts, prompts or
answers.** In the agent's own command rules (Claude Code's [permission
rules](https://code.claude.com/docs/en/permissions), Codex's
[rules](https://developers.openai.com/codex/exec-policy)), deny `brnr
session new`, `session resume`, `session fork`, `session delete`,
`prompt send`, `permission allow`, `elicitation accept`, `config set`,
`event notify` and `brnr acp`. Deny `brnr doctor`, `brnr remote check`
and every `--remote` command whole too. Even without `--fix`, doctor runs
the three `brnr-*-adapter` programs and non-npm
`muse-acp` with `--version`, resolved using adapter lookup (ADR 11.2);
the other npm adapters are identified from `package.json`. That argument
does not constrain what the selected executable can do. `remote check`
starts system SSH and the destination's doctor, including its adapter
probes.

Deny `session list` whole: with `--` or `--profile`, it starts an agent.
`session resume` and `session delete` start the one recorded for the
session even with neither. Deny the internal entry points `brnr host`
and `brnr _remote` whole: `host` runs the program its stdin request
names, and `_remote` runs any verb locally. `prompt send` matters even
with the rest denied: a session you or another agent started in a mode
that asks nothing does what any prompt says, and a fork of one may keep
its mode. Disruptive verbs such as `process stop` and `session close`
stop or delete brnr's own state but don't escalate; `skill install`,
`doctor --fix` and `session prune` also write or delete files. Bare
`skill install` writes to the default skill directories
(`~/.claude/skills` and `~/.agents/skills`); `--dir` selects other
directories.

Safer is to allow only the verbs that read and observe: `session status`,
`event log`, `event watch`, `event wait`, `permission requests` and `show`,
`elicitation requests` and `show`, and `config get`. This list has no
session-listing verb, so the agent gets session ids from whoever started
the sessions, or from `permission requests` / `elicitation requests`. A
short allow list limits the verbs, but does not keep their reads local
or prevent program starts: every `--remote` invocation starts SSH, even
on these read verbs. Command rules cannot make `doctor`, `remote check`
or any `--remote` command safe: they start programs whose choice and
behavior depend on the caller's environment and files. A deny-list policy
must deny these paths whole, including `--remote` on otherwise allowed
read verbs. What holds is the [OS sandbox](#an-os-sandbox), not a set of
command rules. The [threat model](../threat-model.md#program-starts) gives
known examples of the inputs involved, not an exhaustive list.

The allowed read verbs also take `--remote <host>` and read session
state on every destination the user's SSH reaches. Without `--remote`,
no command but `remote check` connects through brnr's transport: it runs
`ssh <target> brnr doctor`, with a version preflight. It requests
diagnostics without `--fix`, but executed adapters can have side
effects. `--remote` can come anywhere among a verb's arguments, so a
prefix rule can't exclude it. In Claude Code, add `Bash(brnr * --remote
*)` to `permissions.deny`: its [wildcard
rules](https://code.claude.com/docs/en/permissions#wildcard-patterns)
support `*` in the middle, so this matches both `brnr session status s
--remote h` and `brnr session status --remote h s`. Codex's prefix rules
can't express that exclusion. The Claude Code rule is best effort
against an agent trying to evade it: quoting, an absolute path or `env
brnr` can change the command text it matches. Network and key
restrictions can limit remote access, but do not stop these programs
from running local commands. The [OS sandbox](#an-os-sandbox) is the
boundary. A deny list must cover every spelling of a command; an allow
list still needs sandbox limits on remote reads and program starts.
