# Exit codes

**Covered** from 1.0.0: the code of each situation below. A minor release
can give a situation that has none a code of its own.

| Code | When |
|---|---|
| 0 | Success. `--help` or `-h`, the usage on stdout. |
| 1 | An operation failed: a session, process or profile that isn't there, an agent's error, a file that can't be read. Every [error code](errors.md) exits 1. |
| 2 | A usage error: what the command line alone shows wrong, with nothing done and the usage on stderr. |
| 3 | A command cut off for falling behind that couldn't catch up from the transcript. |
| 124 | A wait's `--timeout` passed, as timeout(1) exits. |

## By command

| Command | Result |
|---|---|
| Every command | 2 for a usage error. 0 for `--help` or `-h`. |
| An ordinary command | 0 on success; 1 for an operation failure. |
| `event wait`, `session new --wait`, `session resume --wait`, `prompt send --wait` | 124 when the wait's timeout passes. For a turn: 0 for `end_turn`, otherwise 1; a dropped message and the session closing or the agent exiting first fail too, 1. `event wait --for permission`, `elicitation` and `exit` exit 0 when the condition is met; `--for exit` fails, 1, when the connection fails and brnr can't see the process exit. |
| `event wait --for idle` | A session idle already, or one that closes while it waits, exits as its last turn ended. Idle is no turn running, nothing held, and no approval or elicitation waiting. |
| `event watch`, `event notify`, `event wait`, `session new --wait`, `session resume --wait`, `prompt send --wait` | 3 when the process cut the command off for falling behind and it couldn't catch up from the transcript: what it missed is an [`events_missed`](events.md#events_missed) note ([ADR 10.2](../adr/0010-bounded-resources.md#102-a-peer-cut-off-catches-up-from-the-transcript)). `event watch` prints it as its last event; the others say it on stderr. |
| `brnr acp` | 2 for a usage error; 1 for a config error or a process that fails to start; after the start, as the agent exited, or its signal. |
| `session new --foreground`, `session resume --foreground` | The agent's exit status; a failed start is nonzero. |
| `session delete` | 1 if the agent didn't delete the session (its error, or no answer), with `--purge` too, but for an agent that doesn't have it (`resource_not_found`): then 0, brnr's transcript deleted. 1 too if a file couldn't be deleted or recorded in. |
| `event notify` | Non-zero when its connection ends otherwise than with the process, or a signal stops it. |
| `doctor` | 1 if a check failed: read its checks for the cause. |

A failed command doesn't mean that the process or the agent stopped. A
wait's timeout doesn't cancel its turn. The detailed wait rules are in
[ADR 8.3](../adr/0008-the-command-line-and-compatibility.md#83-waiting-and-exit-status).
