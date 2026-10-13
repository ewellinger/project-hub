---
type: architecture
title: Hub state, manifest, and locking
description: How `hub` locates a hub repository, what state is committed (`hub.json`) versus kept per machine (feature records under the git common dir), how the advisory lock serializes mutating commands, and how JSON is written atomically.
tags: [architecture, state, manifest, locking, persistence]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-13T22:12:12.058Z
sources:
  - id: openwiki-source-a9fddfa72ce7af0e031fedf8
    resource: repo://src/hub.rs
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-cfc759a7da3457809607f5ab
    resource: repo://src/manifest.rs
  - id: openwiki-source-d7623854325d1c8d971147e7
    resource: repo://src/ops/remove.rs
  - id: openwiki-source-d36a1897ad2613d7d98e64cb
    resource: repo://src/ui/worker.rs
generated: { by: "claude-code", at: "2026-10-13T22:12:12.058Z" }
---

# Hub state, manifest, and locking

A *hub* is an ordinary git repository whose main clone holds `hub.json`. All
hub state lives in two places with very different sharing rules:

| State | Location | Shared via git? | Owner |
|---|---|---|---|
| Manifest (`hub.json`): hub name, checkout template, member repos by role | `<hub root>/hub.json` on `main` | Yes, committed | `src/manifest.rs` |
| Feature records: one JSON file per feature (its changes and, since frozen bases, its `pins`) | `<git common dir>/hub/features/<name>.json` | No, per machine | `src/feature.rs`, `src/hub.rs` |
| Lock file | `<git common dir>/hub/hub.lock` | No | `src/hub.rs` |

Because feature records live under the git *common* dir, every hub worktree
(one per feature) reads the same files, so `hub feature list` and `hub status`
agree regardless of which worktree they run from.

## Locating a hub (`Hub::locate`)

`Hub::locate(cwd, env)` in `src/hub.rs` builds the `Hub` value every command
works with:

1. Resolve the git top-level of `cwd`; failing that, the command errors with
   "is not inside a hub repository".
2. Run `git worktree list` and take the **first** entry as the hub root (the
   main clone), so running from any feature worktree still finds the main clone.
3. Require `hub.json` in that root, and require the root to be on branch
   `main` (otherwise a precondition error).
4. Refuse to open a hub that still has legacy `features/*.json` committed on
   `main` (pre-0.13.0 layout); the error prints the manual migration commands.
5. Load and validate the manifest, compute the git common dir, and infer
   `cwd_feature`: the current branch of `cwd`'s worktree is the feature name
   when it is not `main` and a record `<branch>.json` exists.
6. Decide whether tmux sessions are on (see
   [Environment and configuration](../configuration/environment-and-config.md)).

`Hub::resolve_feature(flag)` then picks `--feature NAME` or falls back to
`cwd_feature`, which is why feature commands work without flags inside a
feature's hub worktree.

Path helpers on `Hub` encode the on-disk layout: hub worktrees live at
`<worktree_dir>/<hub dir name>/<checkout>`, a feature's VS Code workspace is
`<hub worktree>/<manifest name>.code-workspace`, and the base workspace sits in
the hub root.

## The manifest (`hub.json`)

`Manifest` holds `name`, an optional `checkout_template`, and an ordered list of
`RepoSpec { role, clone, remote, base, branch_template, description }`. The
order of `repos` is meaningful: it is deployment order and tmux window order.

`Manifest::validate` runs on every load and rejects:

- an empty name;
- a `checkout_template` containing none of `{feature}`, `{feature_snake}`, `{hub}`;
- invalid or duplicate roles and clones;
- empty `remote` or `base`;
- a `branch_template` that lacks `{feature}` (the default template is `{feature}`).

Validation failures surface as `HubError::Malformed` naming the file and field.

Manifest changes are committed: `Hub::save_manifest` writes the file and calls
`Hub::commit`, which stages exactly the given paths, commits them alone with
`git commit --only` as `hub: <message>`, is a no-op when nothing changed, and
restores the paths from `HEAD` if the commit fails so no half-written state
survives. Only `hub.json` goes through this path; nothing is pushed.

## Feature records

`Hub::save_feature` writes a feature record without ever running git. Records
are validated on both load and save (`Feature::load` / `Feature::save`). See
[Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
for the record's contents.

The only command that deletes a record is `hub feature remove`
(`src/ops/remove.rs`), and only for a finished feature after its branches are
deleted. Since records are not in git, that deletion cannot be undone.

## Atomic writes

`manifest::write_json` and `manifest::write_atomic` are shared by the manifest,
feature records, and other generated files. They write pretty JSON (with a
trailing newline) to `<path>.tmp`, `fsync` it, then rename it over the target.
A crash or failure before the rename leaves the previous file intact, which
matters because since 0.13.0 a feature record has no second copy in git.
The directory is then synced best-effort; a failure there is deliberately
ignored so a completed write is never reported as an error that callers would
roll back.

## The hub lock (`Hub::with_lock`)

Every mutating command runs inside `hub.with_lock(|hub| ...)`:

- It creates `<git common dir>/hub/hub.lock` and takes a **non-blocking**
  exclusive `fd-lock` write lock. A second concurrent command fails
  immediately with "another hub command is running", rather than waiting.
- Under the lock it **reloads `hub.json`**, and callers read feature records
  inside the closure, so a command never acts on state another command changed
  while it was starting.

The CLI wraps `repo add/set/rename/remove`, `feature start`, `start-review`,
`add`, `set-base`, `freeze`, `unfreeze`, `refreeze`, `review`, `merged`,
`finish`, `remove`, and `pull` in the lock
(`src/main.rs`); read-only commands such as `status` and `list` do not take it.
The dashboard runs its actions through the same lock from a worker thread
(`src/ui/worker.rs`). Interactive prompts run before the lock is taken.

## Related pages

- [Architecture overview](overview.md)
- [Features, changes, stages, and bases](../concepts/features-changes-and-stages.md)
- [Hub init and repo registration](../workflows/hub-and-repo-management.md)
