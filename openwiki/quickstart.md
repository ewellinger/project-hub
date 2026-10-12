---
type: quickstart
title: Quickstart
description: Orientation for project-hub, the Rust `hub` CLI and TUI that coordinates features spanning several git repositories through a shared hub repo, worktrees, tmux sessions, and VS Code workspaces — with build/test commands and a map from common tasks to the right wiki page.
tags: [quickstart, overview, navigation, rust, cli]
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-f317ee207e1653d2033c81a4
    resource: repo://CONTRIBUTING.md
  - id: openwiki-source-23775c3de52f3ab95a13cb8b
    resource: repo://README.md
  - id: openwiki-source-c17c8e3e271a8329ed7a5e63
    resource: repo://src/feature.rs
generated: { by: "claude-code", at: "2026-10-12T18:18:24.526Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T18:18:24.526Z
---

# Quickstart

**project-hub** (crate `project-hub`, binary `hub`) makes a lightweight "glue"
git repository — a *hub* — for work that spans several repositories. The hub's
committed `hub.json` lists member repos by **role**. Each **feature** gets a
hub branch and worktree for its documents, a branch and worktree in every
member repo it touches, an optional tmux session, and a VS Code workspace.
`hub status` compares all of that with what is on disk; `hub feature finish`
takes it down again while keeping every branch. Feature bindings are recorded
per machine under the hub's `.git/hub/features/` and never committed.

## Build, run, test

```bash
cargo build                    # Rust 1.88+ (edition 2024)
cargo run -- status            # run the CLI
cargo test                     # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Running `hub` needs `git` 2.31+, `project_home` and `worktree_dir` configured
(env vars or `~/.config/hub/config.toml`), optionally `tmux`, an editor
(`code` by default), and `glab`/`gh` only for `start-review`. Bare `hub` in a
terminal opens the dashboard.

## Source map

| Path | What lives there |
|---|---|
| `src/main.rs` | clap CLI and dispatch; binary-only `prompt` and `ui` modules |
| `src/hub.rs`, `src/manifest.rs`, `src/feature.rs` | hub discovery, lock, manifest, feature records |
| `src/env.rs`, `src/names.rs`, `src/error.rs` | configuration, naming rules, errors/exit codes |
| `src/ops/` | one module per command family |
| `src/git.rs`, `src/tmux.rs`, `src/code.rs`, `src/workspace.rs`, `src/procs.rs`, `src/review.rs` | external tool wrappers |
| `src/skill.rs`, `skills/hub/SKILL.md` | agent skill embedded in the binary |
| `src/ui/` | ratatui dashboard |
| `tests/`, `tests/fake-bin/` | integration tests with fake tmux/code/lsof/gh/glab |

## Where to go for a task

| If you need to… | Read |
|---|---|
| Understand the crate layout, dispatch, and exit codes | [Architecture overview](architecture/overview.md) |
| Know what is committed vs per-machine, the lock, atomic writes | [Hub state, manifest, and locking](architecture/hub-state-and-locking.md) |
| Understand features, changes, stages, bases, one-open-change-per-role, naming | [Features, changes, stages, and bases](concepts/features-changes-and-stages.md) |
| Change config resolution, `hub config`, or tmux on/off | [Environment and configuration](configuration/environment-and-config.md) |
| Work on `hub init` or `hub repo add/set/remove/rename` | [Hub init and repo registration](workflows/hub-and-repo-management.md) |
| Work on `feature start`/`add`, adoption, rollback, prompts, branch suggestions | [Starting features and adding roles](workflows/feature-start-and-add.md) |
| Work on `feature start-review` and GitLab/GitHub handling | [Reviewing a merge or pull request](workflows/start-review.md) |
| Work on `review`, `set-base`, `merged`, `finish`, dirty/in-use refusals | [Review, merged, set-base, and finish](workflows/review-merged-finish.md) |
| Work on frozen bases: `freeze`/`unfreeze`/`refreeze`, pins, `start --freeze`, `add` on a frozen role | [Frozen bases: freeze, unfreeze, and refreeze](workflows/frozen-bases.md) |
| Work on `status`, drift, hints, `feature list`, or `pull` | [Status, list, and pull](workflows/status-and-pull.md) |
| Touch git/tmux/editor calls or workspace files | [Git, tmux, and editor integration](integrations/git-tmux-editor.md) |
| Edit the agent skill or its distribution | [Bundled agent skill](integrations/agent-skill.md) |
| Change the dashboard's event loop, workers, or caching | [Dashboard architecture](ui/dashboard-architecture.md) |
| Change dashboard keys, wizards, previews, or overlays | [Dashboard flows and overlays](ui/dashboard-flows-and-overlays.md) |
| Write or debug tests, use the fixture and fakes | [Testing and the integration harness](testing/integration-test-harness.md) |
| Understand CI, releasing, and crates.io publishing | [CI and release](operations/ci-and-release.md) |

## Invariants worth knowing first

- Mutating commands take a non-blocking lock in `.git/hub/hub.lock`; a second
  concurrent command fails immediately.
- A feature has at most one open change per role, and a role resolves to
  exactly one checkout: its open change, a frozen reference worktree, or the
  main clone.
- Creation is transactional up to the feature-record write and rolls back only
  what it created; workspace and tmux steps after the write are warnings with a
  retry command.
- Nothing ever deletes a branch, pushes, or rebases; removals refuse dirty
  (exit 20) or in-use (exit 21) worktrees without `--force`; `status` exits 30
  on structural drift. A frozen worktree whose detached HEAD left its pin is
  refused (exit 1) before any dirty check, so `--force` never silently
  discards commits on no branch.
- Editing `skills/hub/SKILL.md` changes the binary, and the `skill` test
  requires every command it lists to exist.
