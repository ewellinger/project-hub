---
type: workflow
title: Review state from the forge
description: How hub reads the state of a role's merge request (GitLab) or pull request (GitHub) through `glab` / `gh` — status, approval, pipeline, unresolved threads, conflicts — remembers it per feature, and shows it in `hub status`, `hub feature list`, and the dashboard; when the forge is read, and why a failed read never fails a command.
tags: [workflow, review, forge, gitlab, github, status, cache]
sources:
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-cf7a9f9d4f3a7dc4afdc4d25
    resource: repo://src/ops/list.rs
  - id: openwiki-source-1da066772d42f80cecf3225c
    resource: repo://src/ops/review_state.rs
  - id: openwiki-source-c73c116b0313275504b5f637
    resource: repo://src/ops/status.rs
  - id: openwiki-source-35c9fd26cbbe5a478a01ee5a
    resource: repo://src/ops/viewed.rs
  - id: openwiki-source-4b4dd1a8424927498a257bc6
    resource: repo://src/review_state.rs
generated: { by: "claude-code", at: "2026-11-07T20:01:15.981Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-11-07T20:01:15.981Z
---

# Review state from the forge

A role in stage `review` usually has a merge request (GitLab) or pull request
(GitHub) behind it, recorded as the change's `review_url` (see
[Review, merged, set-base, finish, and remove](review-merged-finish.md)). hub
reads that request's state from the forge, remembers the last answer, and shows
it, so a pile of features can be scanned without opening each request.

Two modules own this:

| Module | Responsibility |
|---|---|
| `src/review_state.rs` | The typed state (`Fields`, `ReviewState`), pure JSON parsers for both forges, and `fetch_state`, the forge read |
| `src/ops/review_state.rs` | Which roles are read (`targets`), the remembered store (`remembered`, `refresh`, `store`, `refresh_all`), filling a status report (`attach`), what moved since a viewed mark (`attention`), and the rendering shared by the CLI and the dashboard (`review_cells`, `section_lines`, `word`) |
| `src/ops/viewed.rs` | `hub feature viewed ROLE`: one forge read, then the viewed mark |

## Which roles are read

`targets(feature)` yields one target per **open change in stage `review` of an
open feature whose `review_url` parses** with `review::parse_review_url`
(`https://HOST/GROUP/PROJECT/-/merge_requests/N` or
`https://HOST/OWNER/REPO/pull/N`). A role still `working`, a role in review
with no URL, a URL in any other shape, and every role of a finished feature
are not targets: they are skipped silently, with no row and no warning.

## The state

`Fields` holds what one read yields. Every field is optional: `None` means the
forge did not say, shown as `?`.

| Field | Values | GitLab source | GitHub source |
|---|---|---|---|
| `status` | `open`, `draft`, `merged`, `closed` | `state` (`opened`/`locked` are open), with `draft` or `work_in_progress` | `state` with `isDraft` |
| `approved` | `yes`, `changes`, `no` | `changes` when `detailed_merge_status` is `requested_changes`; else `/approvals` | `reviewDecision`; when that is null, the latest opinionated reviews |
| `pipeline` | `passed`, `failed`, `running`, `none` | `head_pipeline.status` (`canceled` counts as failed, `skipped` and no pipeline as none) | the head commit's `statusCheckRollup.state` |
| `threads` | count of unresolved threads | discussions with a note that is resolvable and unresolved | `reviewThreads` nodes with `isResolved` false |
| `threads_more` | bool | the page of 100 discussions came back full | `totalCount` exceeds the nodes returned |
| `conflicts` | bool | `has_conflicts`, unknown while `merge_status` is `unchecked`/`checking` | `mergeable` (`CONFLICTING`/`MERGEABLE`; anything else unknown) |
| `head` | 40-hex commit, lowercased | the merge request's `sha` | `headRefOid` |
| `comments` | a count that rises with every comment or reply | non-system notes across the first page of 100 discussions | `comments.totalCount` + `reviews.totalCount` (every review-thread reply is wrapped in a review); unknown unless both are present |

Two details in the parsers exist because the forges' own flags mislead:

- **GitLab approval needs an approver.** `/approvals` answers
  `"approved": true` for a project that requires zero approvals even when
  nobody approved. `parse_gitlab_state` reports `yes` only when `approved` is
  true **and** `approved_by` is not empty.
- **GitHub without required reviews** has a null `reviewDecision`, so the
  parser falls back to `latestOpinionatedReviews`: any `CHANGES_REQUESTED`
  gives `changes`, else any `APPROVED` gives `yes`, else `no`.

`head` is kept only when it is a 40-character hex string. The GitLab comment
count is a known shortcut: it counts the first 100 discussions only.

`ReviewState` is `Fields` plus `url` (the recorded `review_url` exactly as it
was when read), `read_at` (Unix seconds), and `viewed`: `null`, or the
`Viewed { at, head, comments }` snapshot that `hub feature viewed` recorded.

## The forge read (`fetch_state`)

All access goes through the forge CLIs with whatever authentication they
already have; hub stores no tokens (see
[Git, tmux, and editor integration](../integrations/git-tmux-editor.md)).

- **GitLab**: three `glab api --hostname HOST --method GET` calls run at the
  same time on scoped threads: the merge request, its `/approvals`, and
  `/discussions?per_page=100`. Only the merge request itself must answer. If
  approvals or discussions fail, their fields become unknown and the read
  still succeeds.
- **GitHub**: one `gh api graphql --hostname HOST` call with a fixed query
  (state, draft, mergeable, reviewDecision, `headRefOid`, the comment and
  review totals, latest opinionated reviews, review threads, and the head
  commit's status check rollup), because unresolved review threads exist only
  in GraphQL.

There is no timeout on these calls beyond the programs' own.

## The remembered store

The last answer per role lives in `<git common dir>/hub/reviews/<feature>.json`
(`Hub::reviews_path`), a JSON map from role to `ReviewState`. It is separate
from the feature record on purpose: status takes no lock, so it must not write
the record (see [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)).

- `remembered(hub, feature)` returns the stored state of each current target
  **whose stored `url` equals the recorded one**. A changed URL therefore makes
  the old answer disappear rather than describe the wrong request. A missing or
  corrupt file reads as empty.
- `refresh(hub, feature)` starts from `remembered`, reads every target
  concurrently, replaces the entries that answered, and saves through `store`.
  A target whose read failed **keeps its remembered entry** and adds a
  warning.
- `store(hub, feature, states, marking)` creates the `reviews/` directory if
  needed and, under a blocking lock on `<feature>.lock`, rereads the file,
  carries each role's `viewed` mark from disk (except `marking`, the role
  `hub feature viewed` is marking now), keeps roles this run could not read,
  and writes. Status runs without the hub lock, so without this a slow
  `hub status` that started before a mark would overwrite it with `null`.
- `refresh_all(hub)` runs `refresh` for every open feature that has a target,
  eight features at a time, because each GitLab role runs three `glab`
  processes and one thread per feature could exhaust file descriptors.
- `hub feature remove` deletes the file and its lock with the record.

## Nothing here fails a command

Forge and store problems become warnings; they never change an exit code:

| Problem | Result |
|---|---|
| `glab` / `gh` not on `PATH` | one warning per program, however many roles need it: `… is not on PATH; install it and run … auth login --hostname HOST` |
| A read fails for one role | `review state not read for ROLE: <first line of the error>`; the row keeps its remembered answer, or shows `?` in every column |
| The reviews file cannot be written | `review state not saved: …` |

Warnings are sorted and de-duplicated. `refresh_all` itself can only fail when
the feature list cannot be loaded, and `hub feature list --refresh` turns even
that into a warning.

## `attach`: filling a status report

`ops::status::status` and `base_status` never read the forge and never write a
file. Review state is added afterwards by
`attach(hub, feature, &mut report, source)`:

- `Source::Forge` calls `refresh`; `Source::Remembered` calls `remembered` and
  can produce no warnings.
- For each target it finds the report row with the same role in stage
  `review`, sets `row.review`, and appends the warnings to the report.
- When the request is **merged**, the row's hint becomes
  `merged on GitLab|GitHub; run hub feature merged ROLE`. When it is
  **closed**, the hint becomes `closed on GitLab|GitHub without merging; run hub
  feature dropped ROLE`. Neither command runs automatically.
  Either replaces whatever hint git produced for that row.
- Otherwise, when the row has **no hint** yet and the review has moved since
  its viewed mark (`attention`), the hint becomes `pushed since viewed` or
  `N replies since viewed` (`1 reply since viewed`). Closing the change wins
  over "look again".

Because `attach` is a separate step, the callers of `status` that only want a
drift check (such as `start-review`) cannot reach the forge.

## Marking a review viewed (`hub feature viewed ROLE`)

`hub feature viewed ROLE` (dashboard `V`) runs under the hub lock and records
the point the user last looked at a review, so later reads can say what moved:

1. The feature must be open and `ROLE` must be one of its targets (in
   `review` with a parseable URL); otherwise it fails with `role 'ROLE' in
   feature 'F' is not in review with a merge/pull request URL`.
2. It runs `refresh` for the feature, which reads the forge for every target.
   If `ROLE`'s entry was not freshly read (its `read_at` predates the call), it
   fails with that role's warning, so a mark is never taken from a stale
   answer.
3. It sets `viewed = { at: read_at, head, comments }` on that entry and saves
   through `store` with `marking = ROLE`. It never changes the feature record.
4. It prints `ROLE: viewed !N head <short sha>, <n> comments` (`#N` on
   GitHub; `?` for an unknown head or count).

`attention(state)` compares the latest read with the mark: a different `head`
is **pushed**; otherwise a higher `comments` count is **N replies** (the
difference). Without a mark, with both sides equal, with a lower count, or
when either side is unknown, nothing is flagged. A changed URL drops the mark
along with the rest of the remembered entry. Nothing clears a mark except
marking again.

## When the forge is read

| Trigger | Reads |
|---|---|
| `hub status` for a feature | that feature, always |
| `hub feature list --refresh` | every open feature with a target, before listing |
| Dashboard `Enter` on a feature, `r` | that feature |
| Dashboard `R` | every open feature with a target |
| `hub feature viewed ROLE`, dashboard `V` | that feature |

Never: on cursor movement in the dashboard, on a timer, for
`hub status --base`, for a plain `hub feature list`, or from a command that
runs status for its own checks. Those show the remembered answer and its age.
The reads touch the forge only; there is no `git fetch`. The dashboard side is
in [Dashboard architecture](../ui/dashboard-architecture.md) and
[Dashboard flows and overlays](../ui/dashboard-flows-and-overlays.md).

## How it is shown

### The review section

`hub status` prints a second table under the role table, only when some row is
in review with a parseable URL (`section_lines`); otherwise the output is what
it was before this feature existed.

```
REVIEW  REQUEST  STATUS  APPROVED  PIPELINE  THREADS  CONFLICTS
api     !412     open    no        failed    2 open   no
bff     !87      merged  -         passed    -        -
web     #31      open    yes       passed    0 open   yes

review state read 4m ago
```

`review_cells` builds each row: REQUEST is `!N` for GitLab and `#N` for GitHub;
an unknown field is `?`; APPROVED, THREADS, and CONFLICTS are `-` for a merged
or closed request, where they no longer apply; THREADS reads `N open`, or
`N+ open` when `threads_more` is set. The age line uses the **oldest**
`read_at` among the rows, so one stale row is not hidden by fresh ones. The
dashboard draws the same cells in its own table.

`hub status --json` carries `rows[].review`: `null`, or the `ReviewState`
object (`url`, `read_at`, the fields flattened including `head` and
`comments`, and `viewed`, which is `null` until marked).

### The list word

`hub feature list` and the dashboard's feature list show one word per role in
review, from the remembered state only: the first of these that applies
(`word`).

| Order | Word | Applies when | Colour |
|---|---|---|---|
| 1 | `merged` | status is merged | green |
| 2 | `closed` | status is closed | red |
| 3 | `pushed` | the head moved since the viewed mark | cyan |
| 4 | `N replies` (`1 reply`) | the comment count rose since the viewed mark | cyan |
| 5 | `conflicts` | conflicts is true | red |
| 6 | `ci failed` | pipeline failed | red |
| 7 | `N threads` (`1 thread`) | at least one unresolved thread | yellow |
| 8 | `changes` | changes were requested | yellow |
| 9 | `ci running` | pipeline running | yellow |
| 10 | `approved` | approved is yes | green |
| 11 | `draft` | status is draft | none |

An open request with nothing to report has no word. In the CLI table the word
follows the stage, as `api:review(ci failed)`; in JSON it is `roles[].review`.
`ops::list::list` takes it from `remembered`, so a plain list makes no forge
call, and a finished feature shows none.

## Tests

- Unit tests in `src/review_state.rs` cover both parsers, including answers
  captured from real requests on both forges (among them a GitLab project
  that requires no approvals).
- Unit tests in `src/ops/review_state.rs` cover the word order (including
  where `pushed` and `N replies` fall), `attention`, the cells, and the age
  line.
- `tests/review_state.rs` drives the binary against the fake `glab`/`gh` (see
  [Testing and the integration harness](../testing/integration-test-harness.md)):
  the store, the status section and hints, JSON, the list word, `--refresh`,
  `feature viewed` and the `pushed` / `N replies` words and hints, marks
  surviving a concurrent `status` write (raced with `FAKE_PROVIDER_SLEEP`),
  and that `status --base` and `start-review`'s drift check make no review
  state call.

## Related pages

- [Status, list, and pull](status-and-pull.md)
- [Reviewing a merge or pull request](start-review.md)
- [Review, merged, set-base, finish, and remove](review-merged-finish.md)
- [Hub state, manifest, and locking](../architecture/hub-state-and-locking.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
- [Dashboard flows and overlays](../ui/dashboard-flows-and-overlays.md)
