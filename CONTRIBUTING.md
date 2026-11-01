# Contributing

## Toolchain and tools

- Rust 1.88 or newer (`rust-version` in `Cargo.toml`); `rustfmt` and `clippy` components.
- `git` 2.31 or newer and `tmux` on `PATH`. `code` (VS Code) for `hub open`, or set `editor` in the config file.
- `glab` or `gh` only for `hub feature start-review`.

## Checks

Every PR must pass the same commands CI runs (`.github/workflows/ci.yml`):

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo +1.88 check --locked --all-targets   # MSRV; rustup toolchain install 1.88
```

Integration tests under `tests/` build real git repositories in a temp
directory and run the `hub` binary with `tests/fake-bin` first on `PATH`.
The fakes there stand in for `tmux`, `code`, `lsof`, and `glab`/`gh`: each
records what it was called with to a log file named by a `FAKE_*_LOG`
environment variable (see `tests/common/mod.rs`) and keeps any state it
needs under `FAKE_TMUX_STATE` or `FAKE_PROVIDER_DIR`. Tests assert on those
logs instead of on a live tmux server. The fixture also points
`GIT_CONFIG_GLOBAL` at its own file, so your global git config never leaks
into a test.

The `skill` integration test checks that every command the table in
`skills/hub/SKILL.md` names still exists and that the generated copies stay
untracked, so a renamed or removed command needs a matching skill edit.

## Branches and pull requests

- Work on a branch; open a PR against `main`. CI must be green before merge.
- One logical change per PR. When the change is user-visible, add a changelog
  fragment in the same PR: one file under `changelog.d/` named
  `+<slug>.<type>.md`, where `<type>` is one of `added`, `changed`,
  `deprecated`, `removed`, `fixed`, `security` (the Keep a Changelog
  sections) and the content is the one-line entry as it should read in
  `CHANGELOG.md`. The leading `+` tells towncrier not to print an issue
  number. `uvx towncrier create +<slug>.<type>.md --content "..."` writes
  one, or create the file by hand. Never edit `CHANGELOG.md` directly for
  unreleased work; `uvx towncrier build --draft --version x.y.z` previews
  how the fragments will render. CI fails a PR that adds no fragment; label
  the PR `skip-changelog` when nothing in it is user-visible (CI, docs,
  refactors).
- A PR that carries a fragment also bumps `version` in `Cargo.toml` to a beta
  of the next release, then runs `cargo build` so `Cargo.lock` follows. The
  first such PR after a release sets `<next minor>-beta.1` (after 0.26.0,
  `0.27.0-beta.1`); each later one steps `beta.N` to `beta.N+1`. A
  `skip-changelog` PR leaves the version alone. CI rejects a non-release PR
  whose version is not a `-beta.N` prerelease or equals the one on `main`.
  Between releases, `cargo install --path .` installs the branch and
  `hub --version` names the build. Beta versions are never tagged: the
  no-bypass "no prerelease tags" ruleset refuses to create any tag containing
  a `-`, which every prerelease version has, so the release workflow and the
  crates.io publish only ever run for a plain `v<major>.<minor>.<patch>`.
- PRs are squash-merged (the repository allows nothing else), so the PR title
  becomes the commit on `main` as `<title> (#<number>)`; write it as the
  summary of the whole change.

## Releasing

A release is a version bump, a tag, and a push; everything after the push is
automated by `.github/workflows/release.yml`.

1. Decide the version: minor for a new capability or increment, patch for a
   fix. Anything that changes the binary (including `skills/hub/SKILL.md`,
   which is embedded in it) needs a bump. The beta on `main` is only a
   guess at this number; a patch release may replace `0.27.0-beta.3` with
   `0.26.1`.
2. Set `version` in `Cargo.toml` to the plain release version, run
   `cargo build` so `Cargo.lock` follows. CI checks that a PR editing
   `CHANGELOG.md` carries a plain `x.y.z` matching the newest changelog
   heading.
3. Compile the fragments into `CHANGELOG.md` (needs [uv](https://docs.astral.sh/uv/);
   towncrier itself is fetched on demand):

   ```bash
   uvx towncrier build --yes --version <version>
   ```

   This writes a `## [<version>] - <today>` section under the
   `<!-- towncrier release notes start -->` marker and deletes the fragments
   it consumed. Read the result and fix wording there if needed.
4. Commit as `Bump to <version>` in the PR, squash-merge it, then tag the
   resulting commit on `main` and push the tag:

   ```bash
   git switch main && git pull --ff-only
   git tag v<version>
   git push origin v<version>
   ```

5. The tag triggers the release workflow: it checks the tag against
   `Cargo.toml`, builds `hub` for macOS (arm64, x86_64) and Linux (x86_64),
   creates the GitHub Release with archives, checksums, the shell installer,
   and the changelog section as notes, then publishes the crate to crates.io
   (`publish-crates-io.yml`, through crates.io trusted publishing, so no
   token is stored). Watch it with `gh run watch`.
6. Update your own install: `cargo install --locked project-hub`.

`.github/workflows/release.yml` is generated by `dist` from
`dist-workspace.toml`; edit the TOML and run `dist generate`, never the YAML
by hand.
