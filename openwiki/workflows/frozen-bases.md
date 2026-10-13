---
type: workflow
title: "Frozen bases: freeze, unfreeze, and refreeze"
description: How a feature pins a role's base to a fixed commit — the Pin record, reference worktrees versus pinned open changes, where pin commits come from, the freeze/unfreeze/refreeze transactions and their off-pin safeguard, how status, start, add, merged, set-base, finish, pull, and the repo commands treat pins, and the dashboard keys.
tags: [workflow, freeze, pins, bases, worktrees, status]
sources:
  - id: openwiki-source-2e469aed167f415af0005649
    resource: repo://src/ops/finish.rs
  - id: openwiki-source-de3a6bb0f340e7d1caaf6a0d
    resource: repo://src/ops/freeze.rs
  - id: openwiki-source-663ac1b065deb6f67a90cc27
    resource: repo://src/ops/merged.rs
  - id: openwiki-source-f9c1270d031f29e27a646bcd
    resource: repo://src/ops/pull.rs
  - id: openwiki-source-d7623854325d1c8d971147e7
    resource: repo://src/ops/remove.rs
  - id: openwiki-source-70c2e2f99d62af87dfbdb300
    resource: repo://src/ops/repo.rs
generated: { by: "claude-code", at: "2026-10-13T22:33:09.880Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-13T22:33:09.880Z
---

# Frozen bases: freeze, unfreeze, and refreeze

In a busy monorepo the base branch moves constantly, so a feature's diff
against `origin/<base>` keeps shifting. A **pin** fixes a role's base at one
commit for one feature, and upstream is taken in on purpose: `refreeze` moves
the pin to the current local base tip, and `unfreeze` releases it. The
operations live in `src/ops/freeze.rs`; the record shape is described in
[Features, changes, stages, and bases](../concepts/features-changes-and-stages.md).

## Two kinds of pin

A `Pin { role, clone, base, sha, worktree }` lives in `Feature.pins`, at most
one per role:

| Role state | `worktree` | Checkout | What the pin does |
|---|---|---|---|
| No open change (**frozen reference**) | `Some` | a hub-owned, **detached** worktree at `<worktree_dir>/<clone>/<feature>-frozen` | gives the feature and its agent a fixed, read-only view of that repo instead of the moving main clone |
| Open change (**pinned change**) | `None` | the change's own worktree | `status` measures the branch against the pin instead of `origin/<base>` |

A role therefore still resolves to exactly one checkout. `session::role_checkouts`
lists open changes and frozen reference worktrees together in manifest order,
so frozen roles get a workspace folder and a tmux window like any member (see
[Git, tmux, and editor integration](../integrations/git-tmux-editor.md)).

## Where a pin's commit comes from

Freezing never fetches, and never reads `origin/<base>`:

- **Reference role:** `resolve_pin` takes the tip of the **local**
  `refs/heads/<base>`. With `--at REF`, the commit must be that tip or one of
  its ancestors. `rev-parse` resolves `REF^{commit}`, so a tag pins a commit,
  not a tag object.
- **Open change:** `change_pin` takes the merge base of the branch and the
  local base, so a freshly pinned change starts 0 behind its pin. `--at` also
  works here.
- **`refreeze`:** always the current local base tip.

`pin.base` is the role's effective base when the pin is taken.

## `hub feature freeze ROLE [--at REF]`

`freeze` dispatches on the role's state:

- **Open change.** `freeze_change` refuses a role that is already pinned. It
  pins the change at its merge base (or `--at`) and saves. No worktree is
  created.
- **No open change.** `add_frozen`, the step shared with `start --freeze`:
  - It refuses an unknown or already-frozen role, a role with an open change,
    a missing clone, or an occupied worktree path.
  - It creates the detached worktree and records it in `Created`, then pushes
    the pin. It does not save.
  - `freeze` then saves. A failed write removes the pin and rolls the
    worktree back.
  - Rewriting the workspace and converging tmux happen after the write, and
    failures there are only warnings.

## Commits off the pin

A reference worktree is detached, so commits made in it are on no branch.
Checking another commit out, or removing the worktree, would lose them, and the
dirty check cannot see them. `require_on_pin(path, pin)` compares the
worktree's HEAD with `pin.sha`. When they differ it refuses with exit 1, and
its message:

- says the worktree "is off its pin";
- names the role and the short HEAD;
- suggests `git -C <path> branch <name> <head>` to keep the commits.

Every command that would move or remove a reference worktree checks this
**before** checking for dirt. The usual answer to a dirty refusal is
`--force`, and force skips the pin check:

| Command | Off its pin | Dirty |
|---|---|---|
| `refreeze` | always refuses | refuses when the pin would move (exit 20) |
| `unfreeze` | refuses unless `--force` | refuses unless `--force` (exit 20) |
| `finish` | checks every pin before any dirty check; refuses unless `--force` | refuses unless `--force` (exit 20) |
| `feature add` on the role | refuses; `add` has no `--force` | refuses (exit 20) |

`status` reports such a worktree as `off-pin` and exits 30 (drift).

## `hub feature unfreeze ROLE [--force]`

For a reference worktree, `unfreeze`:

1. Without `--force`, runs the pin check, then the dirty check.
2. Re-points the role's tmux window at the main clone, so the window's own
   shell does not count as in use.
3. Refuses if a process is still in the worktree (exit 21 without `--force`).
4. Removes the worktree, then removes the pin and saves.

The removal comes first because it cannot be undone. If the write then fails,
rerunning finds the worktree gone and completes. A pinned change has nothing
on disk: its pin is simply removed, and comparisons return to `origin/<base>`.

## `hub feature refreeze ROLE`

`refreeze` moves the pin to the local base tip and never merges anything:

1. It refuses a missing clone or a missing reference worktree, then runs the
   pin check.
2. A pinned change's lag against the new pin is printed either way, as
   "<branch> is N behind the pin".
3. If the pin is already at the tip, it says so and stops.
4. Otherwise it refuses a dirty worktree, checks the new commit out detached,
   and saves.
5. If the save fails, it moves the worktree back to the HEAD it found.

## Status, list, and pull

These read pins without changing them; details are in
[Status, list, and pull](status-and-pull.md):

- **Frozen reference row:** `stage` `frozen`, `branch` = the base, and state
  text such as `ok, frozen @abc1234, master +3 since pin`. A moved base stays
  green: it is a "worth a refreeze?" signal, not a problem.
- **Drift:** HEAD off the pin, or a missing or unregistered worktree, is
  drift (exit 30).
- **Pinned change:** shows `behind pin <short> by N`, plus
  `frozen @<short>` when nothing is missing.
- **JSON:** both kinds carry `frozen_sha` and `base_moved`.
- **`hub feature list`:** shows `ROLE:frozen` for reference roles, and
  `--json` always includes `frozen: [{role, sha}]`. A pinned change stays in
  `roles`.
- **`pull`:** skips frozen worktrees and reports them as frozen.

## Starting, adding, and closing

- **`hub feature start --freeze ROLE`** (repeatable) pins at start inside the
  start transaction. A role given to both `--repo` and `--freeze` is a usage
  error (exit 1). The interactive start offers to freeze the roles not
  picked. See [Starting features and adding roles](feature-start-and-add.md).
- **`hub feature add` on a frozen reference role** replaces the reference
  worktree with a change worktree:
  - By default the pin carries over. A new branch starts at the pin, and an
    adopted branch re-derives its pin from its merge base.
  - `--from-latest` or `--from REF` drops the pin.
  - A carried pin is also recorded as the change's `base_sha`, so a
    local-only pin does not produce a false "merged" hint.
  - The branch and worktree are created first; then the tmux window is
    re-pointed and checked for processes; then the record is written; and
    only then is the reference worktree removed.
- **`merged`** drops the pin with the change, and the role goes back to
  having no checkout in the feature.
- **`set-base`** refuses a pinned change until it is unfrozen. See
  [Review, merged, set-base, and finish](review-merged-finish.md).
- **`finish`** removes frozen worktrees along with the rest.
- **`remove`** looks only at the feature's changes and the hub branch.
  A frozen reference role is a detached checkout with no branch, and `finish`
  has already cleared the pins, so there is nothing to delete for it. A pinned
  open change is still a change, so its branch is checked and deleted like any
  other member branch.

## Repo commands

[`hub repo`](hub-and-repo-management.md) refuses changes that would leave a
pin pointing at a vanished role, clone, or base:

- `repo remove`, `repo set --clone`, and `repo set --base` refuse while any
  open feature has the role frozen.
- `repo rename` carries pins along, and their tmux windows are renamed with
  them.
- `repo rename --records-only` refuses, before writing anything, a rename that
  would give a role two pins, or a frozen reference beside an open change.

## Dashboard

On the feature screen, `z` opens a freeze picker over the unpinned roles, `u`
unfreezes the frozen row under the cursor (a reference worktree gets a removal
preview with off-pin and dirty warnings, and `F` forces), and `Z` refreezes it.
Frozen reference rows render in a read-only style. See
[Dashboard flows and overlays](../ui/dashboard-flows-and-overlays.md).

## Compatibility

Records with no pins serialize without a `pins` key, so files written before
this feature are unchanged. An older `hub` ignores `pins`, and would drop the
key if it rewrote a record that has one.

## Tests

`tests/freeze.rs` covers:

- the freeze → status → refreeze → unfreeze cycle on both kinds of pin;
- `--at` validation, and a local base ahead of origin;
- drift: off-pin and missing worktrees;
- refusals: dirty, in use, and off its pin (including off-pin plus dirty);
- rollback when the record write fails;
- every start-point outcome of `add` on a frozen role;
- the repo command guards, and `feature list` output.

`tests/start.rs` covers `start --freeze`.
