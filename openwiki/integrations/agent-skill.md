---
type: integration
title: Bundled agent skill
description: How `skills/hub/SKILL.md` is embedded into the `hub` binary, written into every hub worktree for Claude Code and Codex on every command, kept untracked through Git's local exclude file, and guarded by the `skill` integration test.
tags: [agent-skill, claude-code, codex, generated-files, integration]
sources:
  - id: openwiki-source-e4a8b7a4fbda1ff7b227f6cf
    resource: repo://skills/hub/SKILL.md
  - id: openwiki-source-b55a21a31ede1b56cd31a6a6
    resource: repo://src/main.rs
  - id: openwiki-source-03561c75d5622526b12cd559
    resource: repo://src/ops/init.rs
  - id: openwiki-source-e782c5002a54f711ccbae710
    resource: repo://src/ops/start_review.rs
  - id: openwiki-source-3f5c288f59bdbf6815ffcff1
    resource: repo://src/ops/start.rs
  - id: openwiki-source-93462e3771be5237fabe2c28
    resource: repo://src/skill.rs
  - id: openwiki-source-ec32cfd44569eaa164e68020
    resource: repo://tests/skill.rs
generated: { by: "claude-code", at: "2026-10-12T18:18:24.526Z" }
verified:
  - by: openwiki/0.7.2
    at: 2026-10-13T22:12:12.058Z
---

# Bundled agent skill

project-hub ships an agent skill that teaches Claude Code and Codex how to drive
the `hub` binary. The single source of truth is `skills/hub/SKILL.md`; the
code that distributes it is `src/skill.rs`.

## Embedding and placement

- `skill::SKILL` is `include_str!("../skills/hub/SKILL.md")`, so the skill is
  compiled into the binary. Editing `SKILL.md` therefore changes the binary and
  needs a version bump when released (see [CI and release](../operations/ci-and-release.md)).
- `skill::SKILL_DIRS` lists the two destinations inside a worktree:
  `.claude/skills/hub/` (Claude Code) and `.agents/skills/hub/` (Codex), each
  with a `SKILL.md`.
- Nothing is installed at user scope; each hub carries the copy matching the
  installed `hub`.

## When it is written

| Trigger | Code | Behavior |
|---|---|---|
| Every hub-scoped command | `main.rs` `locate()` → `skill::refresh(&hub)` | Ensures both directories are in Git's local exclude file and installs the skill in every existing hub worktree directory |
| `hub init` (and `--force`) | `ops::init` | Adds the directories to `.gitignore` and the local exclude, installs into the hub root |
| `feature start`, `start-review` | `ops::start`, `ops::start_review` | Installs into the new hub worktree |

`skill::install` only writes when a copy is missing or differs from the
embedded text, so up-to-date files are untouched while an edited copy is
overwritten by the next command. `skill::ensure_ignored` adds each directory to
the repository's local exclude file (shared by all linked worktrees) so the
generated copies never show up as untracked.

## Failure handling

Skill problems never fail a command. `refresh` returns one message per failure
(exclude failure, or install failure per worktree), and `locate()` prints them
as `warning:` lines on **stderr** so `--json` output on stdout stays clean.
Worktrees whose directory has vanished behind git's back are skipped.
`init` and `start` report install failures as warnings noting that any later
hub command retries.

## What the skill says

`SKILL.md` (frontmatter `name: hub`) is operational guidance for agents:

- **Rules**: run `hub status` before trusting any path; exit 30 means drift —
  stop; refer to repos by role, not path; never push or pull the hub on the
  user's behalf.
- **Commands** table mapping intents to `hub` subcommands.
- **Reading output**: take paths from `hub status --json` `rows[].path` and
  the attach command from `checkout`; field meanings such as `base`,
  `base_behind`, `custom_base`, `review_url`, and for frozen bases
  `frozen_sha`, `base_moved`, and `feature list --json`'s `frozen` array.
- **Workflow** responses to exit codes 20/21, `warning:` lines, tmux-off,
  adopted worktrees, and finished features.
- **Frozen roles**: a `stage` `frozen` row is a read-only reference checkout
  the agent must never edit, commit in, or `git checkout`; `refreeze` runs
  only when the user asks; choosing between starting from the pin and
  `--from-latest` on `feature add` is the user's call. An "is off its pin"
  refusal means commits on no branch, and `--force` past it needs the user's
  explicit yes (`add` has no `--force`).
- Procedures for **reviewing a merge/pull request**, handling **dirty
  worktrees**, and **updating from the base** (a local `git merge --ff` of
  `origin/<base>` only on explicit request, never push).

## Test guard

`tests/skill.rs` keeps the skill and binary in sync:

- `every_command_in_the_skill_table_exists` strips placeholders and flags from
  every `hub ...` command in the skill's table and asks the binary for its
  `--help`, so renaming or removing a command requires a matching skill edit.
- Other tests check that any command restores the skill in every worktree,
  that hubs created before the skill existed stay clean, that a worktree
  removed behind git's back does not fail the command, and that `init` and
  `feature start` install it.

## Related pages

- [Hub init and repo registration](../workflows/hub-and-repo-management.md)
- [CI and release](../operations/ci-and-release.md)
