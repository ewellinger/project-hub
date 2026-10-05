//! Side effects of the dashboard: status requests on background threads,
//! the action that touches the filesystem (opening a workspace), and the
//! clipboard.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::mpsc::Sender;
use std::thread;

use hub::error::HubError;
use hub::feature::Feature;
use hub::hub::Hub;
use hub::ops;
use hub::ops::resolve::AddOptions;
use hub::ops::start::StartOptions;
use hub::ops::suggest;

use crate::ui::app::{Action, Event, Report, RowId};

/// Run the row's status on a detached thread and send one `Event::Report`
/// tagged with `gen`. The app drops stale generations, so a thread that
/// outlives the request it served is harmless.
pub fn request(hub: &Hub, row: RowId, r#gen: u64, tx: Sender<Event>) {
    let hub = hub.clone();
    thread::spawn(move || {
        let result = match &row {
            RowId::Base => ops::status::base_status(&hub).map(Report::Base),
            RowId::Feature(name) => hub.load_feature(name).and_then(|f| {
                ops::status::status(&hub, &f).map(|report| Report::Feature { feature: f, report })
            }),
        };
        let _ = tx.send(Event::Report { row, r#gen, result });
    });
}

/// Same as `hub open` for the row: write the workspace file, launch the editor
/// with its stdio silenced. Runs on a detached thread, since it would
/// otherwise block the event loop and inherit the terminal, which is in
/// raw mode; sends one `Event::Opened`.
pub fn open_workspace(hub: &Hub, row: RowId, tx: Sender<Event>) {
    let hub = hub.clone();
    thread::spawn(move || {
        let result = (|| {
            let path = match &row {
                RowId::Base => ops::session::write_base_workspace(&hub)?,
                RowId::Feature(name) => {
                    let f = hub.load_feature(name)?;
                    f.require_open()?;
                    ops::session::write_workspace(&hub, &f)?
                }
            };
            hub::code::open_workspace_quiet(&hub.env.editor, &path)?;
            Ok(path)
        })();
        let _ = tx.send(Event::Opened(result));
    });
}

/// Put `text` on the OS clipboard on a detached thread and send one
/// `Event::Copied` carrying `label`. `arboard` talks to the clipboard
/// directly (macOS, X11, Wayland, Windows), so no subprocess touches the
/// terminal, which is in raw mode; without a reachable clipboard, as in a
/// headless session, the error says so.
pub fn copy(label: String, text: String, tx: Sender<Event>) {
    thread::spawn(move || {
        let result = set_clipboard(&text).map(|()| label);
        let _ = tx.send(Event::Copied(result));
    });
}

/// One clipboard handle for the life of the process. On X11 the process
/// that set the selection serves it, and arboard gives it up when the
/// handle drops, so a per-copy handle would lose the text as soon as the
/// footer said "copied" unless a clipboard manager took it over. Held
/// here, the text stays available while the dashboard runs.
static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

fn set_clipboard(text: &str) -> Result<(), HubError> {
    let mut slot = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let clipboard = arboard::Clipboard::new()
            .map_err(|e| HubError::Precondition(format!("no clipboard available: {e}")))?;
        *slot = Some(clipboard);
    }
    let result = slot
        .as_mut()
        .expect("filled above")
        .set_text(text)
        .map_err(|e| HubError::Precondition(format!("clipboard write failed: {e}")));
    if result.is_err() {
        // Reconnect on the next copy rather than reuse a broken handle.
        *slot = None;
    }
    result
}

/// Run one action under the hub lock on a detached thread and send one
/// `Event::Done`. The feature record is loaded inside the lock so the op
/// sees the state as it is now, not as the dashboard last cached it.
pub fn run(hub: &Hub, action: Action, tx: Sender<Event>) {
    let hub = hub.clone();
    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(|| {
            hub.with_lock(|hub| match &action {
                Action::Start { name, repos } => ops::start::start(
                    hub,
                    &StartOptions {
                        name: name.clone(),
                        checkout: None,
                        repos: repos.clone(),
                    },
                ),
                Action::Add {
                    feature,
                    role,
                    branch,
                } => {
                    let mut f = hub.load_feature(feature)?;
                    ops::add::add(
                        hub,
                        &mut f,
                        &AddOptions {
                            role,
                            branch: Some(branch),
                            from: None,
                            worktree: None,
                            base: None,
                        },
                    )
                }
                Action::Review { feature, role, url } => {
                    let mut f = hub.load_feature(feature)?;
                    ops::review::review(hub, &mut f, role, url.as_deref())
                }
                Action::SetBase {
                    feature,
                    role,
                    branch,
                } => {
                    let mut f = hub.load_feature(feature)?;
                    ops::set_base::set_base(hub, &mut f, role, branch)
                }
                Action::Merged {
                    feature,
                    role,
                    force,
                } => {
                    let mut f = hub.load_feature(feature)?;
                    ops::merged::merged(hub, &mut f, role, *force, &mut |_| true)
                }
                Action::Finish { feature, force } => {
                    let mut f = hub.load_feature(feature)?;
                    ops::finish::finish(hub, &mut f, *force, &mut |_| true)
                }
                Action::Pull { row: RowId::Base } => ops::pull::pull_base(hub),
                Action::Pull {
                    row: RowId::Feature(name),
                } => {
                    let f = hub.load_feature(name)?;
                    ops::pull::pull_feature(hub, &f)
                }
            })
        }));
        let result = unwind_to_result(result, &action);
        let _ = tx.send(Event::Done { action, result });
    });
}

/// The op's result, or a `Precondition` error when the op thread panicked.
/// Without this a panicking op would never send `Done`, and the dashboard
/// would stay busy until it was restarted.
fn unwind_to_result(
    result: thread::Result<Result<Vec<String>, HubError>>,
    action: &Action,
) -> Result<Vec<String>, HubError> {
    result.unwrap_or_else(|_| {
        Err(HubError::Precondition(format!(
            "the {action} worker panicked; check the hub state with hub status"
        )))
    })
}

/// `start`'s preflight (name, checkout, nothing exists yet) off the event
/// loop; sends one `Event::Preflighted` with the resolved checkout name,
/// tagged with the name it was asked to preflight so a stale reply can be
/// told apart from the one a later request is waiting on.
pub fn preflight(hub: &Hub, name: String, tx: Sender<Event>) {
    let hub = hub.clone();
    thread::spawn(move || {
        let result = ops::start::preflight(&hub, &name, None);
        let _ = tx.send(Event::Preflighted { name, result });
    });
}

/// Build the branch picker rows for `role` off the event loop: the default
/// branch plus every suggestion git already knows. Reads refs only.
pub fn load_branches(hub: &Hub, feature: Feature, role: String, tx: Sender<Event>) {
    let hub = hub.clone();
    thread::spawn(move || {
        let result = suggest::default_branch(&hub, &feature, &role).and_then(|default| {
            suggest::suggestions(&hub, &feature, &role).map(|s| suggest::branch_rows(default, s))
        });
        let _ = tx.send(Event::Branches { role, result });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The thread plumbing is not unit-testable (a panicking thread plus a
    /// panic hook), but the mapping from a `catch_unwind` result to the
    /// event's result is.
    #[test]
    fn a_panicking_op_becomes_a_precondition_error() {
        let action = Action::Finish {
            feature: "suspense".into(),
            force: false,
        };
        let panicked: thread::Result<Result<Vec<String>, HubError>> = Err(Box::new("boom"));
        assert_eq!(
            unwind_to_result(panicked, &action).unwrap_err().to_string(),
            "the finish suspense worker panicked; check the hub state with hub status"
        );
        let ok = unwind_to_result(Ok(Ok(vec!["Feature finished".into()])), &action);
        assert_eq!(ok.unwrap(), vec!["Feature finished".to_string()]);
        let failed = unwind_to_result(
            Ok(Err(HubError::Precondition("nothing to finish".into()))),
            &action,
        );
        assert_eq!(failed.unwrap_err().to_string(), "nothing to finish");
    }
}
