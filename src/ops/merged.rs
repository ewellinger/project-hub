use std::path::{Path, PathBuf};

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Owner, Stage, now_rfc3339};
use crate::git::{Git, WorktreeRemoval};
use crate::hub::Hub;
use crate::ops::session::{after_save, write_workspace};
use crate::procs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    Removed,
    MainClone,
    AlreadyGone,
    Declined,
}

/// Refuse to remove `paths` while a process has its working directory inside
/// one of them: a dev server or test watcher recreates files while git deletes
/// the tree, which leaves an unregistered directory behind. `force` skips the
/// check. Returns a warning when `lsof` is not installed.
pub fn refuse_in_use(paths: &[PathBuf], force: bool) -> Result<Vec<String>> {
    if force {
        return Ok(Vec::new());
    }
    let Some(holders) = procs::cwd_holders()? else {
        return Ok(vec![
            "warning: lsof not found; skipped the check for processes running in the worktree"
                .to_string(),
        ]);
    };
    for path in paths {
        let inside = procs::holders_in(&holders, path);
        if !inside.is_empty() {
            return Err(HubError::InUse {
                path: path.clone(),
                processes: inside.iter().map(|h| h.describe()).collect(),
            });
        }
    }
    Ok(Vec::new())
}

/// The line describing a removal, including git's reason when the directory
/// could not be fully deleted.
pub fn removal_line(removal: &WorktreeRemoval, path: &Path, what: &str, kept: &str) -> String {
    match removal {
        WorktreeRemoval::Removed => {
            format!("{what}: removed worktree {} ({kept} kept)", path.display())
        }
        WorktreeRemoval::ResidueLeft(reason) => format!(
            "warning: {what}: worktree {} unregistered but its directory remains ({reason}); inspect and delete it by hand ({kept} kept)",
            path.display()
        ),
    }
}

/// Remove a change's worktree if it has one. Dirty trees need `force`;
/// adopted worktrees need `force` or a yes from `confirm`. Never deletes a branch.
pub fn close_change(
    hub: &Hub,
    change: &Change,
    force: bool,
    confirm: &mut dyn FnMut(&Path) -> bool,
) -> Result<(CloseOutcome, Vec<String>)> {
    let Some(wt) = &change.worktree else {
        return Ok((
            CloseOutcome::MainClone,
            vec![format!(
                "{}: {} lives in the main clone; nothing to remove",
                change.role, change.branch
            )],
        ));
    };
    let path = hub.env.worktree_path(&change.clone, &wt.name);
    if !path.exists() {
        return Ok((
            CloseOutcome::AlreadyGone,
            vec![format!(
                "{}: worktree {} is already gone",
                change.role,
                path.display()
            )],
        ));
    }
    let git = Git::new(&path);
    if git.is_dirty()? && !force {
        return Err(HubError::Dirty {
            path,
            status: git.status_short()?,
        });
    }
    if wt.owner == Owner::Adopted && !force && !confirm(&path) {
        return Ok((
            CloseOutcome::Declined,
            vec![format!(
                "{}: kept adopted worktree {}",
                change.role,
                path.display()
            )],
        ));
    }
    let removal = Git::new(hub.env.clone_path(&change.clone)).worktree_remove(&path, force)?;
    let kept = format!("branch {}", change.branch);
    Ok((
        CloseOutcome::Removed,
        vec![removal_line(&removal, &path, &change.role, &kept)],
    ))
}

pub fn merged(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    force: bool,
    confirm: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<String>> {
    feature.require_open()?;
    let change = feature.open_change(role).cloned().ok_or_else(|| {
        HubError::Precondition(format!(
            "no open change for role '{role}' in feature '{}'",
            feature.name
        ))
    })?;
    let mut lines = Vec::new();
    let session = feature.checkout.clone();
    let clone_dir = hub.env.clone_path(&change.clone);
    let path = change
        .worktree
        .as_ref()
        .map(|wt| hub.env.worktree_path(&change.clone, &wt.name))
        .filter(|p| p.exists());

    // Check the tree before touching anything: re-pointing the tmux window
    // below kills whatever runs in it, which a refused merge must not do.
    if let Some(path) = &path
        && !force
    {
        let git = Git::new(path);
        if git.is_dirty()? {
            return Err(HubError::Dirty {
                path: path.clone(),
                status: git.status_short()?,
            });
        }
    }

    // The role's tmux window sits in the worktree; re-point it first so its
    // shell, and anything started from it, cannot block the removal.
    if path.is_some() {
        lines.extend(hub.tmux_warning());
        if let Some(tmux) = &hub.tmux {
            let tmux_step = (|| -> Result<Vec<String>> {
                if tmux.has_session(&session)? && tmux.windows(&session)?.iter().any(|w| w == role)
                {
                    tmux.respawn_window(&session, role, &clone_dir)?;
                    return Ok(vec![format!(
                        "Re-pointed tmux window {role} at the main clone"
                    )]);
                }
                Ok(Vec::new())
            })();
            after_save(
                &mut lines,
                tmux_step,
                &format!("hub tmux --feature {}", feature.name),
            );
        }
    }
    if let Some(path) = &path {
        lines.extend(refuse_in_use(std::slice::from_ref(path), force)?);
    }

    // Transaction: the worktree removal is irreversible, so it happens first
    // and the record write follows. If the write fails the previous record is
    // left in place (stage still open) and rerunning `merged` finds the
    // worktree already gone and completes.
    let (outcome, closed) = close_change(hub, &change, force, confirm)?;
    if outcome == CloseOutcome::Declined {
        return Err(HubError::Precondition(format!(
            "{}; no worktree was removed. Rerun with --force to remove it",
            closed.join("; ")
        )));
    }
    lines.extend(closed);
    let open = feature.open_change_mut(role).expect("checked above");
    open.stage = Stage::Merged;
    open.merged_at = Some(now_rfc3339());
    hub.save_feature(feature)?;
    after_save(
        &mut lines,
        write_workspace(hub, feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );
    lines.push(format!("{role}: {} marked merged", change.branch));
    Ok(lines)
}
