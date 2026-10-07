# project-hub

[![CI](https://github.com/ewellinger/project-hub/actions/workflows/ci.yml/badge.svg)](https://github.com/ewellinger/project-hub/actions/workflows/ci.yml)

`hub` makes a lightweight "glue" repository for working across several
repositories at once, carrying the guidance coding agents need to see how
those repositories fit together. It leans on git worktrees so that several
features can be in progress at any given time, and so that an agent working
on one of them references the right state of every repository involved.

A hub is a git repo whose `hub.json` lists the member repos by role; give it a
remote and teammates share it. Each feature gets a hub worktree for its
documents, a branch and worktree in every repo it touches, a tmux session,
and a VS Code workspace. `hub status` compares all of it with what is
actually on disk, and `hub feature finish` takes it down again, keeping every
branch. Those bindings are recorded on this machine under the hub's
`.git/hub/features/<name>.json` and are never committed; `hub.json` and the
documents on feature branches are the only hub state git tracks.

## Why?

This project was inspired by work on a project that spanned several
distinct repositories, which sometimes needed coordinated changes across
them.

For example, suppose a project has these components:

- A UI frontend, where users actually interact with the application.
- An API the frontend calls, which coordinates between the application
  database and a separate AI API. For brevity this is called the
  "Backend-for-Frontend", or BFF.
- The AI/agent/ML API.

Running Claude or Codex inside one of those repos made it over-index on that
component, unless it was explicitly told to look at the larger picture. There
was also the overhead of keeping track of many git worktrees, and of making
sure agents _actually_ referenced the right state of each codebase.

This project was also an excuse to get more experience with Rust and with
building a TUI in [Ratatui](https://ratatui.rs).

## The workflow `hub` assumes

`hub` is built around one way of working, and fits best when yours is close
to it:

- Work is organized as features. A feature touches one or more member
  repos, and several features can be open at once.
- Each repo a feature touches gets its own branch and its own worktree under
  `worktree_dir`. The main clones stay on their base branches.
- The hub gets a worktree per feature too, on a hub branch that holds that
  feature's documents: plans, specs, notes. You and your coding agent work
  from there, so the agent sees the hub's guidance and the [agent
  skill](#agent-skill), and `hub status` points it at every member worktree
  for the feature.
- Each repo's change is reviewed and merged on its own, through a GitLab
  merge request or GitHub pull request; `hub feature review` and
  `hub feature merged` record that progress one repo at a time.
- When the feature is done, `hub feature finish` removes its worktrees and
  session and keeps every branch.
- Optionally, each feature gets a tmux session, and an editor workspace
  opens all of its worktrees together.

If your workflow differs a lot, for example you work in one repo at a time,
keep all your work on one long-lived branch, or don't use worktrees, `hub`
will likely get in your way more than it helps.

## What goes in a hub

The hub's `CLAUDE.md` and its feature documents are where an agent learns
how the member repos fit together. Keep them to what no single repo can say.

Do put in the hub:

- How the member repos relate: which calls which, the contracts between
  them, and the data that flows across them.
- Ordering: dependencies between changes, which repo merges first, and the
  order to deploy in.
- Per-feature plans and designs that span more than one repo.

Don't put in the hub:

- Guidance or context that already lives in a member repo, such as its own
  `CLAUDE.md`, `AGENTS.md`, or README. The agent reads those when it works in
  that repo, and a copy in the hub drifts out of date.

## Install

From crates.io, with a Rust toolchain of 1.88 or newer:

```bash
cargo install --locked project-hub
```

`--locked` matters: `hub` uses a ratatui feature that has no semver
guarantee, and the lock file pins the version it was tested with.

Prebuilt binaries for macOS (Apple silicon and Intel) and Linux (x86_64)
are attached to every [GitHub release](https://github.com/ewellinger/project-hub/releases).
The installer script puts `hub` in `~/.cargo/bin`:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/ewellinger/project-hub/releases/latest/download/project-hub-installer.sh | sh
```

From a clone: `cargo install --locked --path .`.

### Requirements

- `git` 2.31 or newer on `PATH`.
- `tmux`, optional: see [Without tmux](#without-tmux).
- An editor launcher for `hub open` and the dashboard's `o`: `code` (VS Code)
  unless the config file names another.
- `glab` or `gh`, authenticated for the host, for `hub feature start-review`.

### Configuration

`hub` needs to know where main clones and worktrees live. Set both once:

```bash
hub config set project_home ~/workspace
hub config set worktree_dir ~/workspace/worktrees
```

This writes `~/.config/hub/config.toml` (`$XDG_CONFIG_HOME/hub/config.toml`
when that variable is set; the path is the same on macOS and Linux), which
you can also edit by hand:

```toml
project_home = "~/workspace"           # main clones live at <project_home>/<clone>
worktree_dir = "~/workspace/worktrees" # worktrees live at <worktree_dir>/<clone>/<name>
editor = "code"                        # optional: the command `hub open` runs
tmux = false                           # optional: skip tmux sessions (default: on when tmux is on PATH)
```

Both directories must already exist, and a leading `~` in the file is your
home directory. `editor` is split on whitespace and run without a shell, so
arguments work (`"code --new-window"`) but quoting and `$VAR` do not. The
environment variables `PROJECT_HOME` and `GIT_WORKTREE_DIR`, when set and
non-empty, override the file's two paths. With neither source, every command
exits with the file's path and the example above.

`hub config` lists every key with its effective value and where it comes
from (the variable, the file, or the default), and `--json` gives the same
for scripts. `hub config set KEY VALUE` checks the value the way every
command does (directories must exist, `tmux` is `true` or `false`) before
writing, keeps comments and other keys, and refuses unknown keys; `hub
config unset KEY` removes one.

### Without tmux

With `tmux = false` in `config.toml`, or when `tmux` is not on `PATH`, `hub`
still creates every worktree, workspace file, and feature record but no
session: `feature start`, `start-review`, and `add` print `tmux is off; no
session created`, `finish` and `merged` skip their session steps, `status`
and the dashboard leave out the attach hints, and `hub tmux` refuses. Open
the feature with `hub open` or `cd` into the paths `hub status` lists.
Setting `tmux = true` with no `tmux` installed prints one warning and carries
on without sessions.

With tmux on, a session `hub` creates starts `hub` in its `hub` window, so
the feature view is waiting when you attach; quitting it leaves a shell.

## Quick start

The member repos are already cloned as `acme-api`, `acme-web`, and `acme-bff`
under `~/workspace`; `hub init` makes the new directory a git repo:

```bash
mkdir ~/workspace/acme && cd ~/workspace/acme
hub init --checkout-template 'acme-{feature_snake}'
hub repo add api acme-api --description 'API service'
hub repo add ui acme-web --branch-template 'jdoe/{feature}'
hub feature start suspense --repo ui --repo api:feature/acme-suspense
tmux attach -t acme-suspense
hub repo add bff acme-bff --description 'BFF service'
hub feature add bff --branch feature/acme-suspense
hub feature review api https://gitlab.example/merge_requests/412
hub feature merged api
hub feature finish
```

`--checkout-template` names each feature's hub worktree and tmux session; it
takes `{feature}`, `{feature_snake}` (dashes become underscores), and
`{hub}`. A role's `--branch-template` names the branch `feature start`
creates in that repo, with `{feature}` substituted; it defaults to
`{feature}`, and `ROLE:BRANCH` overrides it for one feature.

## Commands

| Command | What it does |
|---|---|
| `hub init [--name N] [--checkout-template T] [--force]` | Turn the current directory into a hub; `--force` refreshes an existing hub's init-owned files |
| `hub repo add ROLE CLONE [--base B] [--branch-template T] [--description D]` | Register `<project_home>/CLONE` under a role |
| `hub repo set ROLE [--clone C] [--base B] [--branch-template T] [--description D]` | Change a role's fields in place, keeping its position |
| `hub repo rename OLD NEW [--records-only [--clone C]]` | Rename a role in `hub.json` and this machine's feature records |
| `hub repo remove ROLE` / `hub repo list [--json]` | Unregister / show the member repos |
| `hub feature start NAME [--checkout C] [--repo ROLE[:BRANCH]]...` | Hub branch + worktree, one change per `--repo`, tmux session; prompts for roles and branches in a terminal |
| `hub feature start-review URL [--name N] [--checkout C]` | Review feature for the one role whose remote the GitLab MR or GitHub PR URL names: its real source branch at the verified head, base = the MR's target, stage `review`, workspace and session; refuses forks, closed reviews, and occupied branches |
| `hub feature add [ROLE] [--branch B] [--from REF] [--base B] [--worktree W]` | Bring one more role into the feature; prompts for the role and branch in a terminal |
| `hub feature set-base ROLE BRANCH` | Record the branch the role's open change merges into; the member branch and its git upstream are untouched |
| `hub feature review ROLE [URL]` | Record that the role's change is in review |
| `hub feature merged ROLE [--force]` | Close the change: remove its worktree, keep the branch |
| `hub feature finish [--force]` | Remove every worktree and the session; keep every branch |
| `hub feature list [--all] [--json]` | Open features with the stage of each role; `--all` includes finished ones |
| `hub status [--feature N] [--base] [--json]` | Compare feature state, or base checkouts from any hub worktree, with reality; merged roles and finished features list under `Completed`; exit 30 on structural drift |
| `hub pull [--feature N] [--base]` | Fast-forward each role's checkout to its copy on origin: the base checkouts from the main hub, the feature's open changes from a feature worktree; a checkout that cannot simply move forward is reported and left alone |
| `hub config [--json]` / `hub config set KEY VALUE` / `hub config unset KEY` | Show each config key with its value and source / validate and write a key / remove it; works outside a hub |
| `hub tmux` / `hub open` / `hub sync` | Repair the session / open the workspace / regenerate it; from the main hub, `open` uses all base checkouts |

Feature commands infer the feature from the hub worktree you are in, or take
`--feature NAME`. From the main hub without `--feature`, `hub open` opens a
workspace containing every base checkout, and `hub status` reports their
configured branch, path, cleanliness, and ahead/behind state.

Exit codes: `0` ok, `1` usage or precondition error (nothing changed), `2`
bad arguments, `20` dirty worktree, `21` worktree in use, `30` drift.

## How it works

### Bases

Each change records the branch it merges into: the role's base from
`hub.json` unless `feature add --base` or `feature set-base` names another
one, such as a long-lived integration branch. `set-base` changes only the
feature record and re-derives `base_sha` as the merge base of the branch and
the new base; it never checks out, rebases, or edits git's upstream, and
git's upstream never changes the recorded base. Merge hints target the
recorded base.

### Status

`hub status` fetches each repo and compares the feature branch with
`origin/<base>`, showing `behind base <branch> by <n>` in yellow when the
base has commits the branch lacks. A `BASE` column appears only when some
change merges into a branch other than its role's base, so a feature on
default bases keeps the compact table. `--json` carries it as `base`,
`custom_base`, and `base_behind` next to `ahead`/`behind` against the
branch's own origin copy, and reports `review_url` on every row.

Dirtiness and base lag are operational state, reported but never failing
the command. Missing or unregistered checkouts and wrong branches are
structural drift and exit 30. A merged role, or every row of a finished
feature, has no worktree by design, so `status` lists it in a separate
`Completed` table with role, branch, and stage rather than as missing. Paths
under your home directory print as `~/…`; `--json` keeps them absolute.

### Pull and base updates

`hub pull` acts on the `+a/-b` column: from the main hub it fast-forwards
each role's base checkout to `origin/<base>`, and from a feature worktree, or
with `--feature`, each open change to `origin/<branch>`. Repos are fetched
and moved concurrently, and the lines come back in manifest order. A
checkout that is missing, on another branch, dirty, or diverged from origin
(local commits and new commits on origin) is reported and left exactly as it
was; one that is only ahead is reported as up to date. Nothing is merged,
rebased, or stashed, the hub's own clone is not pulled, and the feature
records are untouched.

`hub pull` never brings `origin/<base>` into a feature branch, and `status`
only measures that lag. Updating a feature from its bases is a job for the
[agent skill](#agent-skill): asked to, the agent merges `origin/<base>` into
each open change in turn, runs that repository's checks, and reruns
`status` until every selected role shows zero lag. The merge is local:
nothing is pushed, and the recorded base, stage, and worktree are unchanged.
`ahead` normally grows after such a merge.

### Reviewing a merge or pull request

`hub feature start-review URL` turns a merge or pull request into a feature
of its own. The URL is matched against the `remote` of every registered
role (HTTPS, `ssh://`, and `git@host:path` forms all reduce to host and
path); exactly one must match. The review is read through `glab api` or `gh
api`, so that CLI must be installed and logged in for the host; hub stores no
tokens. The role's source branch is checked out at the head the provider
reports, in a hub-owned worktree, and the change records the MR's target as
its base and the canonical URL as `review_url`. A local copy of the source
branch is reused when it is at that head, fast-forwarded when it is behind,
and refused when it is ahead, diverged, or checked out anywhere, including
the main clone; nothing is ever reset. Existing feature records (finished
ones too), hub branches, and sessions are collisions, not adoptions: pass
`--name` or `--checkout`. Everything created before the record write is
rolled back if a later step fails, with conditional ref updates so a branch
another process moved meanwhile is left alone. Finishing the feature works
as for any other and needs no provider access.

### Existing work and failures

Existing work is adopted, not refused: a branch already on origin, checked
out in the main clone, or checked out in a review worktree under
`<worktree_dir>` is bound as-is. The hub only creates what does not exist,
and rolls back exactly that if anything up to the record write fails. After
the write, a failed workspace or tmux step is printed as a `warning:` with
the command that retries it (`hub sync`, `hub tmux`).

`feature merged` and `feature finish` refuse to remove a dirty worktree
without `--force`, and refuse (exit 21) while a process has its working
directory inside a worktree they are about to remove: a dev server or test
watcher recreating files mid-delete leaves an unregistered directory behind.
Both commands re-point or kill the feature's own tmux windows first, so only
processes started elsewhere trip the check. Should git unregister a worktree
and still fail to delete its directory, the role or feature is recorded as
closed anyway and a `warning:` names the leftover directory to inspect and
delete by hand. Finished features are history: only `feature list --all` and
`status` accept them.

### Hub files

`hub init` keeps an existing `CLAUDE.md` and merges `.gitignore`. After
upgrading `hub`, `hub init --force` in an existing hub brings the
init-owned files up to date: it merges the current ignore lines into
`.gitignore`, removes files they now match from the index (they stay on
disk), and commits the result as `hub: refresh init files`; `hub.json` and
`CLAUDE.md` are never modified. Generated workspace files remain local:
every workspace write adds its exact filename to Git's local exclude file.

Feature records are plain JSON under `.git/hub/features/` in the hub's main
clone; every hub worktree reads the same files, so `hub feature list` and
`hub status` agree everywhere. `hub feature list --json` and `hub status
--json` are the supported way to read them.

### Sharing a hub

Sharing a hub is ordinary git. Push it, and a teammate clones it, writes
their own [`config.toml`](#configuration), and clones each member repo under
their `project_home` with the directory name `hub.json` records as its
`clone`. `hub feature start` then fetches the hub's origin and adopts a hub
branch a teammate already pushed, the same way it adopts member branches
that exist on origin. Nothing pushes or pulls on its own: `repo add`,
`remove`, `set`, and `rename` commit `hub.json` on `main`, and everyone
pulls `main` themselves. `hub status` shows each hub row's ahead/behind
against its origin copy, so a pushed `hub.json` change appears as `behind`
until you pull. `hub repo add` records the remote URL as git has it
configured (`remote.origin.url`), not a `url.<base>.insteadOf` rewrite.

Feature records are per machine, so a `hub repo rename` pulled from the
hub's remote leaves other machines' records on the old name. Run
`hub repo rename OLD NEW --records-only` on each of them. The same flag
with `--clone C` repairs records left by an earlier remove and re-add,
renaming only the changes recorded with clone `C`. NEW must already be
registered in `hub.json`. It refuses when OLD and NEW are the same, or when
the rewrite would leave a feature with two unmerged changes for one role.
When catching up a sequence of renames (a swap, say), run them in the order
they were made; running them out of order mixes histories, which `--clone`
repairs.

## Dashboard

`hub` with no subcommand, in a terminal, opens the dashboard: the base
checkouts and every open feature on the left, the live status of the
selected row on the right. Status runs in the background when you move the
selection or press `r`, showing the last result until the new one lands.
Finished features are hidden until `a` shows them. Run from a feature's hub
worktree, `hub` opens that feature's view directly; `Esc` returns to the
dashboard with that feature selected.

| Key | Action |
|---|---|
| `↑` `↓` `j` `k` | select a row |
| `Enter` | open the selected feature's view |
| `n` | start a new feature: name, roles, a branch per role, summary, then `feature start` |
| `a` | show or hide finished features |
| `r` | reload the feature list and refresh the selected row |
| `o` | open the selected row's workspace in the editor (base workspace on the base row) |
| `p` | `hub pull` for the selected row: the base checkouts, or the feature's open roles; each role's outcome is shown when it finishes |
| `?` | list every key; `?`, `Esc`, or `q` closes the list |
| `q` `Esc` | quit; refused with `busy: <action>…; Ctrl-C quits anyway` while an action runs |
| `Ctrl-C` | quit at once, even mid-action: the worker is killed with the process |

`Enter` is refused on `base` and on a finished feature, with the reason in
the footer; `Ctrl-C` quits from either screen.

The feature view shows one feature's roles with their branch, stage, and
state, plus warnings, the selected role's review URL, and the attach command.
Open roles are listed above merged ones, and the branch column is never
truncated. The footer shows the frequent keys; `?` lists them all.
Its keys call the same commands the CLI has, under the hub lock, and show the
command's output when it finishes; a failure shows the error and changes
nothing.

| Key | Action |
|---|---|
| `↑` `↓` `j` `k` | select a role |
| `c` | copy the role's branch name, worktree path, or review URL to the OS clipboard; only what exists is offered |
| `v` | `feature review` for the role, with an optional URL |
| `m` | `feature merged` for the role, after a preview of the worktree it removes; `F` in the preview forces |
| `b` | `feature set-base` for the role |
| `+` | `feature add`: pick a role, then a branch |
| `f` | `feature finish`, after a preview of every worktree it removes and a warning for roles not yet merged; `F` forces |
| `o` | open the feature's workspace |
| `p` | `hub pull` for the feature: fast-forward every open role |
| `r` | refresh |
| `?` | list every key |
| `Esc` `q` | back to the dashboard |

Pickers offer the same rows the CLI prompts do: the default branch first,
marked `new` when it does not exist, then branches git already knows in that
clone, then a free-text row. `Esc` in any prompt cancels the whole action
with nothing changed. One action runs at a time; the footer shows it.

`finish` cannot run from inside the feature's own tmux session, because
killing the session would kill the dashboard; the preview says so and points
at the CLI command to run from another terminal. The dashboard never attaches
to tmux; the attach command is shown in the status pane. Without a terminal,
bare `hub` reports a missing subcommand on stderr and exits 2.

The manifest is read once at startup; restart the dashboard after `hub repo
add`, `remove`, `set`, or `rename`.

## Interactive use

When stdin and stderr are both a terminal, `feature start` and `feature add`
prompt for whatever you left out: the roles (`start` with no `--repo`, or
`add` with no role) and then a branch per role. The branch prompt lists the
default first (marked `new` when it does not exist yet), then branches git
already knows in that clone, each showing whether it is local, on origin, or
checked out in a worktree the hub can adopt, and finally a free-text row.
Enter on the first row is exactly what the flags would have done. Esc or
Ctrl-C exits 1 with nothing changed; prompts finish before the hub lock is
taken. Without a terminal nothing prompts: `start` with no `--repo` is a
docs-only start and `add` needs its role.

## Agent skill

`skills/hub/SKILL.md` teaches Claude Code and Codex to drive this binary. The
binary embeds it at build time and writes it into every hub worktree as
`.claude/skills/hub/SKILL.md` and `.agents/skills/hub/SKILL.md` on every
command, so each hub always carries the copy that matches the installed
`hub`. The files are generated: `hub init` adds them to the hub's `.gitignore`,
every command also adds them to Git's local exclude file, and a copy that is
missing or edited is rewritten by the next command. A failed write is a
`warning:` and never fails the command. Nothing is installed at user scope.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the toolchain, the checks CI runs,
how the fake `tmux`/`code` in `tests/fake-bin` work, and the release
procedure. Releases are listed in [CHANGELOG.md](CHANGELOG.md).

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## AI disclosure

AI coding agents helped build this project, with the author directing and
reviewing the work.

## License

[MIT](LICENSE).
