# 40. `release.sh`

Accepted (former decision 31); reviewed 2026-10-07. Implemented.
Amended by 47: `release.sh tag` also checks crates.io.

## Decision

Two steps, because main only changes through reviewed pull requests:
`release.sh <bump>` lints and tests main and opens the release pull request,
with the changes and the adapters' versions; `release.sh tag`, after the
merge, checks that CI passed on main, tags, follows the release workflow and
checks the tap. `release.sh notes` prints what the release pull request
would say.

## Considered

- One command that pushes the bump to main and tags: it skips review.
- Doing it all in a workflow (`workflow_dispatch`): it works, but is harder
  to run and debug than a script you can read.
