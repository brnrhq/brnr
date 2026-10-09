# 58. A start's flags win over its profile, setting by setting

Accepted 2026-10-08. Implemented. Amends 28; resolves #64.

## Context

ADR 28 has `start --mode`, `--model`, `--set` and the profile's headless
`mode` and `config` applied before the first prompt, but not which wins when
they name the same setting. The code merged `--set` into the profile's
`config` by id, then applied the mode, the model and the config options in
that order. The model is the option whose category is `model`, whatever its
id, so a profile's `config = { model = "small" }` was applied after
`--model large`, and the session ran on `small`; the start succeeded and
nothing said so (P3). The mode had the same overlap with an option of
category `mode`. Which option is the model or the mode is known only once
the agent has opened the session and said what its options are.

## Decision

- A start has two sources of settings: its flags (`--mode`, `--model`,
  `--set`) and its profile's headless `mode` and `config`. They are resolved
  into one set of settings once the session is open, from the agent's
  config options, before anything is set.
- A config option whose category is `mode` is the mode, and one whose
  category is `model` is the model, whatever their ids (ADR 28). So
  `--model` and `--set <the model option's id>=…` are one setting, as are
  the profile's `mode` and its `config` for the mode option.
- The flags win, setting by setting: whatever they set, the profile's value
  for it is neither applied nor checked. What only the profile sets
  applies.
- Within one source, two different values for one setting fail the start
  before anything is set, naming both (P4, P7): `--model large and --set
  llm=small both set the model`, `--set effort=low and --set effort=high
  disagree`, `the profile's mode plan and its config approvals=default both
  set the mode`. The same value twice is one setting.
- Each setting is sent once: the mode first, then the model, then the other
  options by id. `status` has the values the agent reports after them; its
  `mode`, for an agent with no modes, is the mode option's value.

## Consequences

- `--set` for one id given twice with different values, which used to keep
  the last, now fails.
- A profile whose `mode` and `config` disagree about the mode fails every
  start that doesn't set the mode with a flag.

## Considered

- Merging by id in `brnr start`, as before, and applying the flags last:
  `start` doesn't know the agent's option ids, and an option applied twice
  changes the agent's state twice, with the profile's value briefly current.
- The last flag winning a conflict within the flags: a guess at what was
  meant (P4); the user can drop one.
