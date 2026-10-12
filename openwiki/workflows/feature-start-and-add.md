---
type: workflow
title: Starting features and adding roles
description: How `hub feature start` and `hub feature add` create or adopt the hub branch, hub worktree, and per-role branches and worktrees through `ops::resolve::open_change`, roll back exactly what they created on failure, treat workspace and tmux steps as post-save warnings, and prompt for roles and branches in a terminal.
tags: [workflow, feature-start, feature-add, adoption, rollback, prompts]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
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
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# Starting features and adding roles

`hub feature start NAME [--checkout C] [--repo ROLE[:BRANCH]]...` creates a
feature; `hub feature add [ROLE] [--branch B] [--from REF] [--base B]
[--worktree W]` brings one more role into an open feature. Both share
`ops::resolve::open_change` for the per-role work and run under the hub lock
(see [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)).

## `feature start` (`src/ops/start.rs`)

1. **Preflight** (`start::preflight`, also used by the dashboard wizard):
   validate the feature name, resolve the checkout name (`--checkout` or the
   manifest checkout template), validate it, and refuse if the feature record
   already exists or the hub worktree path
   `<worktree_dir>/<hub dir>/<checkout>` already exists.
2. Validate every `--repo` role against the manifest and refuse duplicates;
   process roles in **manifest order** regardless of flag order.
3. **Transaction** (`start_inner`):
   - If the hub has remotes, fetch `origin` (failure is a `warning:` and the
     last fetched refs are used) so a hub branch a teammate pushed can be adopted.
   - Hub branch `NAME`: adopt a local branch if it exists; else create a
     tracking branch from `origin/NAME`; else create it from `main`.
   - Add the hub worktree on that branch.
   - For each role, `open_change` (below), then `Feature::push_change`.
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
from a merged change and still pointing at the main clone), it **respawns that
window** in the new worktree.

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
| no | no | create from `--from` or `origin/<base>` |

6. Record `base_sha` as the merge base of the branch and `origin/<base>`
   (falling back to the base tip for unrelated histories so adoption never fails here).
7. Worktree:
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
8. Return a `Change` with stage `working`, `origin_seen` = whether the branch
   was on origin, and the recorded `base` and `base_sha`.

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

- `start` with no `--repo`: preflight first (fail as `start` would), then a
  multi-select of roles (none = docs-only start), then a branch per role.
- `add` with no role: select from roles without an open change
  (`suggest::addable_roles`); then a branch if `--branch` is missing. The
  feature is loaded and `require_open` checked before asking.
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

- [Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
- [Reviewing a merge or pull request](start-review.md)
