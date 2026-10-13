---
type: architecture
title: Dashboard architecture
description: How the ratatui dashboard (bare `hub` in a terminal) is structured — an event loop around a pure `App::update(Event) -> Vec<Effect>` state machine, background worker threads for status, actions, editor, and clipboard, generation-tagged status requests, and pure rendering — and how dashboard actions reuse the same `ops` functions under the hub lock.
tags: [ui, tui, ratatui, dashboard, architecture, concurrency]
sources:
  - id: openwiki-source-c55e62404e7071e7e42c91b9
    resource: repo://src/ui/app/dashboard.rs
  - id: openwiki-source-7bdce6d8095dc0966e3f6005
    resource: repo://src/ui/app/mod.rs
  - id: openwiki-source-e9fb0e624f38b4189cab5a2d
    resource: repo://src/ui/mod.rs
  - id: openwiki-source-39beb1223f2645ced84e1825
    resource: repo://src/ui/view/dashboard.rs
  - id: openwiki-source-d36a1897ad2613d7d98e64cb
    resource: repo://src/ui/worker.rs
generated: { by: "claude-code", at: "2026-10-13T22:33:09.880Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-13T22:33:09.880Z
---

# Dashboard architecture

Bare `hub` in a terminal opens a TUI dashboard (see
[Architecture overview](../architecture/overview.md) for how `main.rs`
decides). The code lives in the binary-only `src/ui/` tree, the only place
that imports `ratatui` and `crossterm`. It is split into three layers:

| Layer | Path | Responsibility |
|---|---|---|
| State | `src/ui/app/` | `App` and the pure `update` function. No terminal, threads, or git, so it is unit-tested with synthetic events. |
| Side effects | `src/ui/worker.rs` | Everything that touches git, the filesystem, the editor, or the clipboard, on detached threads. |
| Rendering | `src/ui/view/` | Pure functions from `&App` to a ratatui `Frame`. |

`src/ui/mod.rs` wires them together.

## Startup and the event loop

`ui::run(hub)`:

1. Loads the feature list (`ops::list::list`) and builds `App` with the
   manifest repos, checkout template, whether tmux is on, and the tmux session
   the dashboard itself runs inside (used to refuse finishing that session).
2. `App::start(cwd_feature)` returns the initial effects: when launched from a
   feature's hub worktree it opens that feature's view directly; otherwise it
   requests the base row's status.
3. `ratatui::try_init()` takes over the terminal (it also installs a panic
   hook that restores it); `ratatui::restore()` runs on exit.

`event_loop` repeats:

1. **Run effects** from a `VecDeque<Effect>` (`run_effects`). An effect whose
   result feeds straight back (e.g. `ReloadList`) may queue more effects.
2. **Draw** with `view::render(app, frame)`; return if `app.quit`.
3. **Collect events**: one key press (poll timeout 100 ms, otherwise a `Tick`
   for the spinner), then drain every worker result from the `mpsc` channel.
4. **Update**: feed each event to `app.update(event)` and append the returned
   effects.

The manifest is read once at startup, so `repo add/remove/set/rename` require
restarting the dashboard.

## Events and effects

`Event` is everything that can happen: `Key`, `Tick`, and worker results
(`Report`, `List`, `Opened`, `Copied`, `Done`, `Branches`, `Preflighted`).
`Effect` is everything the loop must do on the app's behalf: `Request` (status
for a row), `OpenWorkspace`, `Copy`, `ReloadList`, `Run(Action)`,
`LoadBranches`, `Preflight`. `update` never performs side effects itself.

`Action` is one hub operation with resolved arguments — `Start`, `Add`
(carrying `from_latest` for a frozen role), `Review`, `SetBase`, `Merged`,
`Freeze`, `Unfreeze { force }`, `Refreeze`, `Finish`, `Remove`, `Pull { row }` — and is
the unit the worker executes. The dashboard never passes `--at`: `Freeze`
always pins the local base tip.

## Status requests and caching

Rows are the base row (`RowId::Base`) followed by features, with finished
features hidden unless toggled. For each row the app keeps the last good
report (`cache`), the newest outstanding request generation (`in_flight`),
and the last error (`errors`).

- `request(id)` does nothing if a request for that row is already in flight;
  `request_fresh(id)` always issues one with a new generation (used after an
  action, since an earlier reply reflects pre-action state).
- A `Report` is applied only if its generation matches `in_flight` for that
  row and the row still exists; stale replies are dropped, so a slow thread
  that outlives its request is harmless.
- While a request runs, the last cached result stays visible.
- `rebuild_rows` prunes cache/in-flight/error state for features that left
  the list.

## Worker threads (`src/ui/worker.rs`)

Each effect spawns a detached thread that sends exactly one event back:

- `request` runs `ops::status::base_status` or `ops::status::status` for a
  feature — the same code as `hub status`.
- `run(action)` executes the action **inside `hub.with_lock`**, loading the
  feature record inside the lock so it sees current state rather than the
  dashboard's cache, and calls the same `ops::*` functions as the CLI. Panics
  are caught with `catch_unwind` and turned into a `Precondition` error, so a
  panicking op cannot leave the dashboard stuck "busy". Merged/finish pass a
  confirm callback that always says yes; confirmation happens earlier, in the
  dashboard's preview overlay (see
  [Dashboard flows and overlays](dashboard-flows-and-overlays.md)).
- `open_workspace` writes the workspace file and launches the editor with
  silenced stdio (the terminal is in raw mode).
- `copy` uses `arboard` with one process-wide clipboard handle, because on X11
  the owning process must stay alive to serve the selection; a failed handle
  is dropped and reconnected on the next copy.
- `preflight` and `load_branches` feed the new-feature and add-role flows.

## One action at a time

`App.running` holds the action in flight. `App::run` refuses a second one
with `busy: <action>…` in the footer. `q`/`Esc` refuse to quit while an action
runs (`Ctrl-C quits anyway`); `Ctrl-C` always quits immediately, killing the
worker with the process. When an action completes (`done`), a single-line
result goes to the footer, multi-line output or an error opens a result
overlay, and the affected row is refreshed with `request_fresh` (or the list
reloaded after `start`/`finish`, and after every `remove`, whether it
succeeded or was refused; finishing the open feature returns to the
dashboard).

## Rendering

`view::render` draws a header, the active screen, a footer (messages expire
after 3 s), and any overlay on top. The dashboard screen has the row list on
the left and the selected row's status on the right (`view/dashboard.rs`),
including report age and a spinner while refreshing; the feature screen is in
`view/feature.rs`. There are minimum terminal dimensions (`MIN_WIDTH`,
`MIN_HEIGHT`).

Both screens share `status_table`: ROLE, BRANCH, STAGE, STATE, PATH. BRANCH
is a `Length` column sized to the longest branch and is never truncated;
STATE is a `Max` column capped at 56 so a pinned change's whole state (such as
`behind pin <sha> by <n>, <base> +N since pin`) fits when there is room, and it
gives way first when the terminal is narrow. Merged rows are dimmed, and
frozen reference rows use the read-only style (dim + italic). The dashboard
list pane also lists each open feature's frozen roles, after its members, in
the read-only style.

In that list pane each role line (`stage_line`) colours only the stage text:
`working` green, `review` yellow, `frozen` dark grey, any other stage the
default. Merged roles are left out of an open feature's lines, and a finished
feature is a single dim `<name>  finished` line. On the selected row the
`REVERSED` highlight turns the colour into the stage's background.

## Dashboard keys

`app/dashboard.rs`: `↑↓/jk` select (and request status), `Enter` opens a
feature view (refused on the base row and finished features), `n` starts the
new-feature flow, `a` toggles finished features, `r` reloads, `o` opens the
workspace, `p` pulls the selected row, `x` removes the selected finished
feature after a confirmation (refused in the footer on the base row and on
open features), `?` shows help. Feature-screen keys are
covered in [Dashboard flows and overlays](dashboard-flows-and-overlays.md).

## Related pages

- [Frozen bases: freeze, unfreeze, and refreeze](../workflows/frozen-bases.md)
- [Dashboard flows and overlays](dashboard-flows-and-overlays.md)
- [Status, list, and pull](../workflows/status-and-pull.md)
- [Architecture overview](../architecture/overview.md)
