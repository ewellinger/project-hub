---
type: integration
title: Git, tmux, and editor integration
description: The thin subprocess wrappers project-hub uses for git (worktrees, refs, fetches, conditional ref updates), tmux (sessions and windows addressed exactly), and the editor, plus how feature sessions converge and how `.code-workspace` files are generated and kept local.
tags: [integration, git, tmux, editor, vscode, workspace, subprocess]
sources:
  - id: openwiki-source-f0e955d5976d950d845e5025
    resource: repo://src/code.rs
  - id: openwiki-source-a3a32cb165bec89fdae31522
    resource: repo://src/git.rs
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-ec55c4ac9d71666850b1040d
    resource: repo://src/ops/session.rs
  - id: openwiki-source-3f5c288f59bdbf6815ffcff1
    resource: repo://src/ops/start.rs
  - id: openwiki-source-1b5cfaa33f22a91204f59602
    resource: repo://src/tmux.rs
  - id: openwiki-source-fbbd34a2cb4fd0a8355f1f33
    resource: repo://src/workspace.rs
  - id: openwiki-source-f13236b3ab41e3f4cea296b6
    resource: repo://tests/misc.rs
generated: { by: "claude-code", at: "2026-10-13T22:33:09.880Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-13T22:33:09.880Z
---

# Git, tmux, and editor integration

project-hub never links a git or tmux library; it shells out. Each wrapper is
deliberately free of business logic so the `ops` modules own the decisions.
All subprocess failures become `HubError::Command { command, stderr }`
(exit 1).

## Git (`src/git.rs`)

`Git::new(dir)` runs `git` with `current_dir(dir)`. Helpers fall into a few
groups:

- **Discovery**: `toplevel`, `common_dir`, `current_branch`, `remote_url`,
  `default_remote_branch` (what `origin/HEAD` points at), `worktrees`
  (parses `git worktree list --porcelain` into `WorktreeEntry { path, head,
  branch }`; the first entry is the main worktree).
- **Refs**: `local_branch_exists`, `remote_branch_exists`, `rev_parse`,
  `is_ancestor`, `refnames`, `merge_base` (returns `None` when histories are
  unrelated, i.e. exit code 1), `missing_commits`, `ahead_behind` (against
  `origin/<branch>`, `None` without a remote copy), `check_ref_format`.
- **Mutation**: `create_branch`, `create_tracking_branch`, `force_branch`,
  `delete_branch`, `merge_ff_only`, and the **conditional** ref updates
  `update_ref_if` / `delete_ref_if` (`git update-ref <ref> <new> <old>`),
  which only act if the ref still points at the expected commit. Rollbacks in
  `start-review` use these so a branch another process moved meanwhile is
  left alone.
- **Worktrees**: `worktree_add` creates the parent directory first.
  `worktree_add_detached(path, commit)` does the same for a detached checkout
  (a frozen reference worktree), and `checkout_detached(commit)` moves a
  detached worktree's HEAD (used by `refreeze`).
  `worktree_remove` distinguishes a clean removal from `ResidueLeft(reason)`:
  git unregisters a worktree before deleting its directory, so a failed
  removal whose worktree is no longer registered is reported as residue (the
  caller prints a `warning:` naming the leftover directory) rather than an
  error; a failure that leaves it registered is returned as an error.
- **Index/commits** for the hub's own repo: `add`, `has_staged_changes`,
  `commit_paths` (used by `Hub::commit`), `discard`, `rm_cached`,
  `tracked_matching`.
- **Ignore handling**: `is_ignored` (`git check-ignore`) and
  `exclude_locally(pattern)`, which appends to the file `git rev-parse
  --git-path info/exclude` names. That file is shared by all linked worktrees
  and never committed; it is how generated workspace and skill files stay
  untracked.

`same_path` compares paths by canonical form, which matters because
`project_home`/`worktree_dir` are canonicalized (see
[Environment and configuration](../configuration/environment-and-config.md)).

## Tmux (`src/tmux.rs`)

`Tmux` runs `tmux` (tests substitute a program and env via `with_program` /
`env`). Sessions are always targeted as `=<name>` so tmux never
prefix-matches another session. Operations: `has_session`, `new_session`
(detached), `windows`, `new_window`, `respawn_window` (kill the pane's
process and start a fresh shell in a directory), `send_keys`,
`rename_window`, `kill_session`, and `current_session` (only when `$TMUX` is
set). Whether tmux is used at all is decided in `Hub::locate`; when it is off,
`Hub.tmux` is `None`.

### Session convergence (`ops::session::converge_tmux`)

A feature's session is named after its `checkout`. Converging:

1. With tmux off, return the optional warning plus `tmux is off; no session created`.
2. Create the session (window `hub`, cwd = hub worktree) if missing.
3. Add any missing `hub` and `ai` windows (both in the hub worktree), then one
   window per role checkout — each open change and each frozen reference
   worktree, from `role_checkouts` — named after its role and rooted at that
   path, in manifest order.
4. Existing windows are never moved or killed, so a pre-existing session only
   gets missing windows appended.
5. For a newly created session, type `hub` + Enter into the `hub` window so the
   dashboard is waiting on attach; quitting it leaves a shell.
6. Print `Attach with: tmux attach -t <checkout>`. hub never attaches itself.

`hub tmux` runs exactly this to repair a session.

### The base session

Run on `main`, outside any feature worktree and without `--feature`,
`hub tmux` converges a session for the base checkouts instead
(`converge_base_tmux`). Both paths share `converge_session`, so the base
session follows the same rules: windows `hub` and `ai` in the hub's main
clone, then one window per registered repo's main clone in manifest order,
missing windows appended, nothing moved or killed, and `hub` started in the
`hub` window of a new session. Its name is `base_session_name`: the
`checkout_template` rendered with `main` as the feature (for example `dm-main`
for `dm-{feature_snake}`). Because a feature's checkout could render to the
same name, `feature start` refuses that checkout (see
[Feature start and add](../workflows/feature-start-and-add.md)).

`repoint_window(hub, feature, role, dir)` respawns a role's existing window
in another directory, killing whatever ran there. `merged`, `unfreeze`, and
`feature add` on a frozen role call it before removing a worktree, so the
window's own shell does not count as a process still using it.

## Editor and workspace files

`ops::session::write_workspace` writes `<hub name>.code-workspace` into the
feature's hub worktree, with folders: the hub worktree (`.`) and each role
checkout from `role_checkouts` — open changes and frozen reference worktrees
(role name, path relative to the hub worktree) — in manifest order.
`write_base_workspace` writes the same file into the hub root with every
registered repo's main clone. `change_path` resolves a change to its worktree
or, when `worktree` is `None`, the main clone, using the clone recorded on the
change so it works after `repo remove`. `pin_path` resolves a frozen pin to
its reference worktree, and is `None` for a pin that belongs to an open
change.

`workspace::render` only replaces the top-level `folders` key; every other key
in an existing workspace file (settings, extensions) survives. It refuses an
existing file that is not a JSON object. Before writing, the filename is added
to the local exclude file unless already ignored.

`code::open_workspace(editor, path)` runs the configured editor command
(default `code`) with the path appended, without a shell.
`open_workspace_quiet` silences stdio for the dashboard, whose raw-mode
terminal would otherwise be corrupted.

## Post-save steps are warnings

`ops::session::after_save` wraps workspace and tmux steps that run **after**
the feature record is written: a failure becomes
`warning: <err>; run \`hub sync\`` (or `hub tmux`) instead of a rollback,
because the recorded state is authoritative.

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](../workflows/frozen-bases.md)
- [Environment and configuration](../configuration/environment-and-config.md)
- [Starting features and adding roles](../workflows/feature-start-and-add.md)
- [Testing and the integration harness](../testing/integration-test-harness.md)
