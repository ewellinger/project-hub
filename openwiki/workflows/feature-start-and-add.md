---
type: workflow
title: Starting features and adding roles
description: How `hub feature start` and `hub feature add` create or adopt the hub branch, hub worktree, and per-role branches and worktrees through `ops::resolve::open_change`, roll back exactly what they created on failure, treat workspace and tmux steps as post-save warnings, and prompt for roles and branches in a terminal.
tags: [workflow, feature-start, feature-add, adoption, rollback, prompts]
sources:
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-f44614058892a27046d634e5
    resource: repo://src/ops/add.rs
  - id: openwiki-source-e0a6376d32dd6b3f48d01cbe
    resource: repo://src/ops/resolve.rs
  - id: openwiki-source-3f5c288f59bdbf6815ffcff1
    resource: repo://src/ops/start.rs
  - id: openwiki-source-456c783b5ac71de54597b9a8
    resource: repo://src/ops/suggest.rs
  - id: openwiki-source-fcdf0f9c4361c1ee384f2689
    resource: repo://src/prompt.rs
generated: { by: "claude-code", at: "2026-10-12T18:18:24.526Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T18:18:24.526Z
---

# Starting features and adding roles

`hub feature start NAME [--checkout C] [--repo ROLE[:BRANCH]]...
[--freeze ROLE]...` creates a feature; `hub feature add [ROLE] [--branch B]
[--from REF | --from-latest] [--base B] [--worktree W]` brings one more role
into an open feature. Both share
`ops::resolve::open_change` for the per-role work and run under the hub lock
(see [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)).

## `feature start` (`src/ops/start.rs`)

1. **Preflight** (`start::preflight`, also used by the dashboard wizard):
   validate the feature name, resolve the checkout name (`--checkout` or the
   manifest checkout template), validate it, and refuse if the feature record
   already exists or the hub worktree path
   `<worktree_dir>/<hub dir>/<checkout>` already exists.
2. `start::check_roles` validates every `--repo` and `--freeze` role against
   the manifest and refuses a role given twice, or given to both `--repo` and
   `--freeze`, as `HubError::Usage` (exit 1). The check lives in `ops`, not
   clap, so the dashboard's call to `start` gets it too. Both lists are then
   processed in **manifest order** regardless of flag order.
3. **Transaction** (`start_inner`):
   - If the hub has remotes, fetch `origin` (failure is a `warning:` and the
     last fetched refs are used) so a hub branch a teammate pushed can be adopted.
   - Hub branch `NAME`: adopt a local branch if it exists; else create a
     tracking branch from `origin/NAME`; else create it from `main`.
   - Add the hub worktree on that branch.
   - For each role, `open_change` (below), then `Feature::push_change`.
   - For each `--freeze` role, `freeze::add_frozen`: pin the local base tip
     and add a detached reference worktree, recorded in the same `Created`
     set, so a later failure removes it too (see
     [Frozen bases](frozen-bases.md)).
   - Write the feature record.
4. **After the write**: write the workspace file and converge tmux
   (failures become `warning: …; run \`hub sync --feature NAME\`` /
   `hub tmux --feature NAME`), and install the agent skill into the hub worktree.

With no `--repo` and no terminal, `start` is a docs-only start: just the hub
branch, worktree, workspace, and session.

## `feature add` (`src/ops/add.rs`)

Requires an open feature. `open_change`, `push_change`, and the record write
form one transaction. After the write it rewrites the workspace and converges
tmux; if the session already had a window named after the role (left over
from a merged change or a frozen role and pointing elsewhere), it **respawns
that window** in the new worktree.

On a role frozen with a reference worktree, `add` replaces that worktree with
the change's:

1. Cheap checks first, before anything is created:
   - **Base.** When the pin carries over (`AddOptions::keeps_pin`: no
     `--from`, no `--from-latest`), the change's effective base (`--base`, or
     the role's base in `hub.json`) must equal `pin.base`. Otherwise `add`
     refuses and names `--from-latest`.
   - **Pin.** `require_on_pin` refuses a reference worktree whose HEAD left
     the pin (exit 1, "is off its pin").
   - **Dirt.** A dirty reference worktree is refused with exit 20. The pin
     check comes first, and `add` has no `--force`.
2. `open_change` creates the branch and worktree. Nothing touches tmux until
   it succeeds.
3. Re-point the role's tmux window at the main clone, then `refuse_in_use`
   on the reference worktree (exit 21).
4. Write the record: push the change and replace the reference pin with the
   pin `open_change` returns, if any.
5. Remove the reference worktree. A failure here leaves a stray directory and
   a warning, never a wrong record.

A failure before the write rolls back what `open_change` created, and
re-points the window back at the reference worktree only if it was moved.

## Opening one change (`ops::resolve::open_change`)

For a role:

1. Refuse if the role is unknown, or already has an open change
   (`role 'ROLE' already has an open change on branch '…'`).
2. The role's main clone `<project_home>/<clone>` must exist and its
   `origin` URL must equal the manifest's `remote` exactly.
3. Base: `--base` (must be a valid branch name without `origin/`) or the
   role's manifest base. Fetch origin and require `origin/<base>` to exist.
4. Branch: `--branch`, else the default — the role's branch template for a
   first change, or a follow-up `-<n>` name after a merged one
   (`resolve::default_branch` via `names::next_branch`; git errors stop the
   search instead of looping forever).
5. Resolve the branch by where it already exists:

| Local | On origin | Action |
|---|---|---|
| yes | yes, same tip | reuse |
| yes | yes, local behind and not checked out | fast-forward to `origin/<branch>`, reuse |
| yes | yes, local ahead | reuse |
| yes | yes, diverged | refuse: reconcile first |
| yes | no | reuse |
| no | yes | create a tracking branch |
| no | no | create from `--from`, else the carried pin's commit, else `origin/<base>` |

   On a frozen role, `--from` with a branch that already exists (locally or
   on origin) is refused before anything is created, since it would silently
   drop the pin.
6. The pin. When a frozen reference role's pin carries over, a branch created
   here takes the pin's commit as-is; an existing branch gets a pin re-derived
   as its merge base with the local base (`freeze::change_pin`). Either way
   `Opened.pin` is a pin with no worktree, and a "Pinned to …" line is printed.
   When the pin is dropped (`--from` or `--from-latest`), a "Dropped the pin…"
   line says the change now compares against `origin/<base>`.
7. Record `base_sha`: the pin's commit when a pin carries over (a local-only
   pin is not on `origin/<base>`, so a merge base there would be older and
   `status` would call an untouched branch merged once the base is pushed);
   otherwise the merge base of the branch and `origin/<base>` (falling back to
   the base tip for unrelated histories so adoption never fails here).
8. Worktree:
   - Branch already checked out in the **main clone** → use the main clone
     (`worktree: None`).
   - Checked out directly under `<worktree_dir>/<clone>/` → **adopt** it
     (`owner: adopted`).
   - Checked out anywhere else → refuse.
   - `--worktree` with an already-checked-out branch → refuse.
   - Otherwise create a hub-owned worktree at
     `<worktree_dir>/<clone>/<--worktree or flattened branch>`, refusing if
     the directory exists, and copy `.vscode/settings.json` and `.env` from the
     main clone **only if the clone ignores them** (an unignored copy would
     make the worktree dirty immediately).
9. Return a `Change` with stage `working`, `origin_seen` = whether the branch
   was on origin, and the recorded `base` and `base_sha`, plus the carried
   pin, if any.

See [Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
for these fields.

## Rollback (`resolve::Created`)

Everything an invocation creates is recorded in `Created`: plain branches,
conditionally created branches, fast-forwarded branches, and worktrees. If
anything fails up to and including the record write, `rollback` undoes it in
reverse order: remove worktrees (forced), restore fast-forwards, delete
conditional branches, then delete plain branches. Ref moves are conditional
(`update-ref` with the expected old value) and skip a branch some worktree
now has checked out, so concurrent work is never clobbered. Anything left
behind is reported in the error. Existing work that was only adopted is never
touched.

After the record write, nothing is rolled back: the record is authoritative,
and workspace/tmux failures are warnings with a retry command
(`ops::session::after_save`).

## Interactive prompts (`src/prompt.rs`)

When stdin and stderr are both terminals (`prompt::is_interactive`), `main.rs`
fills in missing input **before taking the lock**, so cancelling changes
nothing:

- `start` with no `--repo`: preflight and `check_roles` first (fail as
  `start` would), then a multi-select of roles (none = docs-only start, with
  any `--freeze` roles left out). If roles remain, a second multi-select asks
  which to freeze at their local base tip, defaulting to none, and its picks
  are added to `--freeze`. Then a branch per role. With `--repo` neither role
  question is asked.
- `add`: in a terminal every `add` goes through `pick_add`, even with the
  role and branch given. With no role, select from roles without an open
  change (`suggest::addable_roles`); then a branch if `--branch` is missing.
  For a **new** branch of a frozen reference role, unless `--from` or
  `--from-latest` was given, `pick_start` asks "from frozen <sha>" or "from
  latest origin/<base>". It never asks for an existing branch. The feature is
  loaded and `require_open` checked before asking.
- Esc or Ctrl-C exits 1 with nothing changed. Without a terminal nothing
  prompts and `add` requires its role (clap error, exit 2).

### Branch rows (`src/ops/suggest.rs`)

The branch picker (CLI and dashboard alike) shows `suggest::branch_rows`:

1. the default branch, marked `new` if no ref exists, otherwise with where it lives;
2. other branches from `refs/heads` and `refs/remotes/origin` (excluding the
   role's base and branches checked out somewhere the hub cannot adopt),
   labeled local/origin/"checked out at …", sorted with names containing the
   feature name first;
3. "type another name", which survives any filter and opens a validated
   name prompt prefilled with the filter text.

Suggestions read refs only and never fetch.

## Tests

`tests/start.rs`, `tests/add.rs`, `tests/resolve.rs` (adoption, refusal, and
rollback cases using the fixture's fault injection), and `tests/suggest.rs`;
`resolve.rs` unit tests cover conditional rollback and the
`default_branch` error path.

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](frozen-bases.md)
- [Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
- [Reviewing a merge or pull request](start-review.md)
