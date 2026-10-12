# Files

- [Dashboard architecture](dashboard-architecture.md) - How the ratatui dashboard (bare `hub` in a terminal) is structured — an event loop around a pure `App::update(Event) -> Vec<Effect>` state machine, background worker threads for status, actions, editor, and clipboard, generation-tagged status requests, and pure rendering — and how dashboard actions reuse the same `ops` functions under the hub lock.
- [Dashboard flows and overlays](dashboard-flows-and-overlays.md) - How the dashboard's multi-step interactions work — the Flow state machines (new-feature wizard, add role, review, set-base, merged and finish previews, copy), the Overlay widgets and their key handling, the feature screen's keys and cursor mapping, and the busy, cancel, and blocked rules.
