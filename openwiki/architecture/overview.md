---
type: architecture
title: Architecture overview
description: Crate layout of project-hub (library `hub` plus binary `hub`), how modules are layered, how `src/main.rs` dispatches CLI subcommands to `ops::*`, and how `HubError` maps to process exit codes.
tags: [architecture, cli, modules, errors, exit-codes]
sources:
  - id: openwiki-source-651d1fb6c9e49916a916ab51
    resource: repo://Cargo.toml
  - id: openwiki-source-95f84abff7e2ed283d50b3fb
    resource: repo://src/error.rs
  - id: openwiki-source-ed8bf05e307c6278442542c2
    resource: repo://src/lib.rs
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
generated: { by: "claude-code", at: "2026-10-12T18:18:24.526Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T18:18:24.526Z
---

# Architecture overview

project-hub is a single Rust crate (edition 2024, MSRV 1.88) published as
`project-hub` that builds a binary named `hub`. It coordinates features that
span several git repositories: one hub git repo lists member repos by role, and
each feature gets a hub worktree, a branch and worktree per member repo it
touches, an optional tmux session, and a VS Code workspace.

## Crate layout

`Cargo.toml` declares both a library and a binary from the same package:

- **Library `hub`** (`src/lib.rs`): all domain logic. Integration tests link
  against it (for example `hub::env::Env` in `tests/common/mod.rs`).
- **Binary `hub`** (`src/main.rs`): clap argument parsing and dispatch, plus two
  binary-only module trees declared with `mod prompt; mod ui;` inside
  `main.rs`. Interactive prompting (`inquire`) and the dashboard
  (`ratatui`/`crossterm`) therefore never become part of the library API.

## Module layers

| Layer | Modules | Role |
|---|---|---|
| Core state | `hub`, `manifest`, `feature`, `env`, `names`, `error` | Locate a hub, load/validate `hub.json` and feature records, resolve configuration, validate names, define errors. See [Hub state](hub-state-and-locking.md) and [Features](../concepts/features-changes-and-stages.md). |
| External tools | `git`, `tmux`, `code`, `procs`, `review`, `workspace`, `skill` | Thin wrappers that shell out to `git`, `tmux`, the editor, `lsof`, `glab`/`gh`, and generate workspace/skill files. See [Git, tmux, and editor](../integrations/git-tmux-editor.md). |
| Operations | `ops::*` (`init`, `config`, `repo`, `start`, `start_review`, `add`, `resolve`, `freeze`, `set_base`, `review`, `merged`, `finish`, `list`, `status`, `pull`, `session`, `suggest`) | One module per command family. Each returns `Result<Vec<String>>` of output lines, or a report struct that a renderer turns into lines/JSON. |
| Presentation (binary only) | `prompt`, `ui::*` | Interactive pickers and the TUI dashboard. Both call the same `ops` functions the CLI does. |

`ops/mod.rs` also hosts the shared table renderer (`table`, `styled_table`)
built on `comfy-table`.

## CLI dispatch

`main()` parses `Cli`, calls `run`, prints each returned line to stdout, and on
error prints `Error: <message>` to stderr and exits with `err.exit_code()`.

`run` dispatches by subcommand:

- **No subcommand** opens the dashboard when stdin and stdout are terminals;
  otherwise it emits clap's missing-subcommand error (exit 2) so scripts see
  a normal usage error.
- **`init`** and **`config`** run without locating a hub (`config` works
  outside any hub).
- Every other command calls `locate()`, which runs `Hub::locate` and then
  `skill::refresh` to rewrite the generated agent skill in every hub worktree;
  skill problems become `warning:` lines on stderr so `--json` stdout stays
  clean (see [Bundled agent skill](../integrations/agent-skill.md)).
- Mutating commands run inside `hub.with_lock(...)`. Interactive prompts for
  `feature start` (roles, then which other roles to freeze) and `feature add`
  (role, branch, and for a new branch of a frozen role whether to start from
  the pin or the latest base) run **before** the lock is taken, so cancelling
  touches nothing. In a terminal `feature add` always goes through its prompt,
  even when the role and branch are given.
- `status` prints its table or JSON itself and then returns `HubError::Drift`
  when the report has structural drift, giving exit 30 after full output.
- `merged` and `finish` receive a `confirm_removal` callback that asks on a
  terminal before removing a worktree the hub did not create and answers "no"
  when stdin is not a terminal.

## Errors and exit codes

`HubError` (`src/error.rs`) is the single error type; `hub::Result<T>` aliases
it.

| Variant | Meaning | Exit |
|---|---|---|
| `Usage` | Bad arguments or a missing prerequisite the user controls | 1 |
| `Precondition` | A state check failed; nothing changed | 1 |
| `Malformed` | `hub.json` or a feature record failed validation | 1 |
| `Command` | A git/tmux/editor subprocess exited non-zero | 1 |
| `Io` | Filesystem error with context | 1 |
| `Dirty` | Worktree has uncommitted changes and `--force` not given | 20 |
| `InUse` | A process still has its cwd inside a worktree to remove | 21 |
| `Drift` | `status` found a structurally inconsistent row | 30 |

Clap itself uses exit 2 for argument errors.

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](../workflows/frozen-bases.md)
- [Hub state, manifest, and locking](hub-state-and-locking.md)
- [Dashboard architecture](../ui/dashboard-architecture.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
