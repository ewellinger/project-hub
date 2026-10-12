---
type: workflow
title: Status, list, and pull
description: How `hub status` compares recorded feature or base state with git (rows, drift and exit 30, merge hints, base lag, the Completed table, JSON fields), how `hub feature list` summarizes features, and how `hub pull` fast-forwards checkouts concurrently while refusing anything that is not a clean fast-forward.
tags: [workflow, status, drift, list, pull, json, concurrency]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
sources:
  - id: openwiki-source-a3a32cb165bec89fdae31522
    resource: repo://src/git.rs
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-cf7a9f9d4f3a7dc4afdc4d25
    resource: repo://src/ops/list.rs
  - id: openwiki-source-f9c1270d031f29e27a646bcd
    resource: repo://src/ops/pull.rs
  - id: openwiki-source-c73c116b0313275504b5f637
    resource: repo://src/ops/status.rs
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# Status, list, and pull

These commands read reality and report it. `status` and `list` take no lock and
change no hub state; `pull` runs under the lock but only fast-forwards
checkouts. The dashboard's status pane calls the same `ops::status` functions
(see [Dashboard architecture](../ui/dashboard-architecture.md)).

## Choosing a mode

`hub status [--feature N] [--base] [--json]` (and `hub pull` likewise) reports
the **feature** when `--feature` is given or the cwd is inside a feature's hub
worktree, unless `--base` is passed; otherwise it reports the **base
checkouts**.

## Feature status (`ops::status::status`)

Produces a `StatusReport { feature, checkout, status, rows, warnings, drift }`:

- **Row 0 is the hub worktree**: path, whether it exists, is registered with
  git, and is on the feature branch; dirtiness; ahead/behind against its origin
  copy (so a pushed teammate change shows as `behind`).
- **Then one row per change, in record order** (merged ones included). Paths
  come from the change's recorded clone, so rows survive `hub repo remove`.
  The dashboard relies on this positional order.

Per change row:

| Field | Meaning |
|---|---|
| `exists`, `registered`, `branch_matches` | directory present; worktree registered (main-clone changes count as registered); checkout is on the recorded branch |
| `dirty` | uncommitted changes |
| `ahead`, `behind` | vs. `origin/<branch>`; `null` without a remote copy |
| `base`, `custom_base` | effective base of an open change; whether it differs from the role's manifest base |
| `base_behind` | commits on `origin/<base>` the branch lacks; `null` (plus a warning) when `origin/<base>` is missing |
| `hint` | merge hints (below) |
| `review_url` | recorded MR/PR URL |
| `drift` | structural problem |

**Drift** = an *open* change (in an open feature) whose checkout is missing,
unregistered, on the wrong branch, or whose clone is missing; for the hub row,
an open feature whose hub worktree is missing/unregistered/off-branch. Merged
changes and finished features cannot drift. Dirtiness and base lag are
operational state and never drift. If any row drifts, `main.rs` prints the
output and exits **30**.

**Merge hints** (open changes): "merged into origin/<base>; run hub feature
merged ROLE" when the branch is an ancestor of `origin/<base>` and has work
beyond `base_sha`; "at base tip, no commits of its own" for a branch that was
merely fast-forwarded to a moved base (at the base tip, absent from origin,
never seen there); "gone from origin; run hub feature merged ROLE" when hub saw
the branch on origin earlier (`origin_seen`) and a pruning fetch removed it.
See [Features, changes, stages, and bases](../concepts/features-changes-and-stages.md).

**Fetching**: each clone with an open change is fetched once with
`git fetch --prune origin`, and the hub's origin is fetched too. Changes are
grouped by clone: different clones are processed concurrently on scoped
threads; changes sharing a clone run sequentially. A failed fetch is a
warning and the last fetched refs are used.

## Base status (`ops::status::base_status`)

Row 0 is the hub's main clone on `main` (ahead/behind against origin); then
one row per registered role's **main clone**, in manifest order, comparing it
with the configured base branch. A missing clone or a clone not on its base is
drift. All repos and the hub are fetched concurrently.

## Rendering

The human table (`render_table`) prints a header (feature, status, tmux session
only when tmux is on), the open rows, then a separate **`Completed`** table
(role, branch, stage) for merged rows — or every row of a finished feature —
since their worktrees are gone by design. A `BASE` column appears only when
some change has a custom base; nonzero lag shows as `behind base <branch> by
<n>` in yellow. Paths under `$HOME` print as `~/…`. Warnings follow.

`--json` serializes the full report; `rows` always contains every row with
absolute paths, which is the supported machine interface (the agent skill
reads paths from `rows[].path`).

## `hub feature list [--all] [--json]` (`src/ops/list.rs`)

One `FeatureSummary { name, checkout, status, roles }` per feature, open
features first, then finished, each by name. `roles` holds the **latest**
change per role in manifest order, with roles since removed from `hub.json`
appended. Without `--all`, finished features are hidden (with a note when that
leaves nothing). It reads records only — no git calls.

## `hub pull [--feature N] [--base]` (`src/ops/pull.rs`)

Fast-forward only. From the main hub (or with `--base`), `pull_base` targets
every registered role's main clone on its base branch; from a feature (or with
`--feature`), `pull_feature` targets each **open** change's checkout on its
branch, in manifest order. The hub's own clone is never pulled.

For each target (`pull_one`):

1. missing checkout → skipped; fetch fails → skipped;
2. not on the expected branch, or detached → skipped;
3. no `origin/<branch>` → skipped;
4. not behind → "up to date" (noting how far ahead, if at all);
5. ahead and behind → skipped as diverged ("merge or rebase by hand");
6. dirty → skipped;
7. otherwise `git merge --ff-only origin/<branch>`.

Nothing is merged, rebased, stashed, or committed, and feature records are
untouched. Targets are grouped by clone so repos run concurrently while
checkouts of the same repo run sequentially (concurrent fetches race on ref
locks); lines come back in input order.

`pull` never brings `origin/<base>` into a feature branch; `status` only
measures that lag. Updating from the base is a separate, explicit agent-skill
procedure (see [Bundled agent skill](../integrations/agent-skill.md)).

## Tests

`tests/list_status.rs` (drift, hints, base lag, Completed table, JSON shape,
base status), `tests/pull.rs`; `pull.rs` has unit tests for grouping and each
skip case.

## Related pages

- [Review, merged, set-base, and finish](review-merged-finish.md)
- [Dashboard architecture](../ui/dashboard-architecture.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
