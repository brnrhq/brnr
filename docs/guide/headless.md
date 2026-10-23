# Headless sessions

```sh
brnr session new --cwd ~/work/project --prompt "fix the failing tests" -- brnr-claude-adapter
brnr session new --mode plan --model opus --prompt - < task.md      # set up before the first prompt
brnr session new --option effort=high --prompt - < task.md          # any of the agent's config options, likewise
brnr session resume $s                                              # carry on a session that ended
brnr session new --auth api-key --prompt - -- brnr-codex-adapter < task.md   # log in first (OPENAI_API_KEY)
brnr session new --foreground --prompt - -- brnr-codex-adapter < task.md     # in the foreground; Ctrl-C stops it
```

`session new` opens a new session, and `session resume` an existing one;
each starts a process for it, and they take the same flags. A start is
atomic. It reads the prompt and attachments first, hands the process
everything in one request, and the start commits once the session is open
with its mode and config options set; only then does the agent get the
prompt. Until then, a step failing, the timeout (120 seconds,
`BRNR_START_TIMEOUT`) or the command being interrupted stops the process, and
the prompt is never sent. A start that fails ends its error with the agent's last
lines on stderr (`claude CLI not found`).

`--mode`, `--model`, `--thought-level` and `--option` win over the profile's
`mode`, `model`, `thought_level` and `options`, setting by setting. The model
is the agent's config option of category `model`, whatever its id, so
`--model large` replaces a profile's `options = { model = "small" }`; the mode
and the thought level (category `thought_level`) likewise. Two different
values for one setting from the flags (`--model large --option model=small`),
or from the profile, fail the start before anything is set, as does a
setting the agent has no option for.

`--stop-when-idle <s>` closes a session once it has been idle that many
seconds (no turn running, nothing held, no approval or elicitation waiting),
counting from the start; `0` is as soon as it is. Its last session isn't closed: the
process stops instead. `--response-timeout <s>` answers an approval nobody
answered in that many seconds as `response_timeout` does: with its
`reject_once` option, or, without exactly one, by cancelling its turn, which
answers every request pending in the session `cancelled`; an elicitation, by
answering it `cancel`. Each wins over the profile's `stop_when_idle` and
`response_timeout`.

`--foreground` keeps the session in the terminal, for a supervisor such as
systemd or a container. It shows the session's events on stdout (`--json`: as
JSON lines; `--quiet`: not) and brnr's messages and the agent's own stderr on
stderr, and exits as the agent did, non-zero if the start failed. Ctrl-C stops
the session (twice kills the agent). A display that can't keep up skips
events, saying how many (`… 120 events not shown`); stdout closing
(`| head -1`) ends the display, not the session.

`session resume` uses the agent's `session/resume` (or `session/load`), in the
session's cwd, with the agent and profile it last had (where it was last
served, the newest record of all its files), and appends to the same
transcript. `--profile p` runs p's agent instead, when p names one, and
`-- <agent>` runs that agent, whatever the profile. A load replays the session's history: brnr records it, as
events marked `replayed`, only when it has no transcript of the session (one
it has never seen, whose transcript is gone, or whose loads all failed,
leaving only their openings), so repeated loads don't record it twice. A
`history` event says how many updates were replayed and whether they were
recorded. An editor's `session/load` through `brnr acp` is
recorded the same way, and the editor gets the replay as the agent sent it.
Its loads of a session that overlap share one replay and one `history`
event, with how many `loads`; a recorded replay then has the history once per
load, each copy marked. brnr's transcript has only what happened through
brnr: turns taken in the agent's own client, or with `log = "off"`, aren't
in it, and a later load doesn't add them.

With no editor attached brnr is the agent's client: approvals wait for
`brnr permission allow`/`reject` or a bridge, elicitations for `brnr elicitation
accept`/`decline`/`cancel` or a bridge, and anything else is answered with
"method not found". How much the agent asks is the agent's own setting: its
mode (`--mode`, `brnr config set --mode`). brnr doesn't log in for you. If the agent needs
a login, the start fails with the agent's methods: log in with the agent's own
CLI first, or name a method that needs no terminal with `--auth <id>` (`auth`
in the profile), such as codex-acp's `api-key`.
