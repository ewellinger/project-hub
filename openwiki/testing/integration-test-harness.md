---
type: testing
title: Testing and the integration harness
description: How project-hub is tested — unit tests inside modules, and integration tests that build real git repositories in a temp directory and run the `hub` binary with fake tmux, code, lsof, gh, and glab on PATH — plus the fault-injection helpers and which test file covers which command.
tags: [testing, integration-tests, fixtures, fakes, assert_cmd]
verified:
  - by: openwiki/0.7.2
    at: 2026-10-12T04:10:52.427Z
sources:
  - id: openwiki-source-5c70522e7407c31f9c2fa754
    resource: repo://tests/common/mod.rs
  - id: openwiki-source-cf826a8dbc3ecf1fcd3659fa
    resource: repo://tests/fake-bin/glab
  - id: openwiki-source-d2553b8006bcf868b54ed156
    resource: repo://tests/fake-bin/lsof
  - id: openwiki-source-14f64ae4f9fdaf3f49cfc5e8
    resource: repo://tests/fake-bin/tmux
  - id: openwiki-source-f13236b3ab41e3f4cea296b6
    resource: repo://tests/misc.rs
generated: { by: "claude-code", at: "2026-10-12T04:10:52.427Z" }
---

# Testing and the integration harness

Run everything with `cargo test` (CI uses `cargo test --locked` on Linux and
macOS; see [CI and release](../operations/ci-and-release.md)).

## Two kinds of tests

- **Unit tests** live in `#[cfg(test)] mod tests` blocks inside the modules
  they cover: parsers and validators (`names`, `review`, `procs`, `env`,
  `manifest`, `feature`, `error`), git helpers against throwaway repos, and
  the dashboard state machine. `src/ui/app/*` is written as a pure update
  function precisely so it can be tested with synthetic events and no
  terminal (see [Dashboard architecture](../ui/dashboard-architecture.md)).
- **Integration tests** in `tests/*.rs` run the real `hub` binary
  (`assert_cmd::Command::cargo_bin("hub")`) against real git repositories.

## The `Fixture` (`tests/common/mod.rs`)

`Fixture::new()` creates a temp "world":

| Path / env | Purpose |
|---|---|
| `projects/` → `PROJECT_HOME` | main clones |
| `worktrees/` → `GIT_WORKTREE_DIR` | worktrees |
| `remotes/` | bare repos acting as `origin` |
| `gitconfig` → `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_NOSYSTEM=1` | isolated identity and `init.defaultBranch=main`, so the developer's git config never leaks in |
| empty `xdg/` → `XDG_CONFIG_HOME` | the developer's own `hub/config.toml` never reaches the binary |
| `tests/fake-bin` prepended to `PATH` | fake external tools |
| `tmux/`, `tmux.log`, `code.log`, `provider/`, `provider.log` | fake-tool state and logs |

`Fixture::hub(cwd, args)` also removes `TMUX` from the environment so running
the suite inside tmux does not change behavior.

Common builders: `make_repo(clone, base)` (bare remote with one commit, cloned
into `projects/`), `remote_branch`, `commit_file`, `init_hub`, `add_repo`,
`make_provider_repo` (records a provider URL such as
`https://gitlab.example.com/g/proj.git` as `origin` while a gitconfig
`insteadOf` rewrite points fetches at the local bare repo), and readers such as
`feature_json`, `tmux_log`, `window_names`.

### Fault injection

| Helper | Mechanism | Used to test |
|---|---|---|
| `block_commits` | failing `pre-commit` hook | rollback when a `hub.json` commit fails |
| `block_feature_writes` | read-only `.git/hub/features` | rollback when the atomic record write fails |
| `block_deletion` | excluded read-only `keep/` dir in a worktree | `ResidueLeft` warnings after `merged`/`finish` |
| `seed_session` | pre-made fake tmux session | converging an existing session |
| `path_without_tmux` | PATH with git and fakes but no tmux | tmux-off behavior |

## Fake binaries (`tests/fake-bin/`)

Each fake records its invocation to a log named by an environment variable;
tests assert on logs and state instead of a live tmux server or network.

- **`tmux`** — sessions are directories under `$FAKE_TMUX_STATE`, each with a
  `windows` file of `name<TAB>dir` lines; every call is appended to
  `$FAKE_TMUX_LOG`. `FAKE_TMUX_FAIL=<subcommand>` forces that subcommand to fail.
- **`code`** — appends its arguments to `$FAKE_CODE_LOG`.
- **`lsof`** — prints `-F pcn` records from `$FAKE_LSOF_CWDS`
  (`pid<TAB>command<TAB>cwd` lines); `FAKE_LSOF_SELF` also reports lsof, hub,
  and hub's parent as real lsof would, to test self-filtering.
- **`gh` / `glab`** — support only `api`; log to `$FAKE_PROVIDER_LOG` and print
  `$FAKE_PROVIDER_DIR/<key>.json` where the key is the endpoint with `/`→`__`
  and `%`→`_` (`provider_response`); `provider_response_next` supplies a
  different answer from the second call on, to test "review changed while
  checking". `FAKE_PROVIDER_FAIL` makes every call fail.

## Where commands are covered

| File | Covers |
|---|---|
| `tests/init.rs` | `hub init`, `--force` refresh |
| `tests/repo.rs` | `repo add/set/remove/rename/list` |
| `tests/config.rs` | `hub config` show/set/unset |
| `tests/start.rs` | `feature start` |
| `tests/add.rs`, `tests/resolve.rs` | `feature add`, change opening and adoption |
| `tests/suggest.rs` | branch suggestions for prompts/pickers |
| `tests/start_review.rs` | `feature start-review` |
| `tests/set_base.rs` | `feature set-base` |
| `tests/review_merged.rs` | `feature review`, `feature merged` |
| `tests/finish.rs` | `feature finish` |
| `tests/list_status.rs` | `feature list`, `status` |
| `tests/pull.rs` | `hub pull` |
| `tests/tmux_off.rs` | behavior with sessions off |
| `tests/skill.rs` | generated agent skill and its command table |
| `tests/misc.rs` | `tmux`/`open`/`sync`, exit codes, the lock, finished-feature read-only rules, record location |

## Related pages

- [CI and release](../operations/ci-and-release.md)
- [Git, tmux, and editor integration](../integrations/git-tmux-editor.md)
