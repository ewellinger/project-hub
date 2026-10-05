---
name: hub
description: Use when working inside a directory tree that contains hub.json, or when asked to start, extend, review, set up a review from a merge request or pull request URL, update from its bases, merge, finish, or inspect a feature that spans several git repositories coordinated by a hub repo.
---

# Hub Multi-Repo Coordinator

A hub is a git repo, shared through a remote when the project has one, whose `hub.json` lists the member repos of one project by **role**. Each feature gets a hub branch and worktree, a branch and worktree in every member repo it touches, a tmux session, and a VS Code workspace, all recorded on this machine under the hub's `.git/hub/`, never committed; `hub.json` and the documents on feature branches are the only hub state git tracks. The `hub` binary on `PATH` (the `project-hub` crate, installed with `cargo install --locked project-hub` or from a GitHub release; configured by `~/.config/hub/config.toml`, see the README's Install section) performs every change to feature state: branches, worktrees, sessions, workspaces, and the feature record. Do not recreate those steps by hand. Ordinary git work inside a member worktree, such as committing or merging its base in when the user asks, is not a hub operation.

## Rules

1. Run `hub status` before acting on any path found in a document or a message. The feature file is authoritative; prose is not. Exit code 30 means structural drift (a missing or unregistered checkout, or a wrong branch); report the affected rows and stop. Dirtiness is reported independently and is handled according to the operation under **Dirty worktrees** below.
2. In specs, plans, and handoffs, refer to member repos by role, never by filesystem path. Paths derive from each machine's `project_home` and `worktree_dir` configuration and change between machines and features.
3. A hub may have a remote. Push a hub branch only when the user asks. `hub.json` on `main` is pulled by the user, never by you; `hub status` shows the hub row behind its origin copy when a pull is due.

## Commands

| Intent | Command |
|---|---|
| Turn the current directory into a hub; `--force` refreshes an existing one | `hub init [--name N] [--checkout-template T] [--force]` |
| Register `<project_home>/CLONE` under a role | `hub repo add ROLE CLONE [--base B] [--branch-template T] [--description D]` |
| Change a role's clone, base, branch template, or description in place | `hub repo set ROLE [--clone C] [--base B] [--branch-template T] [--description D]` |
| Rename a role in `hub.json` and this machine's records; `--records-only` catches up records after pulling a rename | `hub repo rename OLD NEW [--records-only [--clone C]]` |
| Unregister a role (clone untouched) | `hub repo remove ROLE` |
| Show the member repos | `hub repo list [--json]` |
| Start a feature: hub branch and worktree, one change per `--repo`, tmux session | `hub feature start NAME [--checkout C] [--repo ROLE[:BRANCH]]...` |
| Set up an isolated review of a GitLab MR or GitHub PR: its role from the URL, the real source branch at the verified head, base = the MR's target, stage `review` | `hub feature start-review URL [--name NAME] [--checkout C]` |
| Bring one more role into the feature | `hub feature add ROLE [--branch B] [--from REF] [--base B] [--worktree W]` (the role is required without a terminal) |
| Record the branch a role's open change merges into, when it is not the role's base | `hub feature set-base ROLE BRANCH` |
| Record that a role's change is in review | `hub feature review ROLE [URL]` |
| Close a role's change: remove its worktree, keep the branch | `hub feature merged ROLE [--force]` |
| Remove every worktree and the session; keep every branch | `hub feature finish [--force]` |
| Open features with the stage of each role; `--all` includes finished ones | `hub feature list [--all] [--json]` |
| Compare feature state, or all base checkouts from the main hub, with reality; `--base` forces base status from a feature worktree; merged roles list under `Completed`; exit 30 on structural drift | `hub status [--base] [--json]` |
| Create or repair the tmux session (never attaches) | `hub tmux` |
| Open the VS Code workspace (all base clones from the main hub) | `hub open` |
| Regenerate the workspace file | `hub sync` |

Commands that act on a feature infer it from the hub worktree you are in. Anywhere else, pass `--feature NAME`. `hub <command> --help` explains each flag. `hub` never prompts when stdin is not a terminal, so pass every role and branch explicitly; `hub feature add` needs its role, and `hub feature start` without `--repo` is a docs-only start. Bare `hub` opens an interactive dashboard only in a terminal; the skill always passes a subcommand.

`hub open` and `hub status` are the exceptions. From the main hub without `--feature`, `open` opens a workspace containing the hub and every registered repo's main clone in manifest order, while `status` reports those base checkouts.

## Reading output

After any command that changes state, run `hub status --json` (with `--feature NAME` when outside the hub worktree). Take each worktree path from `rows[].path` and the attach command from `checkout`: `tmux attach -t <checkout>`. The checkout name can differ from the feature name; never derive it, or a path, from a branch name. The human table shortens paths under the home directory to `~/…` and moves merged roles (and every row of a finished feature) to a `Completed` table without state or path; the JSON `rows` array is unchanged, so always take paths from it. `hub feature list --json` gives every open feature with its checkout and the stage of each role; pass `--all` to include finished features. Each open row also carries `base`, the branch that change merges into, and `base_behind`, the number of commits on `origin/<base>` the branch lacks (`null` when the base ref is unavailable). The human table shows a nonzero count as `behind base <branch> by <n>` and adds a `BASE` column only when some open change merges into a branch other than its role's base; `custom_base` marks those rows in the JSON. Every `hub status --json` row also carries `review_url`, the merge or pull request recorded on that change (`null` when none); `hub feature list --json` does not report it.

## Workflow

- **Starting a feature:** ask which roles participate if the request does not say; zero roles is a valid docs-only start. Run `hub feature start`, then `hub status --json`, and report the hub branch, each role's branch and worktree path, and the attach command. Stop there: do not attach and do not launch anything in the `ai` window.
- **Setting up a review from a URL:** a request such as "set up a feature for reviewing https://gitlab.com/group/project/-/merge_requests/486" inside a hub means `hub feature start-review URL`. Follow **Reviewing a merge request or pull request** below.
- **Extending, reviewing, merging:** `feature add` opens one more role's change; `feature review` records the MR or PR URL; `feature merged` removes that role's worktree and keeps its branch. A later `feature add` for the same role gets a follow-up branch automatically.
- **A change that merges into a branch other than the role's base:** pass `--base BRANCH` to `hub feature add`, or run `hub feature set-base ROLE BRANCH` on an open change. Never use `git branch --set-upstream-to` for this; the hub ignores git's upstream. When `hub status` shows `behind base <branch> by <n>`, report it; update the branch only when the user asks, following **Updating from the base** below.
- **Opening a PR or MR for a role:** take the row's `base` from `hub status --json` and use it as the target branch, whether it is the role's default or a per-change base set with `--base` or `set-base`. Use another target only when the user names one explicitly. Record the URL with `hub feature review ROLE URL`.
- **Inspecting an unbound role:** run `hub status --base --json`, take the registered role's base-checkout path from its row, and treat that checkout as read-only. Run `hub feature add ROLE` only after the investigation proves that role needs edits.
- **Exit code 20 from `merged` or `finish`:** show the dirty rows and ask whether to force-remove those exact worktrees. Only after an explicit yes, rerun with `--force`. Neither command deletes a branch anywhere; do not add branch deletion.
- **Exit code 21 from `merged` or `finish`:** a process still has its working directory inside a worktree about to be removed; the error lists pid, command, and directory. Report them and ask whether to stop those processes or force removal. Only after an explicit yes, either stop the exact processes named and rerun, or rerun with `--force`. Never kill processes the user did not name.
- **`finish` refuses because it is running inside the feature's own tmux session:** killing that session would also kill the command. Do not retry from this window; tell the user to run `hub feature finish --feature NAME` from a terminal outside the session.
- **`tmux is off; no session created`, or `hub tmux` refusing with `tmux is off`:** sessions are disabled by `tmux = false` in the config or tmux is not installed. This is not an error; report the worktree paths from `hub status --json` instead of an attach command, and do not install or start tmux.
- **Adopted worktrees:** `merged` and `finish` ask before removing a worktree the hub did not create, and stop without `--force` when they cannot ask. Name the worktree path and get the user's yes before passing `--force`.
- **A `warning:` line:** the feature record was written but a workspace or tmux step failed; the line names the command that retries it (`hub sync` or `hub tmux`). Run that command once. If it fails again, report the error and stop. The `warning: tmux = true … but tmux is not on PATH; skipping sessions` line is informational; do not run `hub tmux` for it.
- **A `warning:` line saying a worktree's directory remains:** git unregistered the worktree but could not delete the directory, and the role or feature is already recorded as closed. Report the path and git's reason; do not delete the directory unless the user asks.
- **A `warning:` naming a fetch, `origin/<base>`, or the hub skill:** a failed fetch or a missing `origin/<base>` means that clone's counts are stale and `base_behind` may be `null`; fix the fetch (network, credentials, or a sandbox that cannot write to the clone) and rerun `status`. `hub sync` and `hub tmux` do not help. A warning that the hub skill could not be refreshed or excluded concerns only the generated skill copies; report it and nothing else is affected.
- **Any other failure:** report the error verbatim and stop. Do not improvise git or tmux commands to work around it. A merge conflict during a requested base update is not a hub failure; handle it as described under **Updating from the base**.
- **Finished features** are history: only `feature list --all` and `status` accept them. Without `--all`, `feature list` hides them, so a feature missing from the plain listing may be finished rather than absent.

## Reviewing a merge request or pull request

The URL names the role: never ask which roles take part or for a repository prefix, and ask for the URL only when it cannot be inferred. Same-repository GitLab MRs and GitHub PRs are supported through an authenticated `glab` or `gh` for that host; forks, closed or merged reviews, and deleted source branches are refused before anything is created.

1. Run `hub status --json` from the main hub (or the feature you are in) and stop on exit 30.
2. Run `hub feature start-review URL`. Pass `--name` only when the user names the feature or the default `<role>-review-<number>` collides. The command creates the hub branch and worktree, checks out the review's actual source branch at the head the provider reports into a hub-owned worktree, records the MR's target branch as the change's base, marks the change `review` with the canonical URL, and creates the workspace and a detached tmux session.
3. Verify with `hub status --json --feature NAME`: report the role, `branch`, `base`, `review_url`, the worktree `path`, the checked head (the command printed it), any `warnings` about fetches or lag, and `tmux attach -t <checkout>`. Do not attach and do not launch an agent.
4. Refusals: the source branch checked out in the main clone or another worktree, a local copy ahead of or diverged from origin, an existing feature record (finished ones included), an existing hub branch or session, a directory already at the member worktree path the review would use, a missing `origin/<target>`, a source and target that share no history, or a review that changed while checking. `--name` and `--checkout` help only with the feature, branch, and session collisions; every other refusal names state to resolve first. Report the message; do not construct a detached-head checkout, a manual `git worktree add`, or a tmux session by hand, and do not delete or adopt existing state to make the command pass.
5. To look at other roles while reviewing, run `hub status --base --json` and treat those base checkouts as read-only: report their branch, commit, and dirty or stale state. The review feature's workspace stays limited to the reviewed role; add a role with `hub feature add ROLE` only when the user asks for edits there.
6. Finish with `hub feature finish --feature NAME` from outside that feature's tmux session. The usual rules apply: dirty, in-use, or adopted worktrees need an explicit yes before `--force`, and every branch is kept. Finishing records nothing about the review's outcome; it is not evidence of a merge. To find the feature for a URL, resolve the role from the manifest and look for a matching `review_url` in `hub status --json --feature NAME` for each feature `hub feature list --json --all` shows; never guess a custom name, and ask when more than one feature could be meant. Cleanup needs no provider access, so it works after the review is closed or its branch is deleted.

## Dirty worktrees

A dirty row on the expected branch and registered path is normal active development, not structural drift. Keep it visible and protect it according to what the user asked you to do:

- **Read-only inspection or reporting:** continue. Do not require the user to commit, stash, or discard local changes.
- **Implementation:** inspect the changed-file list before editing. Preserve pre-existing changes, continue with non-overlapping work, and stop for direction if the requested edit overlaps changes whose ownership is unclear.
- **Known task output:** if a row was clean at the initial preflight and became dirty only after a user-authorized edit in the current task, treat that edit as known task output. Newly changed files outside that known diff are user- or concurrently-owned until established otherwise.
- **Local-only configuration:** an intentionally uncommitted runtime override is legitimate dirty state. Do not commit it, hide it with Git index flags, or treat it as drift unless the user asks.
- **Updating from the base:** the target worktree must be clean before the merge starts, which is stricter than git's own check. Stop and ask rather than stashing or committing for the user. A dirty hub worktree or a dirty unrelated role does not block a clean target.
- **Removal or cleanup:** never remove a dirty worktree implicitly. `feature merged` and `feature finish` enforce this with exit code 20; follow the explicit-force workflow above.

## Updating from the base

Only an explicit request such as "update the feature branches from their bases" authorizes this. Unsolicited `behind base` lag is reported, never merged. The request authorizes local merges in the selected member worktrees and nothing else: no push, no change of stage or base, no `feature merged` or `feature finish`.

1. Run `hub status --json` (with `--feature NAME` outside the hub worktree) and select the open member rows, or only the roles the user named. Skip the hub row, `merged` rows, and roles not in the feature, and say so when the user named one. A row with `base_behind` 0 is already current. A `null` count, or a `warnings` entry about that clone's fetch, means the count is stale: fix the fetch and rerun `status` before touching the role. Structural drift stops the update as it stops everything else.
2. Work in manifest order unless the user or the repositories' own instructions give another order, and finish one role before starting the next.
3. In the row's `path`, require a clean worktree on the row's `branch` with no merge or rebase in progress. Otherwise stop and ask; never stash, commit, discard, or clean on your own.
4. Record the current HEAD and the target commit, `git rev-parse origin/<base>` using the row's `base`, never git's upstream or the role's default. Merge that commit, fast-forwarding when possible:

   ```bash
   git merge --ff --no-edit -m "Merge origin/<base> into <branch>" <sha>
   ```

   Never use bare `git pull`, `reset --hard`, `--allow-unrelated-histories`, or a blanket ours/theirs resolution. Resolve an ordinary conflict by keeping both sides' intent, stage only the files you resolved, and complete the merge. If the right resolution is unclear, leave the merge open and stop with the list of conflicted files. Rebase only when the user asks for it.
5. Run the repository's own checks. Then confirm `git merge-base --is-ancestor <sha> HEAD` succeeds and rerun `hub status --json`. The update is complete when every selected row shows `base_behind: 0` with no fetch warning; if the base moved meanwhile, merge the new commit the same way. Report per role: base, lag and HEAD before and after, and the check results. A larger `ahead` is expected after a merge. Commits on the branch's own origin copy that are missing locally are reported, not pulled.
