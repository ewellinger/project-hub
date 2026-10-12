---
type: configuration
title: Environment and configuration
description: How `hub` resolves project_home, worktree_dir, editor, and tmux from environment variables and `~/.config/hub/config.toml`, how values are validated, how `hub config` shows and edits the file, and how tmux sessions are switched on or off.
tags: [configuration, env, config-toml, tmux, editor]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
sources:
  - id: openwiki-source-a3b2fe9fb2fafbad4d2be800
    resource: repo://src/env.rs
  - id: openwiki-source-a9fddfa72ce7af0e031fedf8
    resource: repo://src/hub.rs
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-9eaa593f7abd794d0feb165d
    resource: repo://src/ops/config.rs
  - id: openwiki-source-1b5cfaa33f22a91204f59602
    resource: repo://src/tmux.rs
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# Environment and configuration

Every hub command needs two directories: **`project_home`**, where member
repos' main clones live (`<project_home>/<clone>`), and **`worktree_dir`**,
where worktrees live (`<worktree_dir>/<clone>/<name>`, and the hub's own at
`<worktree_dir>/<hub dir>/<checkout>`). Resolution lives in `src/env.rs`;
the `hub config` command lives in `src/ops/config.rs`.

## Sources and precedence

| Key | Environment override | File key | Default |
|---|---|---|---|
| `project_home` | `PROJECT_HOME` | `project_home` | none — required |
| `worktree_dir` | `GIT_WORKTREE_DIR` | `worktree_dir` | none — required |
| `editor` | — | `editor` | `code` |
| `tmux` | — | `tmux` | auto: on when `tmux` is on `PATH` |

- The file is `$XDG_CONFIG_HOME/hub/config.toml`, or
  `~/.config/hub/config.toml` when `XDG_CONFIG_HOME` is unset, empty, or
  relative (the XDG spec says to ignore relative values). The path is the
  same on macOS and Linux.
- A missing file is an empty config. Unknown keys are **refused**
  (`deny_unknown_fields`) so a typo does not silently fall through.
- A variable wins only when it is set **and non-empty**.
- A leading `~` in file values is expanded against `HOME`.
- When either directory is missing from both sources, every command fails
  with a usage error that prints the config path and an example file.

`Env::resolve` then validates: both directories must be absolute and exist,
and are **canonicalized** so they compare equal to the paths git prints.
`editor` is split on whitespace and run without a shell (arguments work;
quoting and `$VAR` do not); an empty value is an error. Error messages name
the source of a bad value (`PROJECT_HOME` or `project_home in ~/.config/...`).

Tests construct environments with `Env::from_vars`, which uses only the
variables and the default editor.

## `hub config`

`hub config` runs without locating a hub. `ConfigFile::from_process` reads
the same path and variables as `Env`.

- `hub config [--json]` lists each key with its effective value and source:
  the overriding variable's name, `file`, `default`, or `unset`. Values are
  not validated here.
- `hub config set KEY VALUE` validates the value the way commands do
  (directories must exist and be absolute after `~` expansion; `editor` must
  name a command; `tmux` must be `true` or `false`), edits the file with
  `toml_edit` so comments and other keys survive, re-parses the result to
  refuse writing anything the next command could not read, and writes it
  atomically. If an environment variable overrides the key, it prints a note.
- `hub config unset KEY` removes the key; unknown keys are refused for both.

Coverage: `tests/config.rs`.

## Tmux on or off

`tmux::decide(setting, found, label)` is called from `Hub::locate` with the
config's `tmux` value and `tmux::on_path`, which scans `PATH` for an
executable named `tmux` without spawning anything:

| `tmux` setting | tmux on PATH | Result |
|---|---|---|
| `false` | any | off, reason `tmux = false in <config>` |
| unset or `true` | yes | on |
| unset | no | off, reason `tmux is not on PATH` |
| `true` | no | off, plus a one-time warning |

When off, `Hub.tmux` is `None` and `Hub.tmux_off` carries the reason.
`feature start`, `start-review`, and `add` print `tmux is off; no session
created`, `finish`/`merged` skip session steps, `status` and the dashboard
omit attach hints, and `hub tmux` refuses with `tmux is off (<reason>)`.
Worktrees, workspace files, and feature records are still created.
Coverage: `tests/tmux_off.rs`.

## Related pages

- [Architecture overview](../architecture/overview.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
