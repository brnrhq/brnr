# 6. Settings and profiles

Draft 2026-10-20. Implemented. Factor: F3 (ADR 1). Brings together
former ADRs 33, 58, 28, 31 and 32 (docs/adr/README.md maps them).

How a brnr process is set up: the profiles in `config.toml`, a start's
flags over its profile, the agent's mode, model and config options, MCP
servers for headless sessions, and files and images in a message. These are
what brnr adds around ACP so that a headless session can run without an
editor (F3), with each setting's role in the config itself.

## 6.1 Profiles, and a start's flags over them

A profile has three parts, by role (P7, P12):

```toml
[profiles.work]                     # every process of the profile
agent = ["brnr-claude-adapter"]
log = "all"                         # "events", or "off" (ADR 5.1)
strict = false                      # stable ACP only (ADR 2.1)

[[profiles.work.bridges]]
command = "brnr"
args = ["event", "notify", "--stdin", "--",
        "sh", "-c", "terminal-notifier -title \"$BRNR_TITLE\" -message \"$BRNR_TEXT\""]

[profiles.work.headless]            # brnr session new and session resume
cwd = "~/work/project"
mode = "plan"
model = "opus"
thought_level = "high"
options = { effort = "high" }       # config options by id
response_timeout = 600
stop_when_idle = 600
auth = "api-key"

[[profiles.work.headless.mcp_servers]]
name = "github"
command = "github-mcp-server"
args = ["stdio"]
env = { GITHUB_TOKEN = "…" }

[profiles.work.editor]              # brnr acp
experimental = ["send", "permission"] # actions on the editor's session (ADR 7.1)
client_capabilities = ["fs"]        # "fs", "terminal" (ADR 2.7)
echo_as = "tool_call"               # or "user_message_chunk" (ADR 7.2)
features = ["shared_sessions"]      # process management (ADR 3.10)
```

- At the top, what every process of the profile uses: `agent`, `log`,
  `strict`, `bridges`.
- Under `headless`, what only `brnr session new` and `brnr session resume`
  use: `cwd`, `mode`, `model`, `thought_level`, `options`,
  `response_timeout`, `stop_when_idle`, `auth` and `mcp_servers` (ADR 3.8,
  4.5, 4.7, 6.2, 6.3). `response_timeout` is the one timeout for the
  user's responses, approvals and elicitations alike (ADR 4.5).
- Under `editor`, what only `brnr acp` uses: `experimental` (ADR 7.1),
  `client_capabilities` (ADR 2.7), `echo_as` (ADR 7.2) and `features`
  (ADR 3.10). The three lists default empty; `echo_as` defaults to
  `"tool_call"`.
- A key in the wrong part, an unknown key or name, a headless `config`
  key, and the flat layout fail to load (P7, P9). `brnr doctor` says which
  and where.

One name, one meaning, one spelling in config too (#154, ADR 8.1):

- `log` takes strings only: `"all"` (the default), `"events"` or `"off"`.
  `"off"` replaces `false`, and `true` and `false` fail to load, as
  layout errors do: `brnr doctor` names the profile and the value.
- A command in config is one shape, a program and its arguments:
  `command`, a string, and `args`, a list, with `env` where it takes one.
  An MCP server keeps this form, which is ACP's `McpServerStdio` (F1,
  ADR 6.3), and a bridge takes it too, in place of its argv list:
  `command = "brnr"`, `args = ["event", "notify", "--stdin", "--", …]`.
  A list `command` fails to load, saying that its arguments go in `args`.
  `agent` stays a list, the agent's argument vector, as `-- <agent>
  [args...]` is on the command line: another name, with one meaning.
- A duration is in whole seconds, with no unit in its name
  (`response_timeout`, `stop_when_idle`), as on the command line and in
  `--json` (ADR 8.1).

A start (`session new` or `session resume`, with `--pid` too) has two
sources of settings: its flags (`--mode`, `--model`, `--thought-level`,
`--option <o>=<v>`) and its profile's headless `mode`, `model`,
`thought_level` and `options`.

- They are resolved into one set of settings once the session is open,
  from the agent's config options, before anything is set.
- A config option whose category is `mode` is the mode, one whose category
  is `model` the model, and one whose category is `thought_level` the
  thought level, whatever their ids (ADR 6.2). So `--model` and
  `--option <the model option's id>=…` are one setting, as are the
  profile's `model` and its `options` entry for the model option; the same
  holds for the mode and the thought level.
- The flags win, setting by setting: whatever they set, the profile's
  value for it is neither applied nor checked. What only the profile sets
  applies. `--response-timeout` wins over the profile's
  `response_timeout` the same way.
- Within one source, two different values for one setting fail the start
  before anything is set, naming both (P4, P7): `--model large and
  --option llm=small both set the model`, `--option effort=low and
  --option effort=high disagree`, `the profile's mode plan and its options
  approvals=default both set the mode`. The same value twice is one
  setting.
- A v1 mode the agent doesn't list fails before anything is set.
- Each setting is sent once: the mode first, then the model, then the
  thought level, then the other options by id. `session status` has the
  values the agent reports after them; its `mode`, for an agent with no
  modes, is the mode option's value.
- `config set` is resolved the same way, its flags the only source.

**Why.** In a flat profile, `brnr acp --profile work` silently ignored the
headless-only keys, and brnr can't warn about it, since the proxy's stderr
is the agent's (P1); the editor also has settings of its own. Putting the
role in the config makes a misplaced key an error (P7). Which option is the
model or the mode is known only once the agent has opened the session and
listed its options, so the flags and the profile can only be merged then.
Merging by id earlier let a profile's model option override `--model`, and
the start succeeded without saying so (P3). `log` mixes strings and a
boolean, and `command` is a string in an MCP server and a list in a
bridge, in one file (#154): a key with two types is one more thing to get
wrong (F5, P5). ACP has the MCP server's form, so brnr's own takes it.

**Consequences.**

- `--option` for one id given twice with different values fails.
- A profile whose `mode` and `options` disagree about the mode fails every
  start that doesn't set the mode with a flag.

**Considered.**

- A flat profile, with the headless-only keys marked in the README and
  `doctor`: an editor's profile would still carry settings it silently
  ignores.
- Merging the flags into the profile's options by id in the CLI, and
  applying the flags last: the CLI doesn't know the agent's option ids,
  and an option applied twice changes the agent's state twice, with the
  profile's value briefly current.
- The last flag winning a conflict within the flags: a guess at what was
  meant (P4); the user can drop one.
- `log = false` beside the strings, as a boolean for "nothing": one key
  with two types. `"none"` for it: `"off"` says that logging is off, not
  that there are no records.
- A bridge's `command` and the MCP server's both argv lists: the MCP form
  is ACP's (F1), passed on in `session/new` as it is.
- Keeping both shapes under one name (#154): a key whose type depends on
  where it is.

## 6.2 Mode, model and config options

- `brnr config get <session>` lists every config option with its value
  and choices, and the agent's v1 modes where it has them. `--mode`,
  `--model`, `--thought-level` and `--option <o>` narrow it (ADR 8.1).
- `brnr config set <session>` sets options by category with `--mode`,
  `--model` and `--thought-level` (`mode`, `model`, `thought_level`), and
  by id with `--option <o>=<v>`, resolved as ADR 6.1 resolves a start's.
- The mode is the config option whose category is `mode`, even where the
  agent also has v1 modes. `session/set_mode` is sent only to an agent
  with v1 modes and no mode option. `session/set_mode` is ACP v1's; v2
  drops it, and a mode is then only the option of category `mode`.
- The model is the config option whose category is `model`, matched by
  category only, not by id (P4). With no such option, `config get
  --model` fails, "the agent offers no model choice", and a model set by
  `config set` or a start fails before anything is sent.
  `session/set_model` isn't used. The thought level is matched by its
  category the same way.
- A value is sent as the option's advertised `type` has it. A `boolean`
  option takes `true` or `false`, sent as `type: "boolean"` and a JSON
  boolean; any other value for it fails before reaching the agent (P7). A
  `select` option, or one the agent hasn't advertised, gets the value as a
  value id (a string), with no `type`.
- A start's `--option` and the profile's `options` are sent the same way,
  once ADR 6.1 has merged them. A value one of them can't take fails the
  start before any is sent.
- An agent offers boolean options only to a client that advertises them
  (`clientCapabilities.session.configOptions.boolean`). An editor can, and
  then an editor's session has them; brnr's own headless client doesn't.
- A start's settings are applied before the first prompt. Failing to apply
  one fails the start (P7).
- On an editor's session, `config set` is the experimental `config`
  action, and the host tells the editor itself: `current_mode_update`, or
  `config_option_update` from the response's `configOptions` (ADR 7.1).
- `brnr prompt commands <session>` lists the agent's slash commands. They
  are invoked by sending them as text.

**Why.** Both adapters expose the mode through `session/set_mode` and
everything else (model, effort, …) as config options with ACP's
categories. codex-acp also has `session/set_model`, its own legacy method,
and exposes the model as a config option too. The agent doesn't tell the
editor of a change brnr makes: the new options come back in the response,
which is the host's, and ACP answers the requester. So the host tells the
editor, or its model selector keeps showing the old model.

**Considered.**

- Keeping `session/set_model` for older agents, refused on an editor's
  session for want of an update to send: every agent brnr ships has the
  model as a config option.
- Matching the option by id or by category: an option that happens to
  have the id `model` is a guess; the category is ACP's.

## 6.3 MCP servers for headless sessions

- `[[profiles.<p>.headless.mcp_servers]]`, each with a `name` and either
  `command`, `args` and `env` (stdio), or a `url` with `type = "http"` or
  `"sse"` and optional `headers`. They are passed in the `session/new`,
  `session/resume`, `session/load` and `session/fork` requests the host
  sends.
- HTTP and SSE servers are refused at start if the agent doesn't advertise
  support for them (P7). codex-acp 2.1.1 has no SSE.
- With an editor, the MCP servers are the editor's: its own `session/new`
  carries them, and the profile adds none (P1).
- Their `env` and `headers` values are redacted in what brnr records
  (ADR 9.1).

**Why.** A headless session has no editor to give the agent its MCP
servers, so the profile does. An editor's session already has them, and
adding to them would change what the editor sent (P1).

## 6.4 Files and images in a message

- `--file <path>`: a `resource_link` to the file; the agent reads it
  itself. ACP always allows it.
- `--image <path>`: an `image` block, base64-encoded, refused unless the
  agent advertises image prompts (P7).
- Both are taken by `prompt send`, `session new` and `session resume`, and
  read by the CLI; for a start, before the process is launched (ADR 3.4).

**Why.** A link costs nothing and needs no capability. An image can only
be sent as content, and an agent without image prompts can't take it, so
the request fails up front (P7).

**Considered.**

- Embedding the file's contents (`resource`): it needs the agent's
  `embeddedContext` capability (both adapters have it), and duplicates
  what the agent can read anyway.

## Tests

Run `cargo test --release adr_0006_`. Named claims, by section:

- 6.1: [tests/cli.rs](../../tests/cli.rs):
  `adr_0006_profile_layout_errors_say_where`,
  `adr_0006_start_flags_win_over_the_profile` (`--model` over a profile's
  model option, with the id `model` and with another; the mode likewise,
  either way round; each setting sent once; `session status` has the
  value that won), `adr_0006_start_settings_that_disagree_fail` (two
  values for one setting from the flags, or from the profile, fail before
  anything is set; a flag settles the profile's; the same value twice is
  one), `adr_0006_config_names_are_spelled_one_way` (`log = false` and
  `log = true` fail to load, and `doctor` names them; `log = "off"` keeps
  no transcript; a bridge of `command` and `args` starts, and one with a
  list `command` fails to load).
- 6.2: [tests/cli.rs](../../tests/cli.rs):
  `adr_0006_mode_lists_and_switches`, `adr_0006_model_and_config`,
  `adr_0006_model_is_the_option_of_category_model`,
  `adr_0006_mode_as_a_config_option`,
  `adr_0006_start_applies_mode_and_model_before_the_prompt`,
  `adr_0006_commands_lists_the_agents_commands`;
  [tests/headless.rs](../../tests/headless.rs):
  `adr_0006_a_boolean_option_is_set_as_a_boolean`;
  [src/host/state.rs](../../src/host/state.rs):
  `adr_0006_a_config_value_is_sent_as_its_option_type_has_it`;
  [src/host/requests.rs](../../src/host/requests.rs):
  `adr_0006_a_start_refuses_a_boolean_it_cant_send_before_sending_any`.
- 6.3: [tests/cli.rs](../../tests/cli.rs):
  `adr_0006_mcp_servers_reach_the_agent`.
- 6.4: [tests/cli.rs](../../tests/cli.rs):
  `adr_0006_files_and_images_go_with_the_prompt`.
