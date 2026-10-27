---
type: workflow
title: Hub init and repo registration
description: How `hub init` creates a hub (and `--force` refreshes init-owned files), and how `hub repo add`, `set`, `remove`, `rename` (including `--records-only` repair), and `list` edit `hub.json` with one commit on main while keeping per-machine feature records consistent.
tags: [workflow, init, repo, manifest, rename, records]
sources:
  - id: openwiki-source-03561c75d5622526b12cd559
    resource: repo://src/ops/init.rs
  - id: openwiki-source-70c2e2f99d62af87dfbdb300
    resource: repo://src/ops/repo.rs
generated: { by: "claude-code", at: "2026-10-27T18:18:48.498Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-27T18:18:48.498Z
---

# Hub init and repo registration

These commands manage the hub itself: the committed manifest `hub.json` and
the files `hub init` owns. Manifest edits go through `Hub::save_manifest`,
which writes and commits **only** `hub.json` on `main` as `hub: <message>`
and never pushes (see [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)).
Teammates pick up changes by pulling `main` themselves.

## `hub init` (`src/ops/init.rs`)

`hub init [--name N] [--checkout-template T] [--force]` runs without locating a hub.

Fresh init:

1. Hub name: `--name` or the directory name; must be a valid name.
   `--checkout-template` must contain `{feature}`, `{feature_snake}`, or `{hub}`.
2. Refuse if `hub.json` already exists (suggesting `--force`).
3. If the directory is already a git repo it must be on `main`; otherwise
   `git init -b main`.
4. Write `hub.json`; write a default `AGENTS.md` with six hub rules (status
   before paths, roles not paths, what is committed vs local, per-repo docs
   stay in their repo, the generated skill is not edited, and feature
   documents reach `main` only through the skill's merge-back, with one
   folder per feature under `docs/`) **only if neither `AGENTS.md` nor
   `CLAUDE.md` exists**, so a hub never carries two instructions files and a
   hub created before 0.26 keeps its `CLAUDE.md`; merge the hub's lines into
   `.gitignore` (`.DS_Store`,
   `.tmp/`, `*.code-workspace`, `.claude/skills/hub/`, `.agents/skills/hub/`),
   keeping every existing line.
5. Commit those paths as `hub: init <name>`, then exclude and install the agent
   skill (failure is a warning; see [Bundled agent skill](../integrations/agent-skill.md)).

`--force` on an existing hub (`refresh`) brings init-owned files up to date
after upgrading `hub`; it cannot be combined with `--name` or
`--checkout-template`, and requires `main`:

- merges missing ignore lines into `.gitignore`;
- removes files now matched by those lines from the index (they stay on disk) —
  the files are moved aside during `git commit --only` so the commit records a
  deletion rather than re-adding them;
- commits as `hub: refresh init files`;
- deletes a pre-0.14.0 `pre-push` hook if it carries hub's marker;
- reinstalls the skill. `hub.json` and the instructions file (`AGENTS.md`
  or a legacy `CLAUDE.md`) are never modified.

## `hub repo add ROLE CLONE` (`src/ops/repo.rs`)

- Validates the role (reserved `hub`/`ai` rejected) and clone name, refuses a
  duplicate role or a clone already registered under another role.
- `<project_home>/CLONE` must be a git repo with an `origin`; the remote is
  recorded **as configured** (`remote.origin.url`), not an `insteadOf`
  rewrite. Later, `open_change` requires the clone's origin to match this
  exactly.
- Base: `--base` or `origin/HEAD`'s branch (error if neither is available).
- Branch template: `--branch-template` (must contain `{feature}`) or `{feature}`.
- Appends the role (manifest order is deployment and tmux window order) and
  commits `hub: repo add ROLE`.

## `hub repo set ROLE`

Changes any of `--clone`, `--base`, `--branch-template`, `--description` in
place, keeping the role's position, in one commit. Needs at least one flag;
`--base` must not be empty or `origin/`-prefixed. Changing the base refuses
while any open feature has the role frozen (either kind of pin), since the pin
was taken from the old base. Changing the clone requires the new clone to be
free, refuses while any open feature has an open change for the role or has it
frozen, and re-reads the new clone's `origin` as the remote. A no-op change
(including `--base` set to its current value) commits nothing and is never
refused.

## `hub repo remove ROLE`

Refuses while any open feature has an open change for the role or has it
frozen ([Frozen bases](frozen-bases.md)); otherwise
removes it from the manifest and commits. The clone and its branches are
untouched, and existing feature records keep their `clone` field so history and
paths still resolve.

## `hub repo rename OLD NEW`

Feature records are per machine, so a rename touches both shared and local state:

1. Refuse if `NEW` is already registered or any local feature already records
   changes or a pin for `NEW`.
2. Rewrite every local record's changes and pins for `OLD` to `NEW`; rename the
   role in the manifest in place and commit. The summary counts changes and,
   when any were renamed, pins.
3. Save the rewritten records. If that fails, the error explains how to finish
   with `--records-only`.
4. For open features with an open change or a frozen reference worktree for
   `NEW`: rename the tmux window `OLD` → `NEW` and regenerate the workspace
   (warnings only).

`--records-only` catches up **another machine's** records after pulling a
rename, and never commits. `NEW` must already be in `hub.json`; `OLD` and
`NEW` must differ. Before writing anything, it refuses a rewrite that would
give a feature two open changes for `NEW`, move a pin onto a role that already
has a pin or an open change, or move an open change onto a role frozen with a
reference worktree: each would leave a role with two checkouts. Because records
are saved one feature at a time, refusing up front avoids a half-renamed set. `--clone C` (only with `--records-only`) limits the rewrite
to changes recorded with clone `C`, which repairs records left by an earlier
remove and re-add. A sequence of renames should be replayed in the order it was
made.

## `hub repo list [--json]`

Prints role, clone, base, branch template, and description in manifest order;
`--json` serializes the manifest's repo list.

## Tests

`tests/init.rs` and `tests/repo.rs` (including commit-failure rollback via the
fixture's `block_commits`; see
[Testing and the integration harness](../testing/integration-test-harness.md)).

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](frozen-bases.md)
- [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)
- [Bundled agent skill](../integrations/agent-skill.md)
