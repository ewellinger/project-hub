# Changelog

All notable changes to `hub` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Each release is tagged `v<version>`.
Unreleased changes are fragments under `changelog.d/`, compiled into this
file by [towncrier](https://towncrier.readthedocs.io/) when a release is cut.

<!-- towncrier release notes start -->

## [0.28.1] - 2026-11-09

### Changed

- The dashboard dims the name of a role whose review is marked viewed and has not moved since, in the features pane and the role table.


## [0.28.0] - 2026-11-09

### Added

- A role's `setup` script in `hub.json` (`hub repo add/set --setup`) runs in every worktree hub creates for it after `feature start`, `feature add` and `start-review`, with its output logged under `.git/hub/setup/` and its result shown in the CLI and the dashboard; `--no-setup` skips it and `hub feature setup [ROLE]` reruns it.
- The bundled `hub` skill gains a `setup-scripts.md` reference, written next to `SKILL.md` in every hub worktree, on writing, registering, and testing a role's setup script.
- `hub feature dropped ROLE` records an abandoned change (worktree removed, branch kept); `hub feature finish` refuses while a role is still working or in review unless `--force`; `hub feature remove` deletes dropped branches and prints each deleted tip; the `closed on … without merging` hint names the command; TUI key `d`.
- `hub feature viewed ROLE` (TUI `V`) records a review's head commit and comment count; `hub feature list`, the dashboard and `hub status` then flag the review as `pushed` or `N replies` when the other side moves.
- `hub status`, `hub feature list` and the dashboard show the state of each role's merge or pull request (status, approval, pipeline, unresolved threads, conflicts), read through `glab`/`gh`; `R` in the dashboard and `hub feature list --refresh` read it for every open feature.

## [0.27.0] - 2026-11-01

### Added

- Feature, checkout and hub names may contain `.` (so `app-release-v1.0.1` works), except `..`, a trailing `.`, or a `.lock` suffix, which git refuses in a branch. tmux cannot hold a `.` in a session name, so the session is named with `_` in its place, and `v1.0.1` and `v1_0_1` share one.

### Changed

- The bundled hub skill tells agents to use a member repo's real name (from the role's `remote`) in that repo's own docs, commit messages, and PR or MR descriptions, and to use the hub's role name only in documents that live in the hub repo.
- `config.toml` is read and written by `toml_edit` alone; the `toml` crate is no longer a dependency.
- `hub feature remove` fetches each member clone once, not once per change.

### Fixed

- `hub feature add --from` passes its value to git as a revision, never as an option: `--from=-m` used to rename the clone's base branch.
- `hub feature merged` asks about an adopted worktree before re-pointing its tmux window, so answering no leaves the window and its shell alone.
- `hub feature review` records the stage and URL when the fetch fails (offline, moved remote), printing a warning like every other command instead of aborting.

## [0.26.0] - 2026-10-27

### Added
- `hub status` measures a feature's hub branch against the main clone's `main`: the hub row carries `base: "main"`, `base_behind` (shown as `behind base main by <n>`), and a new `base_ahead`, the commits not yet merged back, shown as `docs not on main: <n>`. `base_ahead` is `null` on every other row. `hub feature finish` prints `note: hub branch <branch> has <n> commits not on main; they stay on the branch` when that count is nonzero; it still deletes and merges nothing.
- Agent skill: a **Merging feature documents into main** workflow (explicit request or a one-time offer before `finish`; a merge commit in the hub's main clone; no push, no state change; works for finished features by branch name) and the reverse "update the hub branch from `main`". The README and the `hub init` instructions template point at it, and the README recommends one folder per feature with shared documents changed on `main`.

### Changed
- `hub init` writes `AGENTS.md` instead of `CLAUDE.md`, and writes neither when the directory already has either file. Existing hubs are untouched; rename `CLAUDE.md` to `AGENTS.md` by hand if you want the generic name.

## [0.25.0] - 2026-10-13

### Added
- `hub feature remove NAME` frees a finished feature's name for reuse, for example for a recurring task. It deletes the feature's local member branches, its hub branch, and its record, so the next `hub feature start NAME` creates fresh branches instead of adopting the old ones. Each member branch must be marked merged (squash merges count) or contained in `origin/<base>`, and gone from origin; the hub branch must have nothing beyond `main`. Any refusal lists every refused branch and deletes nothing. Remote branches are never touched, and the record cannot be restored.
- Dashboard: `x` on a finished feature's row removes it after a confirmation listing the branches it deletes.

## [0.24.0] - 2026-10-12

### Added
- `hub tmux` from the main hub creates or repairs a session for the base checkouts instead of erroring: `hub` and `ai` windows in the hub's main clone, then one window per registered repo's main clone in manifest order, with `hub` started in the `hub` window. It is named by `checkout_template` with `main` as the feature (e.g. `dm-main`); `feature start` refuses that checkout name.
- Dashboard: the feature list colours each role's stage: `working` green, `review` yellow, `frozen` grey.

## [0.23.0] - 2026-10-12

### Added
- `hub feature freeze ROLE [--at REF]` pins a role outside the feature to a commit of its local base in a detached reference worktree that joins the feature's workspace and tmux session; `unfreeze` releases it and `refreeze` moves it to the base's current local tip. `hub status` shows frozen roles with their pin and how far the base has moved since (`frozen_sha`, `base_moved` in `--json`).
- `hub feature start --freeze ROLE` (repeatable) freezes roles at their local base tip as part of the start; a role given to both `--repo` and `--freeze` is refused. In a terminal, the start prompt offers to freeze the roles you didn't choose. Frozen worktrees are rolled back with the rest if the start fails.
- `hub feature freeze` also pins an open change at its branch's merge base with the local base; `status` measures `base_behind` against the pin, shows it as `behind pin <sha> by <n>`, and reports `frozen_sha` and `base_moved` on the change row, `refreeze` reports the new lag, and `merged` drops the pin. `hub feature add` on a frozen role replaces its reference worktree with the change and carries the pin onto it: a new branch starts at the pin unless `--from-latest` (or `--from REF`), and an adopted branch keeps a pin at its merge base; in a terminal it asks which start a new branch takes, also with `--branch`. `--from` with an existing branch is refused on a frozen role. `set-base` refuses a pinned change until it is unfrozen.
- Dashboard: the feature view lists frozen roles and pinned changes with `frozen @<sha>` and how far the local base moved since the pin. `z` freezes any role not yet frozen, including one not in the feature; `u` unfreezes the role under the cursor after a preview of the reference worktree it removes (`F` forces a dirty or off-pin one); `Z` refreezes it at the base's current local tip. The footer shows `u` and `Z` with the cursor on a frozen row. Adding a new branch for a frozen role asks whether it starts from the frozen commit or from `origin/<base>`; an existing branch keeps the pin. The finish preview lists frozen worktrees and warns about dirty or off-pin ones. The feature list shows frozen roles too, in dim italics as read-only references.
- `hub feature list` shows frozen roles as `ROLE:frozen`, and `--json` adds a `frozen` array (`role`, `sha`) to each feature, kept apart from the member `roles`.

## [0.22.1] - 2026-10-08

### Changed
- `cargo install project-hub` no longer needs `--locked`. Overlay boxes are now sized with stable ratatui API instead of the unstable `Paragraph::line_count`, so ratatui is an ordinary `0.30.2` dependency rather than an exact pin. `--locked` still gives the exact versions a release was tested with.
- The README explains why a feature allows only one open change per role.

## [0.22.0] - 2026-10-07

### Changed
- The branch step of `hub feature start`/`add` and of the dashboard's new-feature and add-role flows keeps "type another name" visible while you filter. Typing a new branch name and choosing it opens the name prompt prefilled with what you typed, so a new name no longer needs a restart.

## [0.21.2] - 2026-10-07

### Changed
- The README adds a "Why" section, the workflow `hub` assumes, what a hub contains, and a note on AI assistance. Republished so the crates.io page matches. No behavior changes.

## [0.21.1] - 2026-10-06

First release published to crates.io.

### Changed
- Source comments no longer reference the private issue tracker, and one test fixture path is renamed. No behavior changes.
- GitHub Actions in the CI and release workflows are pinned to commit SHAs.

## [0.21.0] - 2026-10-05

### Added
- `hub config` shows each config key with its effective value and source (variable, file, or default), with `--json`; `hub config set KEY VALUE` and `hub config unset KEY` edit `config.toml`, validating the value and keeping comments. They run outside a hub, so a fresh install can configure itself.

### Changed
- The README is reorganized for first-time readers: a quick start near the top, the reference prose grouped under "How it works", and upgrade notes for hubs created by older versions removed (they remain in this changelog).
- The "not set" error names `hub config set`.

## [0.20.0] - 2026-10-04

First release from the public repository; its git history starts at this version.

### Changed
- README examples and test fixtures use neutral `acme` names.

### Removed
- The internal design specs and plans under `docs/`.

## [0.19.0] - 2026-10-04

### Added
- `hub repo set` changes a role's clone, base, branch template, or description in place.
- `hub repo rename` renames a role in `hub.json` and in every local feature record, renaming open features' tmux windows and regenerating their workspace files; `--records-only [--clone C]` catches another machine up or repairs records left by a remove and re-add.
- `tmux` key in `config.toml`; tmux is optional and sessions are skipped when it is off or not installed.
- A new feature session starts `hub` in its `hub` window.

### Changed
- The dashboard right-aligns each role's stage in the features pane.
- `config.toml` rejects unknown keys, so a `tmux` key makes `hub` older than 0.19.0 refuse to start; upgrade every machine that shares the config.

### Fixed
- `hub feature finish` and `start-review` no longer fail when tmux is not installed.

## [0.18.0] - 2026-09-22

### Added
- Prebuilt binaries for macOS and Linux on every GitHub release, with a shell installer.
- MIT license file, CI on Linux and macOS, `CHANGELOG.md`, `CONTRIBUTING.md`.
- Crate metadata for crates.io (`cargo install --locked project-hub` once published).

### Changed
- Edition 2024, MSRV 1.88.
- The bundled skill names the crate as the install source.

## [0.17.0] - 2026-09-21

### Added
- `~/.config/hub/config.toml` supplies `project_home`, `worktree_dir`, and the editor; the environment variables remain as overrides.

### Changed
- Copy actions use `arboard` instead of piping to `pbcopy`, so the dashboard works on Linux.
- Feature records are fsynced before the rename that replaces them.

## [0.16.0] - 2026-09-20

### Added
- `hub feature start-review URL` creates an isolated review feature from a GitLab MR or GitHub PR.

## [0.15.0] - 2026-09-18

### Added
- `hub pull` fast-forwards base checkouts or a feature's open changes to their copies on origin.
- Bare `hub` inside a feature worktree opens that feature's view directly.

## [0.14.0] - 2026-09-17

### Changed
- Hubs may have a remote: the pre-push hook and local-only rule are gone; `hub.json` and feature-branch documents are shared through git.

## [0.13.0] - 2026-09-17

### Changed
- Feature records moved out of git into `.git/hub/features/` in the hub's main clone. Hubs created before 0.13.0 are refused with the three commands that move the files.

## [0.12.0] - 2026-09-16

### Changed
- Feature view polish in the dashboard: column priorities, merged roles sorted last, copy action for branch, path, or review URL.

## [0.11.0] - 2026-09-15

### Added
- Feature workflows in the dashboard: create, add roles to, review, and finish features without leaving the TUI.

## [0.10.1] - 2026-09-15

### Fixed
- The dashboard formats the "refreshed … ago" timer by magnitude instead of raw seconds.

## [0.10.0] - 2026-09-13

### Changed
- Tables use the terminal width; `hub status` shows a `BASE` column when a change merges into a non-default base.

## [0.9.2] - 2026-09-13

### Added
- The bundled hub skill updates feature branches from their bases on request.

## [0.9.1] - 2026-09-13

### Changed
- `hub status` prints a three-line header with emphasised names.

## [0.9.0] - 2026-09-13

### Changed
- `hub status` lists merged roles and finished features under `Completed` and shortens home paths to `~`.

## [0.8.0] - 2026-09-13

### Changed
- `hub feature list` hides finished features unless `--all` is passed.

## [0.7.0] - 2026-09-11

### Added
- Per-change base tracking: `feature add --base`, `feature set-base`, and `status` lag against the recorded base.

## [0.3.0] and earlier - 2026-09-08 to 2026-09-11

Initial Rust rewrite of the hub CLI: `init`, `repo`, `feature start/add/review/merged/finish/list`, `status`, `tmux`, `open`, `sync`, the dashboard, and the bundled skill.

