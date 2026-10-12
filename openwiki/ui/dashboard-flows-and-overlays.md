---
type: ui
title: Dashboard flows and overlays
description: How the dashboard's multi-step interactions work — the Flow state machines (new-feature wizard, add role, review, set-base, merged and finish previews, copy), the Overlay widgets and their key handling, the feature screen's keys and cursor mapping, and the busy, cancel, and blocked rules.
tags: [ui, tui, dashboard, flows, overlays, keys]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
sources:
  - id: openwiki-source-79a1d0ec63169ed8c7724b38
    resource: repo://src/ui/app/feature.rs
  - id: openwiki-source-fdbe59e7e6e7d44bcc810416
    resource: repo://src/ui/app/flow.rs
  - id: openwiki-source-7bdce6d8095dc0966e3f6005
    resource: repo://src/ui/app/mod.rs
  - id: openwiki-source-5b31f6bedf9e4f28fd8d628a
    resource: repo://src/ui/app/overlay.rs
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# Dashboard flows and overlays

Interactive actions in the dashboard are built from two pieces in
`src/ui/app/`: **overlays** (`overlay.rs`), modal widgets that collect one
answer, and **flows** (`flow.rs`), state machines that decide what to show
next. Both are pure and driven by `App::update` (see
[Dashboard architecture](dashboard-architecture.md)).

## Overlays (`overlay.rs`)

| Overlay | Use | Keys |
|---|---|---|
| `Select` | single choice with type-to-filter | ↑/↓, typing filters, Backspace, Enter picks; while `loading`, only Esc works |
| `MultiSelect` | choose several rows (roles) | ↑/↓/j/k, Space toggles, Enter answers `PickedMany` |
| `Input` | free text (name, URL, branch) with optional preview and inline error | typing, Backspace, Enter; while `busy`, only Esc works |
| `Confirm` | preview of what an action will do | Enter confirms; `F` confirms with force when offered; when `blocked`, Enter and `F` are inert |
| `Result` | multi-line action output or error | Enter or Esc dismisses |
| `Help` | key list for the current screen | `?`, `q`, Esc close |

`Overlay::key` returns `Outcome::Stay`, `Cancel`, or `Answer(..)`. **Esc in any
overlay cancels the whole flow** with nothing changed (footer: `cancelled`);
`Ctrl-C` always quits the app.

A `Select` may have a **pinned** row (the branch picker's "type another name")
that the filter never hides. Pressing Enter on it while a filter is typed
answers `Text(filter)`, which opens the name input prefilled with the typed text.

## Flows (`flow.rs`)

`Flow::advance(answer, app)` returns a `Step`:

- `Show(overlay)` — display and wait;
- `ShowAndLoad(overlay, effect)` — show a spinner/busy overlay and dispatch a
  worker effect, resuming on `Answer::Loaded`;
- `Run(action)` — finish by running a hub action under the lock;
- `Effect(effect)` — finish with a lock-free effect (clipboard copy);
- `Cancel(message)` — end with a footer error;
- `Ignore` — a stale worker result not meant for this flow; the current overlay
  and flow are restored untouched.

| Flow | Trigger | Steps → action |
|---|---|---|
| `NewFeature` (wizard) | `n` on dashboard | name input → `Preflight` (resolves the checkout name off-thread; errors stay on the name step) → role multi-select (none = docs-only start; skipped when the hub has no repos) → branch pick per role → summary confirm → `Action::Start` |
| `AddRole` | `+` | select among roles without an open change (cancel if none) → branch pick → `Action::Add` |
| `Review` | `v` | URL input prefilled with the current `review_url`; empty means none → `Action::Review` |
| `SetBase` | `b` | input prefilled with the effective base; rejects empty or `origin/`-prefixed input inline → `Action::SetBase` |
| `Merged` | `m` | confirm preview: change to close, worktree to remove (path, owner, state), branch kept; warnings for dirty or adopted worktrees; `F` forces → `Action::Merged` |
| `Finish` | `f` | confirm preview: tmux session to kill (when tmux is on), every worktree to remove including the hub worktree, branches kept, warnings for unmerged roles and dirty worktrees; `F` forces → `Action::Finish` |
| `Copy` | `c` | select among branch, worktree path (if it exists), review URL (if any); options snapshotted at Begin so a refresh cannot retarget them → clipboard `Effect::Copy` |

### Branch picking (`BranchPick`)

Shared by the wizard and add-role. It dispatches `LoadBranches`, whose worker
builds rows with `ops::suggest` (the default branch first, marked `new` if it
does not exist, then branches git already knows, then a free-text row — the
same rows as the CLI prompt; see
[Starting features and adding roles](../workflows/feature-start-and-add.md)).
A load result for a role the user has backed away from is ignored and the
spinner kept. Choosing the free-text row opens a name input whose value is
validated before running.

### Stale-result correlation

The wizard records the name whose preflight is pending, and ignores a
`Loaded::Checkout` or preflight error for any other name or after the stage
moved on. This prevents an abandoned wizard's late reply from tearing down
whatever the user is doing now.

### Finishing from inside its own session

If the dashboard runs inside the tmux session that `finish` would kill
(`App.current_session == record.checkout`), the finish confirm is **blocked**:
the overlay explains why and names the CLI command to run from another
terminal (`hub feature finish --feature NAME`), and Enter/`F` do nothing.

Because confirmation happens in these previews, the worker passes an
always-yes confirm callback to `ops::merged` / `ops::finish`, so adopted
worktrees shown in the preview are removed without a second prompt.

## The feature screen (`feature.rs`)

The feature screen shows one open feature's status report: the hub row, then
open changes, then merged ones (`display_order`). The cursor stores an index
into `feature.changes`, which is append-only and therefore stable, and maps to
status row `index + 1` **positionally** — not by role name, since a role that
was merged and reopened has two rows.

Keys: `↑↓/jk` move within display order; `r` refresh; `o` open the workspace;
`p` pull the feature; `v`/`b`/`m` start the per-role flows; `+` add role;
`f` finish; `c` copy; `?` help; `Esc`/`q` return to the dashboard (and reload
the list).

Per-role actions (`v`, `b`, `m`) go through `role_action`, which refuses with a
footer message when an action is already running, no status is cached yet
("status not loaded yet; press r"), or the selected change is merged. `+`,
`f`, `c`, and `?` likewise refuse while busy or before status loads.

## Rendering

`src/ui/view/overlay.rs` draws each overlay centered over the screen with
tone-colored lines (`Plain`, `Warn`, `Error`, `Dim`) and sizes boxes from the
wrapped line count. `src/ui/view/feature.rs` draws the role table with the
cursor, followed by warnings, hints, the selected role's review URL, the attach
line (when tmux is on), and the refresh age, reusing helpers from
`view/dashboard.rs`.

## Related pages

- [Dashboard architecture](dashboard-architecture.md)
- [Starting features and adding roles](../workflows/feature-start-and-add.md)
- [Review, merged, set-base, and finish](../workflows/review-merged-finish.md)
