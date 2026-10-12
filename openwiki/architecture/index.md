# Files

- [Hub state, manifest, and locking](hub-state-and-locking.md) - How `hub` locates a hub repository, what state is committed (`hub.json`) versus kept per machine (feature records under the git common dir), how the advisory lock serializes mutating commands, and how JSON is written atomically.
- [Architecture overview](overview.md) - Crate layout of project-hub (library `hub` plus binary `hub`), how modules are layered, how `src/main.rs` dispatches CLI subcommands to `ops::*`, and how `HubError` maps to process exit codes.
