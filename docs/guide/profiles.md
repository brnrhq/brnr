# Profiles

`~/.config/brnr/config.toml`:

```toml
[profiles.work]                     # every process of the profile
agent = ["brnr-claude-adapter"]
log = "all"                         # "events": no raw ACP; "off": no transcripts
strict = false                      # stable ACP only

[[profiles.work.bridges]]
command = "~/bin/slack-bridge"      # a program and its arguments, as an MCP server's
args = ["--channel", "#agents"]
events = ["permission_request", "turn_ended"]   # omit for every event but acp

[profiles.work.headless]            # brnr session new and resume
cwd = "~/work/project"
mode = "plan"                       # the agent's mode, model, thought level
model = "opus"                      #   and config options by id, set before
thought_level = "high"              #   the first prompt; the flags win
options = { effort = "high" }
response_timeout = 600              # reject once (else cancel the turn) what
                                    #   nobody answered in 10 minutes
stop_when_idle = 600                # close a session idle 10 minutes
# auth = "api-key"                  # a login method to run first (codex-acp's)

[[profiles.work.headless.mcp_servers]]   # for sessions brnr opens; an editor brings its own
name = "github"
command = "github-mcp-server"       # or url = "https://…", type = "http" | "sse", headers
args = ["stdio"]
env = { GITHUB_TOKEN = "…" }

[profiles.work.editor]              # brnr acp
experimental = ["send", "permission"] # actions on the editor's session
client_capabilities = ["fs"]        # the editor's that reach the agent: "fs", "terminal"
echo_as = "tool_call"               # or "user_message_chunk": how a message is shown
features = ["shared_sessions"]      # process management
```

Without `--profile`, the `default` profile applies, if there is one. A key in
the wrong part, an unknown key and an unknown name fail to load, saying where
the key goes; `brnr doctor` checks every profile.

The file names the commands brnr runs and can hold secrets, so brnr reads it
only if it is yours and only you can read and write it, and its folder is
yours and only you can write it: `chmod 600` the file, `chmod 700` the
folder. Otherwise every command that reads it fails, saying which path, why,
and the `chmod` or `chown` that fixes it; brnr doesn't change it for you. A
symlink is followed, and the file it points at, and its folder, are checked
([ADR 9.4](../adr/0009-security-and-privacy.md#94-the-config-file-is-read-only-if-only-you-can-change-or-read-it)).

# Bridges

A bridge is any process that speaks brnr's JSON-lines protocol: one started by
the brnr process from the profile (requests on its stdout, events on its
stdin, its stderr in the host log; a bare command found next to `brnr` first,
as an agent's is), or anything that connects to the control socket. Its
environment has `BRNR_PID` and `BRNR_SOCKET`.

Requests: `status`, `logged`, `send`, `cancel`, `queue`, `subscribe`,
`was_cut_off`, `pending`, `allow`, `reject`, `set_config`, `fork`, `new`,
`resume`, `close`, `stop` (see the [request and response reference](../reference/bridge.md)); a failure is `{"ok": false, "error": {"code", "message"}}`, its code one of [these](../reference/errors.md#codes); those about a session name it by
its exact id, and those that act on an editor's session are
[experimental](editor.md#experimental-actions), as the CLI's are. Events:
[as above](events.md#events). A started bridge gets them from the process's start.
A request, or a field of one, that the process doesn't have is refused
(`command_unknown`), never done without it.

A process can be an older brnr than the one you run: an editor's lives as
long as the editor's session, past `brew upgrade brnr`. `status` says which
it is (`brnr`, its version, and `protocol`, what its socket speaks), and a
command that fails because of the difference says so, and how to restart
the process: `brnr process stop <pid>` and `brnr session resume <session>`
for a headless one, the agent restarted in its editor for an editor's.

A started bridge lasts until its process exits; its stdout closing only means
it has no more requests. When the process stops, the bridge gets the last
events (`process_exited`), then its stdin closes; one still running 2 seconds later
gets SIGTERM.

# Environment

| Variable | Default |
|---|---|
| `BRNR_HOME` | `~/.brnr` (transcripts and session locks) |
| `BRNR_CONFIG` | `$XDG_CONFIG_HOME/brnr/config.toml`, else `~/.config/brnr/config.toml` (brnr refuses one others can read or write) |
| `BRNR_DIR` | `$XDG_RUNTIME_DIR/brnr`, else `$TMPDIR/brnr-<uid>` (sockets and metadata; brnr refuses one others can use) |
| `BRNR_START_TIMEOUT` | `120`: seconds a start has to commit |
