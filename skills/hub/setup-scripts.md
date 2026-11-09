# Writing a setup script

A role's `setup` in `hub.json` names a script that hub runs inside each new worktree it creates for that role, so the worktree is usable at once: `uv sync` for a `.venv`, `npm ci` for `node_modules`, a local config file copied in. Read this before writing, changing, or registering one.

## The contract

- **Location.** The path is relative to the hub repo and is resolved against the hub's **main clone**, not the feature worktree you are in: a script that exists only on a feature's hub branch is not found until it is on `main` there. Absolute paths and `..` are refused. Where scripts live (for example `tools/hub-setup/<role>.sh`) is the hub's own decision; follow its instructions file, and record a new convention there.
- **Execution.** Hub executes the file directly: it needs a shebang line and the executable bit. A missing file, a directory, or a file without the executable bit gives `setup could not start`.
- **Working directory** is the new worktree. **stdin is closed** (`/dev/null`): nothing may prompt. stdout and stderr go to the log.
- **Environment.** The script inherits the environment of the process that ran `hub` (your shell, the agent's sandbox, or the dashboard's terminal), plus:

  | Variable | Value |
  |---|---|
  | `HUB_ROLE` | the role |
  | `HUB_FEATURE` | the feature |
  | `HUB_BRANCH` | the branch checked out in the worktree |
  | `HUB_WORKTREE` | the worktree (also the working directory) |
  | `HUB_BASE` | the role's base checkout, its main clone (a path, not a branch) |
  | `HUB_ROOT` | the hub's main clone |

- **Exit status.** 0 is `setup ok`. Non-zero is `setup failed (exit N)` with the last 20 log lines; death by signal is `setup killed by a signal`. A failure never rolls back the feature and never changes the exit code of `start`, `add`, or `start-review`; only `hub feature setup` exits 1 when a script fails.
- **No timeout.** Hub waits for the script to exit. A script that hangs keeps `start`, `add`, `start-review`, or `feature setup` from returning (other hub commands still run, since the lock is already released), and the dashboard refuses to quit while one runs. Never wait on input, a lock, or a network call that has no timeout of its own.

## When it runs

- After `hub feature start`, `hub feature add`, and `hub feature start-review`, once the feature record is written and the hub lock released, one role at a time in manifest order. `--no-setup` skips it.
- Only in worktrees hub created. An adopted worktree, a branch used in place in the main clone, and a frozen reference checkout never run it automatically.
- `hub feature setup` reruns every hub-created worktree's script; `hub feature setup ROLE` reruns one role's, adopted worktree included. A change living in the main clone is refused.
- A second run of the same role while one is running gives `setup could not start: setup already running for ROLE`.

## Writing it

- **Idempotent.** A rerun lands in a worktree that already has local state. Running twice must be safe and leave the worktree as one run would.
- **Never overwrite per-worktree files.** Untracked files such as `.env` may hold the user's edits. Copy one in only when it is absent, usually from `$HUB_BASE`.
- **`set -eu`** so the first failing step fails the run and an unset variable is an error, not an empty string.
- **Pin tools explicitly.** The inherited `PATH` differs between a terminal, an agent sandbox, and the dashboard, and can select the wrong runtime (for example a Homebrew Node instead of the project's). Use the project's version manager (`mise exec -- npm ci`, a `.nvmrc`, `uv`) or an absolute path, following the member repo's own instructions.
- **Check credentials up front.** An install that needs registry credentials gets them only from the inherited environment. Test for the variable and exit with a message that names it, rather than letting the installer fail obscurely.
- **Work relative to the worktree.** Use the working directory or `$HUB_WORKTREE`; read from `$HUB_BASE` or `$HUB_ROOT`, never write there.

```sh
#!/bin/sh
# Setup for the api role; hub runs it in each new api worktree.
set -eu

# Seed local config once; a rerun keeps the worktree's own copy.
if [ ! -e .env ] && [ -e "$HUB_BASE/.env" ]; then
  cp "$HUB_BASE/.env" .env
fi

: "${REGISTRY_TOKEN:?export REGISTRY_TOKEN before running hub}"
exec uv sync --locked
```

## Registering it

`hub repo add --setup` and `hub repo set --setup` check only the path's form. They do not check that the file exists, is executable, or is tracked, and they commit **only `hub.json`** on `main` in the main clone (`hub: repo set ROLE`). A pull of that commit without the script gives everyone `setup could not start`. So:

1. Write the script in the hub's main clone on `main` (its `path` is the `hub` row of `hub status --base --json`), `chmod +x` it, and commit it there with any instructions-file entry for its location. `git ls-files -s <path>` must show mode `100755`.
2. Run `hub repo set ROLE --setup <path>` (or `repo add … --setup <path>`); it adds its own `hub.json` commit.
3. Push the two commits together, and only when the user asks.

`hub repo set ROLE --setup ""` clears the script.

## Testing it

Test both paths before calling a script done.

- **Rerun path:** `hub feature setup ROLE --feature <open feature>`, then read `.git/hub/setup/<feature>/<role>.log` in the hub's main clone. The log is rewritten on each run. Run it twice to check idempotence.
- **Fresh path:** start a throwaway feature with that role (`hub feature start <name> --repo ROLE`), check the result line and the log, then close it with `hub feature dropped ROLE --feature <name>` and `hub feature finish --feature <name>` from outside its session; its branches are kept. Or run the script by hand in a scratch directory with every `HUB_*` variable exported and stdin closed (`< /dev/null`).
