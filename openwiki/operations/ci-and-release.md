---
type: operations
title: CI and release
description: The checks every pull request must pass (fmt, clippy, tests on Linux and macOS, MSRV 1.88), the dist-generated tag-driven release workflow that builds binaries and a shell installer, crates.io trusted publishing, and the version/CHANGELOG procedure.
tags: [operations, ci, release, crates-io, cargo-dist, changelog]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
sources:
  - id: openwiki-source-164e2da859b5277df81c7d94
    resource: repo://.github/workflows/ci.yml
  - id: openwiki-source-ae71a77808247924e19d6d00
    resource: repo://.github/workflows/publish-crates-io.yml
  - id: openwiki-source-4d1d392666be6dfdd7a91a2e
    resource: repo://.github/workflows/release.yml
  - id: openwiki-source-f317ee207e1653d2033c81a4
    resource: repo://CONTRIBUTING.md
  - id: openwiki-source-09a6e8c3317970f7ee55af63
    resource: repo://dist-workspace.toml
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# CI and release

## CI (`.github/workflows/ci.yml`)

Runs on pushes to `main` and on every pull request, as four jobs:

| Job | Toolchain | Command |
|---|---|---|
| `fmt` | stable + rustfmt | `cargo fmt --check` |
| `clippy` | stable + clippy | `cargo clippy --all-targets --locked -- -D warnings` |
| `test` | stable, matrix `ubuntu-latest` + `macos-latest` | `cargo test --locked` |
| `msrv` | 1.88 | `cargo check --locked --all-targets` |

The cargo steps set `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`
so the runner's git config cannot leak into tests: unit tests set their own
identity per repo, and the integration fixture supplies its own global git
config (see [Testing and the integration harness](../testing/integration-test-harness.md)).
Actions are pinned to commit SHAs.

Contributors run the same commands locally (listed in `CONTRIBUTING.md`).
PRs are squash-merged, so the PR title becomes the commit on `main` as
`<title> (#<number>)`. User-visible changes add an entry under `## [Unreleased]`
in `CHANGELOG.md` in the same PR.

## Release

A release is a version bump, a tag, and a push:

1. Choose the version: minor for a new capability, patch for a fix. Anything
   that changes the binary needs a bump, including `skills/hub/SKILL.md`,
   which is embedded at build time (see [Bundled agent skill](../integrations/agent-skill.md)).
2. Set `version` in `Cargo.toml` and build so `Cargo.lock` follows.
3. In `CHANGELOG.md` (Keep a Changelog format), rename `[Unreleased]` to
   `[<version>] - <date>`, add a fresh `[Unreleased]`, and update compare links.
4. Merge the bump PR, then tag the resulting `main` commit `v<version>` and
   push the tag.

### Release workflow (`.github/workflows/release.yml`)

`release.yml` is **generated** by `dist` (cargo-dist 0.32.0) from
`dist-workspace.toml`; edit the TOML and run `dist generate`, never the YAML.
The TOML configures:

- targets `aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `x86_64-unknown-linux-gnu`;
- a `shell` installer that installs into `CARGO_HOME` (`~/.cargo/bin`), with no updater;
- `pr-run-mode = "plan"` (PRs only plan);
- a custom publish job `./publish-crates-io` with `id-token: write`;
- SHA-pinned action commits.

On a version tag the workflow plans, builds per-target archives and global
artifacts, creates the GitHub Release (archives, checksums, installer,
changelog section as notes) in the `host` job, then calls the publish job,
then `announce`. The `dist` release profile in `Cargo.toml` inherits
`release` with thin LTO.

### crates.io (`.github/workflows/publish-crates-io.yml`)

A reusable workflow called by `release.yml` after the GitHub Release exists.
It authenticates through **crates.io trusted publishing**
(`rust-lang/crates-io-auth-action` exchanging an OIDC token), so no registry
token secret is stored, and runs `cargo publish --locked`. Prereleases are
skipped unless dist is configured to publish them. The published crate
excludes `/tests`, `/.github`, `/docs`, and dev config files (`exclude` in
`Cargo.toml`).

## Related pages

- [Testing and the integration harness](../testing/integration-test-harness.md)
- [Bundled agent skill](../integrations/agent-skill.md)
