---
type: concept
title: Features, changes, stages, and bases
description: The project-hub domain model stored in feature records — Feature, Change, Stage, Owner, Worktree — including the one-open-change-per-role invariant, recorded bases and base_sha, finished features, and the naming/template rules.
tags: [concepts, domain-model, feature, change, stage, base, naming]
sources:
  - id: openwiki-source-c17c8e3e271a8329ed7a5e63
    resource: repo://src/feature.rs
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-482a254fc9f19462c83aa694
    resource: repo://src/names.rs
  - id: openwiki-source-e0a6376d32dd6b3f48d01cbe
    resource: repo://src/ops/resolve.rs
  - id: openwiki-source-262e8a6df6df877411d7bc10
    resource: repo://src/ops/set_base.rs
  - id: openwiki-source-c73c116b0313275504b5f637
    resource: repo://src/ops/status.rs
generated: { by: "claude-code", at: "2026-10-12T18:18:24.526Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T18:18:24.526Z
---

# Features, changes, stages, and bases

The domain types live in `src/feature.rs` and are serialized as one JSON
record per feature under `<git common dir>/hub/features/<name>.json` (see
[Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)).

## The model

```
Feature { name, checkout, status: open|finished, created, changes: [Change] }
Change  { role, clone, branch, worktree: Option<Worktree>, stage: working|review|merged,
          review_url?, merged_at?, origin_seen, base?, base_sha? }
Worktree { name, owner: hub|adopted }
```

- **Feature** — a unit of cross-repo work. `checkout` is the name of its hub
  worktree and tmux session (rendered from the manifest's checkout template).
  `status` is `open` until `feature finish`, after which the record is kept
  as history.
- **Change** — one branch in one member repo taking part in the feature.
  - `clone` is copied from the manifest when the change opens, so the
    change's location survives a later `hub repo remove`.
  - `worktree: None` means the branch is checked out in the main clone;
    otherwise `name` is the directory under `<worktree_dir>/<clone>/`.
  - `owner` distinguishes worktrees the hub created (`hub`) from existing
    ones it bound as-is (`adopted`); removal of an adopted worktree asks for
    confirmation (see [Review, merged, set-base, and finish](../workflows/review-merged-finish.md)).
  - `origin_seen` records that hub has observed the branch on origin
    (set on open when it existed remotely, by `feature review` when it is on
    origin, and always for `start-review`). `status` uses it to tell a merged
    branch apart from one merely fast-forwarded to its base, and to hint
    "gone from origin" after a pruning fetch.
- **Stage** — `working` → `review` → `merged`. A change is *open* while its
  stage is not `merged`.

## Invariant: one open change per role

A feature holds at most one open change per role. It is enforced twice:

- `Feature::push_change` refuses a second open change with
  `role 'ROLE' already has an open change on branch '...'`.
- `Feature::validate` (run on every load and save) rejects a record in which a
  role has more than one open change.

The purpose is that, within a feature, a role always resolves to exactly one
checkout — the open change's worktree, or the base checkout otherwise — so
role-addressed commands (`review`, `merged`, `set-base`), status/pull rows,
the workspace file, and tmux windows are never ambiguous. To move a role to a
different branch, close the open change with `feature merged` and then
`feature add` again; closed changes stay in `changes` as history, and
`names::next_branch` proposes a follow-up name such as `foo-2`.

## Pins (frozen bases)

A feature record also holds `pins`, one `Pin { role, clone, base, sha,
worktree }` per role whose base is frozen at a fixed commit (`src/feature.rs`).
The key is omitted when there are no pins, so records written before frozen
bases load and round-trip unchanged. A pin takes one of two shapes:

- **With a `worktree`:** the role has no open change and its pinned commit is
  checked out, detached, in a hub-owned reference worktree. That worktree is
  the role's single checkout for the feature.
- **Without a worktree:** the pin belongs to the role's open change, whose
  worktree stays the checkout; `status` measures the change against the pin
  instead of `origin/<base>`.

`Feature::validate` keeps the one-checkout rule across both lists: a role is
pinned at most once, a pin with a worktree may not coexist with an open change
for the role, and a pin without a worktree must have one. `pin.base` is a
branch name without `origin/` and `sha` must be a full 40- or 64-digit commit
id. See [Frozen bases](../workflows/frozen-bases.md).

## Bases

Each change records the branch it merges into:

- `base` is a branch name **without** `origin/` (validation rejects an
  `origin/` prefix). It defaults to the role's `base` in `hub.json` when the
  change opens, unless `feature add --base` or `feature set-base` names
  another one.
- `Change::effective_base` falls back to the manifest base for legacy records
  written before the field existed.
- `base_sha` is the merge base of the branch and `origin/<base>` when the change
  opened (or the base tip for a fresh branch). `status` only reports
  "merged into origin/<base>" once the branch is an ancestor of the base
  **and** has work beyond `base_sha`, so an untouched new branch is not
  flagged as merged.

`hub feature set-base ROLE BRANCH` (`src/ops/set_base.rs`) is a record-only
operation: it requires an open feature and an open change for the role,
refuses `origin/`-prefixed input, fetches origin, requires `origin/<BRANCH>`
to exist and the local feature branch to exist, recomputes `base_sha` as the
merge base, and saves the feature. It never checks out, resets, rebases, or
edits git's upstream, and is a no-op when base and `base_sha` are unchanged.
It refuses a pinned open change ("unfreeze first"), because the pin was taken
from the current base. A frozen reference role has no open change, so it gets
the ordinary "no open change" refusal.

## Finished features

`Feature::require_open` gates lifecycle and environment commands (`add`,
`set-base`, `tmux`, `open`, `sync`, ...) with "feature 'NAME' is finished".
Only `feature list --all` and `status` accept finished features; `status`
lists their rows under `Completed`.

## Names and templates (`src/names.rs`)

| Thing | Rule |
|---|---|
| Feature and checkout names | `^[A-Za-z0-9][A-Za-z0-9_-]*$` — valid as a git branch, tmux session, and directory |
| Roles | `^[A-Za-z][A-Za-z0-9_-]*$`; `hub` and `ai` are reserved for fixed tmux windows |
| Clones | `[A-Za-z0-9-]+` |
| Worktree names | exactly one normal path component |
| Branch names | an in-process equivalent of `git check-ref-format --branch`, additionally rejecting a lone `@` |

Templates:

- **Checkout template** (manifest `checkout_template`, default `{feature}`)
  must contain at least one of `{feature}`, `{feature_snake}` (dashes become
  underscores), `{hub}`.
- **Branch template** (per role, default `{feature}`) must contain
  `{feature}`; `ROLE:BRANCH` on the command line overrides it for one feature.
- `flatten_branch` turns `feature/foo` into `feature-foo` for directory names.

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](../workflows/frozen-bases.md)
- [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)
- [Starting features and adding roles](../workflows/feature-start-and-add.md)
- [Review, merged, set-base, and finish](../workflows/review-merged-finish.md)
