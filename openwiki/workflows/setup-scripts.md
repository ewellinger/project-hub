---
type: workflow
title: Per-role setup scripts
description: How a role's `setup` script in hub.json is registered, which worktrees run it, how it runs outside the hub lock after feature start, add, and start-review, where its log goes, how the CLI and dashboard report it, and how `hub feature setup` reruns it.
tags: [workflow, setup, worktrees, manifest, dashboard]
verified:
  - by: openwiki/0.9.0
    at: 2026-11-09T20:44:31.979Z
sources:
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-cfc759a7da3457809607f5ab
    resource: repo://src/manifest.rs
  - id: openwiki-source-482a254fc9f19462c83aa694
    resource: repo://src/names.rs
  - id: openwiki-source-551bfbd71774ea4fd0b8f063
    resource: repo://src/setup.rs
  - id: openwiki-source-7bdce6d8095dc0966e3f6005
    resource: repo://src/ui/app/mod.rs
  - id: openwiki-source-d36a1897ad2613d7d98e64cb
    resource: repo://src/ui/worker.rs
  - id: openwiki-source-e89fdc875b2c55e64982d891
    resource: repo://tests/setup.rs
generated: { by: "claude-code", at: "2026-11-09T20:44:31.979Z" }
---

# Per-role setup scripts

A fresh worktree is often not ready to use: a uv project needs `uv sync`, a
Node project needs `npm ci`, a local `.env` must be copied in. A role's
optional `setup` field in `hub.json` names a script that hub runs inside each
worktree it creates for that role. The logic lives in `src/setup.rs`; the CLI
wiring is in `src/main.rs`, and the dashboard's in `src/ui/worker.rs` and
`src/ui/app/mod.rs`. The agent-facing contract ships with the bundled skill
as `skills/hub/setup-scripts.md` (see
[Bundled agent skill](../integrations/agent-skill.md)).

## Registering a script

- `hub repo add ROLE CLONE --setup PATH` and `hub repo set ROLE --setup PATH`
  record the path; `hub repo set ROLE --setup ""` clears it
  ([Hub init and repo registration](hub-and-repo-management.md)).
- `names::validate_setup_path` accepts only a relative path with a file name
  and no `..` or root component (`scripts/setup.sh` and `./scripts/setup.sh`
  pass; `/abs.sh`, `../x.sh`, `a/../b.sh`, `""`, and `.` fail). The same check
  runs in `Manifest::validate` on every load, so a hand-edited bad path makes
  `hub.json` malformed.
- Nothing checks that the file exists, is executable, or is committed, and
  `repo add/set` commit only `hub.json`. The path is resolved against the
  hub's **main clone** (`hub.root`), so a script that exists only on a
  feature's hub branch is not found.

## Which worktrees run it

`setup::select_changes` picks, in **manifest order**, the roles that:

1. are among the roles the command touched,
2. have a `setup` in the manifest,
3. have an open change in the feature, and
4. have a worktree that hub created (`Owner::Hub`); an adopted worktree
   qualifies only when the caller asks for it.

A change living in the main clone (no worktree), a role missing from the
manifest, and a frozen reference checkout never qualify.

## When it runs

| Trigger | Roles | Adopted worktrees |
|---|---|---|
| `hub feature start` | the `--repo` roles | no |
| `hub feature add` | the added role | no |
| `hub feature start-review` | the reviewed role | no |
| Dashboard Start and Add | the same as the CLI | no |
| `hub feature setup` | every open change of the feature | no |
| `hub feature setup ROLE` | that role | yes |

`--no-setup` on `start`, `add`, and `start-review` skips the run. The
dashboard has no equivalent and no rerun key.

The ops that create worktrees know nothing about setup. After the locked op
returns and its lines are printed, `run_setup` in `src/main.rs` reloads
`hub.json` (the op read it fresh under the lock, so the setup must too),
rereads the saved feature record, builds the jobs, and runs them one at a
time. Because this happens **after the hub lock is released**, a slow script
never makes another hub command fail. The dashboard worker does the same
after sending the action's `Done` event, on a `Hub` copy kept for after the
lock.

## Running a job

Each `setup::Job` carries the absolute script path (`hub.root` joined with the
manifest path), the label as `hub.json` spells it, the worktree as working
directory, the log path, and the environment hub adds on top of the inherited
one:

| Variable | Value |
|---|---|
| `HUB_ROLE` | the role |
| `HUB_FEATURE` | the feature |
| `HUB_BRANCH` | the change's branch |
| `HUB_WORKTREE` | the worktree (also the working directory) |
| `HUB_BASE` | the role's main clone (a path, not a branch) |
| `HUB_ROOT` | the hub's main clone |

`setup::run` checks that the script is a regular file with an executable bit,
creates the log directory, then opens
`<git common dir>/hub/setup/<feature>/<role>.log` and takes a non-blocking
`fd-lock` write lock on it **before** truncating, so a second run of the same
role (the CLI and the dashboard at once) neither wipes the first run's log nor
shares its worktree. It then executes the script directly, with no shell of its own
(a script without a shebang runs under `/bin/sh`), with stdin from
`/dev/null` and stdout and stderr both written to the log. There is no timeout: hub waits until
the script exits. A background process the script leaves behind does not keep
the lock, since the guard unlocks explicitly on drop.

Outcomes:

| `Status` | CLI line |
|---|---|
| `Ok` | `ROLE: setup ok (1.2s)` |
| `Failed(code)` | `ROLE: setup failed (exit N) after 1.2s; log <path>`, then the last 20 log lines, indented |
| `Signaled` | `ROLE: setup killed by a signal after 1.2s; log <path>`, then the log tail |
| `CouldNotStart(reason)` | `ROLE: setup could not start: <reason>` (not a file, not executable, log not writable, or `setup already running for ROLE`) |

The CLI prints `Running setup for ROLE: PATH` before each job. The log path is
shortened to `~/…` under the home directory. Every failure to start is a
`CouldNotStart` value, never a panic.

## Failure semantics

A setup failure never rolls back the feature and never changes the exit code
of `start`, `add`, or `start-review`: the worktree and record are complete,
and the user reads the log and reruns. Only `hub feature setup` reports
failure through its exit status: it returns `setup failed for ROLE, …`
(exit 1) when any job did not end `Ok`.

## `hub feature setup [ROLE]`

`setup::rerun_jobs` requires an open feature, then:

- **Without a role**, it builds jobs for every open change's hub-created
  worktree. When there are none it prints `no hub-created worktree in feature
  'NAME' has a setup script` and succeeds.
- **With a role**, it refuses a role without a `setup` (usage error naming
  `hub repo set ROLE --setup PATH`), a role without an open change, and a
  change checked out in the main clone (`run <script> there yourself`), since
  that is the user's own checkout. An adopted worktree is accepted.

It takes no hub lock. Each run rewrites the role's log.

## In the dashboard

After a successful Start or Add, the worker sends an `Event::Setup` with
`SetupState::Running` before each script and `SetupState::Done(outcome)` after
it. `App.setups` keeps one `SetupRow` per script. While the action's result
popup is open, each event redraws it with the action's lines plus one line per
script (running, or its result with up to five log lines after a failure) and
marks it as an error when a script failed; after the popup is dismissed, a
finished script reports its one-line result in the footer. `q` and `Esc`
refuse to quit while a script runs (`setup running for ROLE; Ctrl-C quits
anyway`), since quitting would orphan it; setup does not mark the dashboard
busy, so other actions still run. See
[Dashboard flows and overlays](../ui/dashboard-flows-and-overlays.md).

## Cleanup

`hub feature remove` deletes the feature's `setup/<feature>/` log directory
after the record, ignoring failure
([Review, merged, dropped, finish, and remove](review-merged-finish.md)).

## Writing a script

The bundled `setup-scripts.md` is the authoritative guide. In short: make it
executable with a shebang and `set -eu`; make it idempotent, since reruns
land in a worktree with local state; never overwrite per-worktree files such
as `.env` (copy from `$HUB_BASE` only when absent); pin tools explicitly,
because the inherited `PATH` differs between a terminal, an agent sandbox, and
the dashboard; never wait on input or an untimed network call; and commit the
script on `main` in the hub's main clone together with the `repo set --setup`
commit.

## Tests

- `src/setup.rs` unit tests: selection order and filters, adopted worktrees
  only on request, environment and working directory, the log lock refusing a
  concurrent run without wiping its log, a leftover background process,
  exit codes and stderr, missing / non-executable / directory scripts, an
  unwritable log, lossy bounded tails, and each result line.
- `tests/setup.rs`: start, add, and start-review running the script; a failing
  or unstartable script still succeeding the command; `--no-setup`; adopted and
  frozen worktrees skipped; `feature setup` with and without a role, exit 1 on
  failure, an adopted worktree accepted, the main clone and an unconfigured
  role refused.
- `tests/repo.rs`: recording and clearing `--setup`; `src/manifest.rs`: path
  validation.

## Related pages

- [Feature start and add](feature-start-and-add.md)
- [Reviewing a merge or pull request](start-review.md)
- [Hub init and repo registration](hub-and-repo-management.md)
- [Environment and configuration](../configuration/environment-and-config.md)
- [Dashboard flows and overlays](../ui/dashboard-flows-and-overlays.md)
- [Bundled agent skill](../integrations/agent-skill.md)
- [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)
