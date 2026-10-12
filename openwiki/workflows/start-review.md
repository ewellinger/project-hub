---
type: workflow
title: Reviewing a merge or pull request
description: How `hub feature start-review URL` turns a GitLab merge request or GitHub pull request into an isolated review feature — URL parsing and matching to exactly one role, reading the review through `glab api` or `gh api`, verifying the head, refusing forks, closed reviews, occupied or diverged branches and collisions, and conditional rollback.
tags: [workflow, start-review, gitlab, github, review, safety, rollback]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
sources:
  - id: openwiki-source-e782c5002a54f711ccbae710
    resource: repo://src/ops/start_review.rs
  - id: openwiki-source-61647d0ebb1f751a6acc22a7
    resource: repo://src/review.rs
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# Reviewing a merge or pull request

`hub feature start-review URL [--name N] [--checkout C]` creates a feature for
reviewing one role's merge/pull request: a new hub branch and worktree, the
review's **actual source branch** checked out at the provider-reported head in
a hub-owned worktree, base = the review's target, stage `review`, plus
workspace and session. The URL logic lives in `src/review.rs`; orchestration
and every safety check in `src/ops/start_review.rs` (the bundled agent skill
is explicitly not the safety boundary).

Unlike `feature start`, this command **never adopts** existing state: existing
records, hub branches, sessions, and checkouts are collisions.

## URL parsing and role matching (`src/review.rs`)

`parse_review_url` accepts only HTTPS URLs of two shapes (query/fragment ignored):

- GitLab: `https://HOST/GROUP[/SUBGROUP...]/PROJECT/-/merge_requests/N`
  (path lowercased);
- GitHub: `https://HOST/OWNER/REPO/pull/N` (case kept).

Hosts may carry a port but no credentials; segments must be
`[A-Za-z0-9._-]`, not `.`/`..`, without a `.git` suffix; `N` must be a
positive integer. `ReviewUrl::canonical()` gives the normalized URL stored as
`review_url`.

`match_role` reduces every role's manifest `remote` with `parse_remote`
(HTTPS/HTTP/`ssh://`/`git://` and SCP-style `git@host:path` all become host +
path, minus `.git`) and compares it with the URL (identical host, path
case-insensitive). **Exactly one** role must match; zero or several is an error.

## Reading the review

`fetch_review` shells out to the provider CLI — `glab api` for GitLab (the
project, then the merge request), `gh api` for GitHub (the pull request) —
with `--hostname HOST --method GET`. hub stores no tokens; the CLI must be
installed and logged in, and a missing CLI produces a usage error naming
`<cli> auth login`.

Responses are validated strictly (`parse_gitlab`, `parse_github`):

- the response must be for the requested project/repo and number, and its own
  web URL must reduce to the same review (a redirect elsewhere is refused);
- the review must be open (GitLab `opened`; GitHub `open` and not merged);
- **same-repository only**: GitLab source and target project ids must equal
  the project's id; GitHub head and base repo ids must match, and a deleted
  head repo is refused;
- branch names must be valid and the head must be a 40-hex commit id.

The result is `ReviewInfo { url, source_branch, target_branch, head_sha }`.

## Sequence (`start_review`)

1. Parse the URL and match the role (no network yet).
2. **No drift**: run base status, and the invoking feature's status when run
   from a feature worktree; any structural drift returns `HubError::Drift`
   (exit 30) before any provider call. See [Status, list, and pull](status-and-pull.md).
3. Fetch the review.
4. Name defaults to `<role>-review-<number>`; `start::preflight` validates it
   and the checkout, and refuses an existing record (finished ones included)
   or hub worktree path.
5. `check_hub`: fetch the hub's origin (failure is an error here, since current
   refs are required) and refuse if the hub branch exists locally or on origin,
   or the tmux session exists. `--name` / `--checkout` resolve these.
6. `check_member` pins a plan for the role's clone:
   - the clone exists and its origin equals the manifest remote; fetch origin;
   - `origin/<source>` exists and equals the reported head (else "stale or moved, rerun");
   - `origin/<target>` exists and shares history with the head (`base_sha` = merge base);
   - the source branch is **not checked out anywhere**, including the main clone;
   - a local source branch is reused if at the head, fast-forwarded if strictly
     behind, and refused if ahead or diverged ("never resets a branch");
   - the worktree path `<worktree_dir>/<clone>/<flattened source>` must not exist.
7. **Fetch the review again**; if source, target, state, or head changed,
   refuse and ask for a rerun.
8. **Create** (transaction): hub branch from `main` (recorded as a conditional
   branch at main's tip) and hub worktree; then the member branch — create a
   tracking branch (verifying the created tip equals the head), or fast-forward
   with `update-ref` conditional on the old tip, or reuse — and the member
   worktree, copying ignored local config files; verify the worktree is on the
   source at the head; write a record with one change: `stage: review`,
   `review_url` = canonical URL, `origin_seen: true`, `base` = target,
   `base_sha` from step 6, `owner: hub`.
9. After the write: workspace, tmux session, and skill install, as warnings on
   failure (see [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)).

## Rollback

On any failure up to the record write, `Created::rollback` removes the created
worktrees, restores a fast-forwarded source branch only if it still points at
the head and is not checked out, and deletes created branches only if they
still point at the tip recorded at creation and are not checked out (see
[Starting features and adding roles](feature-start-and-add.md#rollback-resolvecreated)).
A branch another process moved meanwhile is left alone and reported.

## Afterwards

The feature behaves like any other: `hub status` shows the role in `review`
with its `review_url` and base lag; `hub feature finish` cleans up without
provider access, so it works after the review closes or its branch is deleted.
Finishing records nothing about the review outcome.

## Tests

`tests/start_review.rs` uses `Fixture::make_provider_repo` (provider remote
rewritten to a local bare repo) and the fake `glab`/`gh`, including
`provider_response_next` to simulate a review changing between the two reads
(see [Testing and the integration harness](../testing/integration-test-harness.md));
`review.rs` unit-tests URL/remote parsing and response validation.

## Related pages

- [Starting features and adding roles](feature-start-and-add.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
