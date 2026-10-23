# config

A session's settings: the agent's config options (ACP's
`configOptions`), and its v1 modes where it has them
([ADR 6.2](../adr/0006-settings-and-profiles.md)). For the config file, see
[configuration](configuration.md).

## `brnr config get`

Lists the config options the agent advertised and its v1 modes, each with
its value and its choices.

    brnr config get <session> [--mode] [--model] [--thought-level] [--option <o>]... [--json]

| Flag | Meaning |
|---|---|
| `--mode` | Only the option of category `mode`, else the v1 modes. |
| `--model` | Only the option of category `model`. |
| `--thought-level` | Only the option of category `thought_level`. |
| `--option <o>` | Only option `<o>`, by its id. Repeatable. |
| `--json` | Print the options as JSON. |

It lists only what the agent advertised: one it doesn't have fails
(`setting_invalid`).

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `options` | list of objects | The options, then the v1 modes. |
| `options[].option` | string or null | The option's id; null for the v1 modes. |
| `options[].category` | string or null | ACP's category (`mode`, `model`, `thought_level`, …); null if the agent gave none. |
| `options[].value` | string or boolean | The current value: a value id, or a boolean option's `true` or `false`. |
| `options[].choices` | list of objects | The values it takes, a group's too. A boolean option has none. |
| `options[].choices[].value` | string | A value id. |
| `options[].choices[].name` | string | Its name. |
| `options[].choices[].description` | string or null | Its description; null if the agent gave none. |
| `options[].name` | string or null | The option's name; null for the v1 modes. |
| `options[].description` | string or null | Its description; null if the agent gave none, and for the v1 modes. |

<!-- example: brnr config get $s --json; before: Idle -->
```json
{
  "session": "sess-1",
  "options": [
    {
      "option": "model",
      "category": "model",
      "value": "small",
      "choices": [
        {
          "value": "small",
          "name": "Small",
          "description": null
        },
        {
          "value": "large",
          "name": "Large",
          "description": null
        }
      ],
      "name": "Model",
      "description": null
    },
    {
      "option": null,
      "category": "mode",
      "value": "default",
      "choices": [
        {
          "value": "default",
          "name": "Default",
          "description": "Asks before edits"
        },
        {
          "value": "plan",
          "name": "Plan",
          "description": "Plans, doesn't edit"
        }
      ],
      "name": null,
      "description": null
    }
  ]
}
```

Text output: a table of each option, then its choices, the current one
marked `*`.

**Exit codes:** 0; 1 if it fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`setting_invalid`.

## `brnr config set`

Sets a session's settings, as `session new`'s are resolved: by category for
`--mode`, `--model` and `--thought-level`, by id for `--option`.

    brnr config set <session> [--mode <m>] [--model <m>] [--thought-level <l>] [--option <o>=<v>]... [--json]

| Flag | Meaning |
|---|---|
| `--mode <m>` | The option of category `mode`. `session/set_mode` only where the agent has v1 modes and no mode option. |
| `--model <m>` | The option of category `model`. |
| `--thought-level <l>` | The option of category `thought_level`. |
| `--option <o>=<v>` | Option `<o>`, by its id. Repeatable. |
| `--json` | Print what was set as JSON. |

One of them is needed. Two values for one setting fail, as does a mode,
model or thought level the agent has no option for, before anything is sent
(a usage error, or `setting_invalid`). Each is sent once, the mode first. An
`--option` id the agent hasn't advertised is sent too, for the agent to take
or refuse; a refusal names what was already set.

**Output** (`--json`):

| Field | Type | Meaning |
|---|---|---|
| `session` | string | The session. |
| `set` | list of objects | Each setting sent, in order. |
| `set[].option` | string or null | The option's id; null for a v1 mode. |
| `set[].category` | string or null | Its category; null if it has none. |
| `set[].value` | string | The value sent. |

<!-- example: brnr config set $s --model large --json; before: Idle -->
```json
{
  "session": "sess-1",
  "set": [
    {
      "option": "model",
      "category": "model",
      "value": "large"
    }
  ]
}
```

Text output: one `<option>=<value>` line for each setting sent, `mode=` for
a v1 mode.

**Exit codes:** 0 once all are set; 1 if one fails.
**Errors:** [those of a running session](errors.md#a-running-session),
`setting_invalid`, `agent_failed`, `experimental_disabled`,
`process_stopping`.
