# Changelog

All notable changes to `hub` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Each release is tagged `v<version>`.

## [Unreleased]

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

