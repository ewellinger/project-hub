use std::path::PathBuf;

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature};
use crate::hub::Hub;
use crate::workspace::{Folder, relative_path, render};

/// Where a change lives on disk: its worktree, or the main clone. Uses the
/// clone recorded on the change, so it works after `repo remove`.
pub fn change_path(hub: &Hub, change: &Change) -> PathBuf {
    match &change.worktree {
        None => hub.env.clone_path(&change.clone),
        Some(wt) => hub.env.worktree_path(&change.clone, &wt.name),
    }
}

/// Run an environment step (workspace file, tmux) whose failure is a
/// warning, never a rollback: recorded state is authoritative. The warning
/// line names the command that retries the step.
pub fn after_save(lines: &mut Vec<String>, step: Result<Vec<String>>, repair: &str) {
    match step {
        Ok(more) => lines.extend(more),
        Err(err) => lines.push(format!("warning: {err}; run `{repair}` to retry")),
    }
}

/// Open changes in manifest (deployment) order.
pub fn ordered_open_changes<'a>(hub: &Hub, feature: &'a Feature) -> Vec<&'a Change> {
    let mut changes = feature.open_changes();
    changes.sort_by_key(|c| hub.manifest.role_index(&c.role).unwrap_or(usize::MAX));
    changes
}

pub fn attach_line(feature: &Feature) -> String {
    format!("Attach with: tmux attach -t {}", feature.checkout)
}

pub const TMUX_OFF_LINE: &str = "tmux is off; no session created";

/// Ensure the session exists with windows `hub`, `ai`, then one per open
/// change in manifest order. Existing windows are never moved or killed, so
/// a pre-existing session gets missing windows appended: complete, but only
/// a fresh session is guaranteed the documented order. A session it creates
/// starts `hub` in the `hub` window.
pub fn converge_tmux(hub: &Hub, feature: &Feature) -> Result<Vec<String>> {
    let Some(tmux) = &hub.tmux else {
        let mut lines = hub.tmux_warning();
        lines.push(TMUX_OFF_LINE.to_string());
        return Ok(lines);
    };
    let session = &feature.checkout;
    let hub_wt = hub.hub_worktree_path(session);
    let mut lines = Vec::new();
    let created = !tmux.has_session(session)?;
    if created {
        tmux.new_session(session, "hub", &hub_wt)?;
        lines.push(format!("Created tmux session {session}"));
    }
    let existing = tmux.windows(session)?;
    for name in ["hub", "ai"] {
        if !existing.iter().any(|w| w == name) {
            tmux.new_window(session, name, &hub_wt)?;
            lines.push(format!("Added tmux window {name}"));
        }
    }
    for change in ordered_open_changes(hub, feature) {
        if !existing.iter().any(|w| w == &change.role) {
            tmux.new_window(session, &change.role, &change_path(hub, change))?;
            lines.push(format!("Added tmux window {}", change.role));
        }
    }
    // Through the shell, so quitting the TUI leaves a prompt in the window.
    if created && let Err(err) = tmux.send_keys(session, "hub", &["hub", "Enter"]) {
        lines.push(format!(
            "warning: {err}; run hub in the session's hub window"
        ));
    }
    lines.push(attach_line(feature));
    Ok(lines)
}

/// Write `<hub>.code-workspace` into the hub worktree, preserving other keys.
pub fn write_workspace(hub: &Hub, feature: &Feature) -> Result<PathBuf> {
    let hub_wt = hub.hub_worktree_path(&feature.checkout);
    let path = hub.workspace_path(feature);
    ensure_workspace_ignored(hub)?;
    let mut folders = vec![Folder {
        name: hub.manifest.name.clone(),
        path: ".".into(),
    }];
    for change in ordered_open_changes(hub, feature) {
        let target = change_path(hub, change);
        folders.push(Folder {
            name: change.role.clone(),
            path: relative_path(&hub_wt, &target)
                .to_string_lossy()
                .into_owned(),
        });
    }
    let existing = std::fs::read_to_string(&path).ok();
    let text = render(existing.as_deref(), &folders)?;
    std::fs::write(&path, text)
        .map_err(|e| HubError::io(format!("writing {}", path.display()), e))?;
    Ok(path)
}

/// Write `<hub>.code-workspace` into the hub's main checkout with every
/// registered repo's main clone in manifest order.
pub fn write_base_workspace(hub: &Hub) -> Result<PathBuf> {
    let path = hub.base_workspace_path();
    ensure_workspace_ignored(hub)?;
    let mut folders = vec![Folder {
        name: hub.manifest.name.clone(),
        path: ".".into(),
    }];
    for repo in &hub.manifest.repos {
        folders.push(Folder {
            name: repo.role.clone(),
            path: relative_path(&hub.root, &hub.env.clone_path(&repo.clone))
                .to_string_lossy()
                .into_owned(),
        });
    }
    let existing = std::fs::read_to_string(&path).ok();
    let text = render(existing.as_deref(), &folders)?;
    std::fs::write(&path, text)
        .map_err(|e| HubError::io(format!("writing {}", path.display()), e))?;
    Ok(path)
}

fn ensure_workspace_ignored(hub: &Hub) -> Result<()> {
    let filename = format!("{}.code-workspace", hub.manifest.name);
    let git = hub.git();
    if !git.is_ignored(&filename)? {
        git.exclude_locally(&format!("/{filename}"))?;
    }
    Ok(())
}
