# 28. Mode, model and config options

Accepted (former decision 9); reviewed 2026-10-07. Implemented, telling
the editor included (ADR 4).
Resolves review item 6 with ADR 4.

## Context

Both adapters expose the mode through `session/set_mode`, and everything
else (model, effort, …) as config options (`session/set_config_option`)
with ACP's categories: `mode`, `model`, `thought_level`. codex-acp also has
`session/set_model`, its own legacy method (`LEGACY_SET_SESSION_MODEL_METHOD`
in codex-acp 2.1.1); it exposes the model as a config option too.

Former decision 9 allowed changes while an editor was attached because "the
agent tells the editor about the change". It doesn't, for these: the new
options come back in the response to the request, which is the host's, and
ACP answers the requester. The editor's model selector kept showing the old
model.

## Decision

- `brnr mode <session> [<mode>]` lists or sets the mode: `session/set_mode`,
  or the config option whose category is `mode` when the agent has no modes.
  `session/set_mode` is ACP v1's; v2 drops it, and a mode is then only the
  config option with category `mode`.
- `brnr config <session> [<option>=<value>...]` lists or sets config
  options.
- `brnr model <session> [<model>]` is `config` for the option whose category
  is `model`; with no such option, "the agent offers no model choice".
  Matched by category only, not by id (P4). `session/set_model` isn't used.
- `start --mode`, `--model`, `--set` and the profile's headless `mode` and
  `config` are applied before the first prompt; failing to apply one fails
  the start (P7).
- On an editor's session these are the experimental `settings` action, and
  the host tells the editor itself (`current_mode_update`, or
  `config_option_update` from the response's `configOptions`) (ADR 4).
- `brnr commands <session>` lists the agent's slash commands, sent as text.

## Considered

- Keeping `session/set_model` for older agents, refused on an editor's
  session for want of an update to send: every agent brnr ships has the model
  as a config option.
- Matching the option by id or category (former decision 9): an option that
  happens to have the id `model` is a guess; the category is ACP's.

## Tests

Run `cargo test --release adr_0028_`. Named claims and their assertions:

- [tests/cli.rs](../../tests/cli.rs)
  - `adr_0028_mode_lists_and_switches`.
  - `adr_0028_model_and_config`.
  - `adr_0028_model_is_the_option_of_category_model`.
  - `adr_0028_mode_as_a_config_option`.
  - `adr_0028_start_applies_mode_and_model_before_the_prompt`.
  - `adr_0028_commands_lists_the_agents_commands`.
