use std::path::Path;

use crate::error::{HubError, Result};
use crate::feature::{Feature, FeatureStatus};
use crate::git::Git;
use crate::hub::Hub;
use crate::ops::merged::{close_change, refuse_in_use, removal_line};
use crate::ops::session::{change_path, ordered_open_changes};

/// Close every open change's worktree, remove the hub worktree, kill the
/// session, and mark the feature finished. Stages are left as they are and no
/// branch is deleted or merged.
pub fn finish(
    hub: &Hub,
    feature: &mut Feature,
    force: bool,
    confirm: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<String>> {
    feature.require_open()?;
    // Killing the session would hang up the shell this command runs in, and
    // this command with it, before anything below could happen.
    if let Some(tmux) = &hub.tmux
        && tmux.current_session()?.as_deref() == Some(feature.checkout.as_str())
    {
        return Err(HubError::Precondition(format!(
            "running inside tmux session {}, which finish would kill along with this command; \
             run it from another terminal: hub feature finish --feature {}",
            feature.checkout, feature.name
        )));
    }
    let hub_wt = hub.hub_worktree_path(&feature.checkout);

    // Check every tree before touching any of them.
    if !force {
        for change in ordered_open_changes(hub, feature) {
            if change.worktree.is_none() {
                continue;
            }
            let path = change_path(hub, change);
            if path.exists() {
                let git = Git::new(&path);
                if git.is_dirty()? {
                    return Err(HubError::Dirty {
                        path,
                        status: git.status_short()?,
                    });
                }
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
    // The session's shells sit in the worktrees; kill it first so they, and
    // anything started from them, cannot block the removals.
    if let Some(tmux) = &hub.tmux
        && tmux.has_session(&feature.checkout)?
    {
        tmux.kill_session(&feature.checkout)?;
        lines.push(format!("Killed tmux session {}", feature.checkout));
    }
    let open: Vec<_> = ordered_open_changes(hub, feature)
        .into_iter()
        .cloned()
        .collect();
    let mut paths: Vec<_> = open
        .iter()
        .filter(|c| c.worktree.is_some())
        .map(|c| change_path(hub, c))
        .collect();
    paths.push(hub_wt.clone());
    paths.retain(|p| p.exists());
    lines.extend(refuse_in_use(&paths, force)?);

    for change in &open {
        let (_, closed) = close_change(hub, change, force, confirm)?;
        lines.extend(closed);
    }
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
