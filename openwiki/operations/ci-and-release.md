---
type: operations
title: CI and release
description: The checks every pull request must pass (fmt, clippy, tests on Linux and macOS, MSRV 1.88), the dist-generated tag-driven release workflow that builds binaries and a shell installer, crates.io trusted publishing, and the version/CHANGELOG procedure.
tags: [operations, ci, release, crates-io, cargo-dist, changelog]
sources:
  - id: openwiki-source-1e075575622e1a77a3dc46e6
    resource: repo://.github/pull_request_template.md
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
  - id: openwiki-source-eaeff68d3e917151c3a9cf0b
    resource: repo://towncrier.toml
generated: { by: "claude-code", at: "2026-11-09T20:44:31.979Z" }
verified:
  - by: openwiki/0.9.0
    at: 2026-11-09T20:44:31.979Z
---

# CI and release

## CI (`.github/workflows/ci.yml`)

Runs on pushes to `main` and on every pull request, as five jobs:

| Job | Toolchain | Command |
|---|---|---|
| `fmt` | stable + rustfmt | `cargo fmt --check` |
| `clippy` | stable + clippy | `cargo clippy --all-targets --locked -- -D warnings` |
| `test` | stable, matrix `ubuntu-latest` + `macos-latest` | `cargo test --locked` |
| `msrv` | 1.88 | `cargo check --locked --all-targets` |
| `changelog` | uv | `uvx towncrier check --compare-with origin/main`, then a version check (pull requests only; skipped by the `skip-changelog` label) |

The cargo steps set `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1`
so the runner's git config cannot leak into tests: unit tests set their own
identity per repo, and the integration fixture supplies its own global git
config (see [Testing and the integration harness](../testing/integration-test-harness.md)).
Actions are pinned to commit SHAs.

Contributors run the same commands locally (listed in `CONTRIBUTING.md`).
PRs are squash-merged, so the PR title becomes the commit on `main` as
`<title> (#<number>)`.

### Changelog fragments and beta versions

The changelog is built with towncrier (`towncrier.toml`). A user-visible change
adds one fragment in the same PR, `changelog.d/+<slug>.<type>.md`, where
`<type>` is one of `added`, `changed`, `deprecated`, `removed`, `fixed`,
`security` and the content is the entry as it should read; nobody edits
`CHANGELOG.md` for unreleased work. The `changelog` job fails a PR that adds no
fragment unless it is labelled `skip-changelog`.

A PR that carries a fragment also bumps `version` in `Cargo.toml` to a beta of
the next release: the first after a release sets `<next minor>-beta.1`, each
later one steps `beta.N`. The same job enforces it by PR kind: a PR that does
not touch `CHANGELOG.md` must carry a `x.y.z-beta.N` version different from the
one on `main`; a PR that does (a release PR) must carry a plain `x.y.z` that
has a `## [x.y.z]` section in `CHANGELOG.md`. Beta versions are never tagged: a
repository ruleset refuses any tag containing `-`, so the release workflow and
the crates.io publish only run for plain versions.

## Release

A release is a version bump, a tag, and a push:

1. Choose the version: minor for a new capability, patch for a fix. Anything
   that changes the binary needs a bump, including every `skills/hub/*.md` file,
   all of which are embedded at build time (see [Bundled agent skill](../integrations/agent-skill.md)).
   The beta on `main` is only a guess at this number.
2. Set `version` in `Cargo.toml` to the plain release version and build so
   `Cargo.lock` follows.
3. Run `uvx towncrier build --yes --version <version>`, which writes a
   `## [<version>] - <today>` section under the
   `<!-- towncrier release notes start -->` marker in `CHANGELOG.md` and
   deletes the fragments it consumed.
4. Squash-merge the bump PR, then tag the resulting `main` commit `v<version>`
   and push the tag.

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
