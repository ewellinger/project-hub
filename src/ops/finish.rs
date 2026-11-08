use crate::error::{HubError, Result};
use crate::feature::{Feature, FeatureStatus};
use crate::git::Git;
use crate::hub::Hub;
use crate::ops::freeze::{frozen_removal_line, require_on_pin};
use crate::ops::merged::{close_change, refuse_in_use, removal_line};
use crate::ops::session::{ordered_open_changes, pin_path};

/// Close every open change's worktree and every frozen reference worktree,
/// remove the hub worktree, kill the session, and mark the feature finished.
/// Without `force`, a role still working or in review is refused, and so is a
/// frozen worktree whose HEAD left its pin or any dirty tree. Stages are left
/// as they are and no branch is deleted or merged.
pub fn finish(hub: &Hub, feature: &mut Feature, force: bool) -> Result<Vec<String>> {
    feature.require_open()?;
    // Killing the session would hang up the shell this command runs in, and
    // this command with it, before anything below could happen.
    if let Some(tmux) = &hub.tmux
        && tmux.current_session()? == Some(crate::tmux::session_name(&feature.checkout))
    {
        return Err(HubError::Precondition(format!(
            "running inside tmux session {}, which finish would kill along with this command; \
             run it from another terminal: hub feature finish --feature {}",
            crate::tmux::session_name(&feature.checkout),
            feature.name
        )));
    }
    let hub_wt = hub.hub_worktree_path(&feature.checkout);

    // Check every tree before touching any of them.
    if !force {
        // Stray commits first: the answer to a dirty refusal is --force,
        // which would discard them.
        for pin in &feature.pins {
            if let Some(path) = pin_path(hub, pin).filter(|p| p.exists()) {
                require_on_pin(&path, pin)?;
            }
        }
        let open = ordered_open_changes(hub, feature);
        if !open.is_empty() {
            let roles: Vec<_> = open
                .iter()
                .map(|c| format!("{} ({})", c.role, c.stage.as_str()))
                .collect();
            return Err(HubError::Precondition(format!(
                "feature '{}' has roles still open: {}\n  \
                 record each one first: hub feature merged ROLE, or hub feature dropped ROLE\n  \
                 or run hub feature finish --force to finish anyway, which also removes dirty worktrees",
                feature.name,
                roles.join(", ")
            )));
        }
        for pin in &feature.pins {
            let Some(path) = pin_path(hub, pin).filter(|p| p.exists()) else {
                continue;
            };
            let git = Git::new(&path);
            if git.is_dirty()? {
                return Err(HubError::Dirty {
                    path,
                    status: git.status_short()?,
                });
            }
        }
        if hub_wt.exists() {
            let git = Git::new(&hub_wt);
            if git.is_dirty()? {
                return Err(HubError::Dirty {
                    path: hub_wt,
                    status: git.status_short()?,
                });
            }
        }
    }

    // Transaction: the session kill and the removals are irreversible and
    // come first; the record write follows. If the write fails the previous
    // record is left in place (status still open) and rerunning `finish`
    // finds everything already gone and completes.
    let mut lines = hub.tmux_warning();
    // Nothing merges the hub branch; say what stays on it so the gap is
    // visible without a status call.
    if hub.git().local_branch_exists(&feature.name)? {
        let n = hub.git().missing_commits("main", &feature.name)?;
        if n > 0 {
            lines.push(format!(
                "note: hub branch {} has {n} commit{} not on main; they stay on the branch",
                feature.name,
                if n == 1 { "" } else { "s" }
            ));
        }
    }
    // The session's shells sit in the worktrees; kill it first so they, and
    // anything started from them, cannot block the removals.
    if let Some(tmux) = &hub.tmux
        && tmux.has_session(&feature.checkout)?
    {
        tmux.kill_session(&feature.checkout)?;
        lines.push(format!(
            "Killed tmux session {}",
            crate::tmux::session_name(&feature.checkout)
        ));
    }
    let open: Vec<_> = ordered_open_changes(hub, feature)
        .into_iter()
        .cloned()
        .collect();
    // Open changes get here only with `force`, which skips the in-use check.
    let mut paths: Vec<_> = feature
        .pins
        .iter()
        .filter_map(|p| pin_path(hub, p))
        .collect();
    paths.push(hub_wt.clone());
    paths.retain(|p| p.exists());
    lines.extend(refuse_in_use(&paths, force)?);

    for change in &open {
        lines.extend(close_change(hub, change, force)?);
    }
    for pin in &feature.pins {
        let Some(path) = pin_path(hub, pin).filter(|p| p.exists()) else {
            continue;
        };
        let removal = Git::new(hub.env.clone_path(&pin.clone)).worktree_remove(&path, force)?;
        lines.push(frozen_removal_line(&removal, &path, &pin.role));
    }
    feature.pins.clear();
    if hub_wt.exists() {
        let removal = hub.git().worktree_remove(&hub_wt, force)?;
        let kept = format!("branch {}", feature.name);
        lines.push(removal_line(&removal, &hub_wt, "hub", &kept));
    }
    feature.status = FeatureStatus::Finished;
    hub.save_feature(feature)?;
    lines.push(format!(
        "Feature '{}' finished; no branches were deleted",
        feature.name
    ));
    Ok(lines)
}
