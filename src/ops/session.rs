use std::path::{Path, PathBuf};

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Pin};
use crate::hub::Hub;
use crate::names;
use crate::workspace::{Folder, relative_path, render};

/// Where a change lives on disk: its worktree, or the main clone. Uses the
/// clone recorded on the change, so it works after `repo remove`.
pub fn change_path(hub: &Hub, change: &Change) -> PathBuf {
    match &change.worktree {
        None => hub.env.clone_path(&change.clone),
        Some(wt) => hub.env.worktree_path(&change.clone, &wt.name),
    }
}

/// Where a frozen reference worktree lives; `None` for a pin that belongs
/// to the role's open change.
pub fn pin_path(hub: &Hub, pin: &Pin) -> Option<PathBuf> {
    pin.worktree
        .as_ref()
        .map(|wt| hub.env.worktree_path(&pin.clone, &wt.name))
}

/// Every checkout the feature binds a role to, open changes and frozen
/// reference worktrees alike, in manifest (deployment) order.
pub fn role_checkouts(hub: &Hub, feature: &Feature) -> Vec<(String, PathBuf)> {
    let mut checkouts: Vec<(String, PathBuf)> = feature
        .open_changes()
        .into_iter()
        .map(|c| (c.role.clone(), change_path(hub, c)))
        .chain(
            feature
                .pins
                .iter()
                .filter_map(|p| pin_path(hub, p).map(|path| (p.role.clone(), path))),
        )
        .collect();
    checkouts.sort_by_key(|(role, _)| hub.manifest.role_index(role).unwrap_or(usize::MAX));
    checkouts
}

/// Point the role's tmux window, when the session has one, at `dir`,
/// killing whatever ran in it. Lets a worktree about to be removed go free.
pub fn repoint_window(hub: &Hub, feature: &Feature, role: &str, dir: &Path) -> Result<Vec<String>> {
    let Some(tmux) = &hub.tmux else {
        return Ok(Vec::new());
    };
    let session = &feature.checkout;
    if tmux.has_session(session)? && tmux.windows(session)?.iter().any(|w| w == role) {
        tmux.respawn_window(session, role, dir)?;
        return Ok(vec![format!(
            "Re-pointed tmux window {role} at {}",
            dir.display()
        )]);
    }
    Ok(Vec::new())
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
    attach_session_line(&feature.checkout)
}

fn attach_session_line(session: &str) -> String {
    format!("Attach with: tmux attach -t {session}")
}

pub const TMUX_OFF_LINE: &str = "tmux is off; no session created";

/// The session `hub tmux` makes on `main`: `checkout_template` rendered
/// with `main` as the feature.
pub fn base_session_name(hub: &Hub) -> String {
    names::checkout_name(hub.manifest.checkout_template(), "main", &hub.manifest.name)
}

/// Ensure the session exists with windows `hub`, `ai`, then one per open
/// change or frozen role in manifest order. Existing windows are never moved or killed, so
/// a pre-existing session gets missing windows appended: complete, but only
/// a fresh session is guaranteed the documented order. A session it creates
/// starts `hub` in the `hub` window.
pub fn converge_tmux(hub: &Hub, feature: &Feature) -> Result<Vec<String>> {
    let hub_wt = hub.hub_worktree_path(&feature.checkout);
    converge_session(
        hub,
        &feature.checkout,
        &hub_wt,
        role_checkouts(hub, feature),
    )
}

/// `converge_tmux` for the base checkouts: `hub` and `ai` in the hub's main
/// clone, then every registered repo's main clone in manifest order.
pub fn converge_base_tmux(hub: &Hub) -> Result<Vec<String>> {
    let roles = hub
        .manifest
        .repos
        .iter()
        .map(|r| (r.role.clone(), hub.env.clone_path(&r.clone)))
        .collect();
    converge_session(hub, &base_session_name(hub), &hub.root, roles)
}

fn converge_session(
    hub: &Hub,
    session: &str,
    hub_dir: &Path,
    roles: Vec<(String, PathBuf)>,
) -> Result<Vec<String>> {
    let Some(tmux) = &hub.tmux else {
        let mut lines = hub.tmux_warning();
        lines.push(TMUX_OFF_LINE.to_string());
        return Ok(lines);
    };
    let mut lines = Vec::new();
    let created = !tmux.has_session(session)?;
    if created {
        tmux.new_session(session, "hub", hub_dir)?;
        lines.push(format!("Created tmux session {session}"));
    }
    let existing = tmux.windows(session)?;
    for name in ["hub", "ai"] {
        if !existing.iter().any(|w| w == name) {
            tmux.new_window(session, name, hub_dir)?;
            lines.push(format!("Added tmux window {name}"));
        }
    }
    for (role, path) in roles {
        if !existing.iter().any(|w| w == &role) {
            tmux.new_window(session, &role, &path)?;
            lines.push(format!("Added tmux window {role}"));
        }
    }
    // Through the shell, so quitting the TUI leaves a prompt in the window.
    if created && let Err(err) = tmux.send_keys(session, "hub", &["hub", "Enter"]) {
        lines.push(format!(
            "warning: {err}; run hub in the session's hub window"
        ));
    }
    lines.push(attach_session_line(session));
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
    for (role, target) in role_checkouts(hub, feature) {
        folders.push(Folder {
            name: role,
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
