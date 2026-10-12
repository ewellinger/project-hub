# Files

- [Bundled agent skill](agent-skill.md) - How `skills/hub/SKILL.md` is embedded into the `hub` binary, written into every hub worktree for Claude Code and Codex on every command, kept untracked through Git's local exclude file, and guarded by the `skill` integration test.
- [Git, tmux, and editor integration](git-tmux-editor.md) - The thin subprocess wrappers project-hub uses for git (worktrees, refs, fetches, conditional ref updates), tmux (sessions and windows addressed exactly), and the editor, plus how feature sessions converge and how `.code-workspace` files are generated and kept local.
