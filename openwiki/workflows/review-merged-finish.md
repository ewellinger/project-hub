---
type: workflow
title: Review, merged, set-base, and finish
description: The lifecycle commands that advance or close a feature's changes — `feature review`, `feature set-base`, `feature merged`, `feature finish` — including the dirty-worktree (exit 20) and in-use (exit 21, via lsof) refusals, adopted-worktree confirmation, tmux handling, irreversible-first ordering, and leftover-directory warnings.
tags: [workflow, review, merged, finish, set-base, safety, lsof]
sources:
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-2e469aed167f415af0005649
    resource: repo://src/ops/finish.rs
  - id: openwiki-source-663ac1b065deb6f67a90cc27
    resource: repo://src/ops/merged.rs
  - id: openwiki-source-bb184e2cea83bf2ff3501c2a
    resource: repo://src/ops/review.rs
  - id: openwiki-source-023f2a1cda1b07809c0aa015
    resource: repo://src/procs.rs
generated: { by: "claude-code", at: "2026-10-12T18:18:24.526Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T18:18:24.526Z
---

# Review, merged, set-base, and finish

These commands move changes through `working → review → merged` and close a
feature. All run under the hub lock, require an **open** feature, and never
delete, merge, or push a branch. See
[Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
for the model.

## `hub feature review ROLE [URL]` (`src/ops/review.rs`)

Requires an open change for the role. Fetches the role's clone, then:

- sets the stage to `review`;
- records `URL` as `review_url` when given (an omitted URL keeps any existing one);
- sets `origin_seen` if the branch is on origin (it is never cleared), which
  later lets `status` say "gone from origin; run hub feature merged ROLE";
- saves the record; prints a note when the branch is not on origin yet.

## `hub feature set-base ROLE BRANCH` (`src/ops/set_base.rs`)

Record-only: changes the open change's `base` and recomputes `base_sha` as the
merge base of the branch and `origin/BRANCH`. It refuses `origin/`-prefixed
input, a base missing from origin, a missing local branch, and unrelated
histories, and never checks out, rebases, or touches git's upstream. It also
refuses a pinned open change ("unfreeze first"), since the pin was taken from
the current base. Details
are on the [concepts page](../concepts/features-changes-and-stages.md#bases).

## `hub feature merged ROLE [--force]` (`src/ops/merged.rs`)

Closes one role's open change:

1. **Dirty check first** (unless `--force`): a dirty worktree fails with
   `HubError::Dirty` (exit 20) **before** anything else, because the next step
   kills processes.
2. **Re-point tmux** (`session::repoint_window`): if the session has a window
   named after the role, it is respawned in the main clone so its shell (and
   anything started from it) cannot block removal.
3. **In-use check** (unless `--force`): `refuse_in_use` fails with
   `HubError::InUse` (exit 21) when any process has its working directory
   inside the worktree.
4. **Close** with `close_change`:
   - no worktree (branch lives in the main clone) → nothing to remove;
   - worktree directory already gone → nothing to remove;
   - adopted worktree → requires `--force` or a yes from the confirm callback
     (CLI asks on a terminal, otherwise declines; a decline aborts with
     "no worktree was removed. Rerun with --force");
   - otherwise `git worktree remove [--force]`.
5. Set the stage to `merged` with `merged_at`, drop the role's pin if the
   change was pinned (printing `pin at <sha> dropped`), save, and rewrite the
   workspace (warning on failure).

A later `feature add ROLE` opens a follow-up change, defaulting to a `-<n>`
branch name.

## `hub feature finish [--force]` (`src/ops/finish.rs`)

Takes the whole feature down:

1. **Refuse inside its own session**: if the command runs in the tmux session
   it would kill, it fails and tells you to run
   `hub feature finish --feature NAME` from another terminal.
2. **Pin check, then dirty check** (unless `--force`): every frozen reference
   worktree must still be at its pin (`require_on_pin`, exit 1, "is off its
   pin"), checked before any dirt because the answer to a dirty refusal is
   `--force`, which would discard commits on a detached HEAD. Then each open
   change's worktree, each frozen worktree, and the hub worktree; the first
   dirty one fails with exit 20 before anything is touched.
3. **Kill the tmux session** so its shells cannot block removals.
4. **In-use check** across all remaining worktree paths, frozen worktrees, and
   the hub worktree (exit 21).
5. Close every open change (`close_change`, as above), remove each frozen
   reference worktree, clear the pins, then remove the hub worktree.
6. Mark the feature `finished` and save. Change stages are left as they were
   (not set to merged), and every branch — including the hub branch — is kept.

A finished feature is history: only `feature list --all` and `status` accept it.

## Ordering and recovery

Worktree removal and session kills are irreversible, so both `merged` and
`finish` do them **before** writing the record. If the write then fails, the
old record (still open) remains, and rerunning the command finds the worktrees
already gone and completes.

When `git worktree remove` unregisters a worktree but cannot delete its
directory (`WorktreeRemoval::ResidueLeft`; see
[Git, tmux, and editor integration](../integrations/git-tmux-editor.md)),
the role or feature is still recorded as closed and a `warning:` names the
leftover directory and git's reason, for manual cleanup.

## In-use detection (`src/procs.rs`)

`procs::cwd_holders` runs `lsof -w -d cwd -F pcn` and parses `p`/`c`/`n`
records into `Holder { pid, command, cwd }`. It excludes hub itself, its parent
shell, and the `lsof` child, which inherit the caller's directory (often the
worktree being checked) but never recreate files. `holders_in(root)` keeps
holders whose cwd is at or under the canonicalized path. If `lsof` is not
installed, the check is skipped with a warning. The motivation is that a dev
server or test watcher recreating files mid-delete would leave an unregistered
directory behind.

## Dashboard

The dashboard's `v`, `b`, `m`, and `f` keys run these same functions after a
preview; for `merged`/`finish` the preview is the confirmation (see
[Dashboard flows and overlays](../ui/dashboard-flows-and-overlays.md)).

## Tests

`tests/review_merged.rs`, `tests/finish.rs` (dirty, in-use via the fake `lsof`,
adopted, residue via `block_deletion`, own-session refusal), `tests/set_base.rs`;
`procs.rs` has parser unit tests.

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](frozen-bases.md)
- [Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
- [Status, list, and pull](status-and-pull.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
