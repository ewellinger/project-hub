use std::path::{Path, PathBuf};

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Owner, Stage, now_rfc3339};
use crate::git::{Git, WorktreeRemoval};
use crate::hub::Hub;
use crate::ops::session::{after_save, repoint_window, write_workspace};
use crate::procs;

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

/// Remove a change's worktree if it has one. Dirty trees need `force`. An
/// adopted worktree is removed too, so callers ask first. Never deletes a branch.
pub fn close_change(hub: &Hub, change: &Change, force: bool) -> Result<Vec<String>> {
    let Some(wt) = &change.worktree else {
        return Ok(vec![format!(
            "{}: {} lives in the main clone; nothing to remove",
            change.role, change.branch
        )]);
    };
    let path = hub.env.worktree_path(&change.clone, &wt.name);
    if !path.exists() {
        return Ok(vec![format!(
            "{}: worktree {} is already gone",
            change.role,
            path.display()
        )]);
    }
    let git = Git::new(&path);
    if git.is_dirty()? && !force {
        return Err(HubError::Dirty {
            path,
            status: git.status_short()?,
        });
    }
    let removal = Git::new(hub.env.clone_path(&change.clone)).worktree_remove(&path, force)?;
    let kept = format!("branch {}", change.branch);
    Ok(vec![removal_line(&removal, &path, &change.role, &kept)])
}

pub fn merged(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    force: bool,
    confirm: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<String>> {
    close(hub, feature, role, Stage::Merged, force, confirm)
}

pub fn dropped(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    force: bool,
    confirm: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<String>> {
    close(hub, feature, role, Stage::Dropped, force, confirm)
}

/// Record a role's open change as `stage` (merged or dropped) and remove its
/// worktree. On a finished feature the worktrees are already gone, so only the
/// record changes.
fn close(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    stage: Stage,
    force: bool,
    confirm: &mut dyn FnMut(&Path) -> bool,
) -> Result<Vec<String>> {
    let change = feature.open_change(role).cloned().ok_or_else(|| {
        HubError::Precondition(format!(
            "no open change for role '{role}' in feature '{}'",
            feature.name
        ))
    })?;
    let word = stage.as_str();
    let mut lines = Vec::new();
    if !feature.is_open() {
        record_stage(feature, role, stage);
        hub.save_feature(feature)?;
        lines.push(format!(
            "{role}: {} marked {word} (feature already finished)",
            change.branch
        ));
        return Ok(lines);
    }
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

    // An adopted worktree is removed only on a yes; ask before the re-point
    // below kills the shell in it, for the same reason as the dirty check.
    let adopted = change
        .worktree
        .as_ref()
        .is_some_and(|wt| wt.owner == Owner::Adopted);
    if let Some(path) = &path
        && adopted
        && !force
        && !confirm(path)
    {
        return Err(HubError::Precondition(format!(
            "{role}: kept adopted worktree {}; no worktree was removed. Rerun with --force to remove it",
            path.display()
        )));
    }

    // The role's tmux window sits in the worktree; re-point it first so its
    // shell, and anything started from it, cannot block the removal.
    if path.is_some() {
        lines.extend(hub.tmux_warning());
        if hub.tmux.is_some() {
            after_save(
                &mut lines,
                repoint_window(hub, feature, role, &clone_dir),
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
    lines.extend(close_change(hub, &change, force)?);
    record_stage(feature, role, stage);
    // The pin belonged to the change; the role goes back to absent.
    let pin = feature.remove_pin(role);
    hub.save_feature(feature)?;
    after_save(
        &mut lines,
        write_workspace(hub, feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );
    lines.push(format!("{role}: {} marked {word}", change.branch));
    if let Some(pin) = pin {
        lines.push(format!("{role}: pin at {} dropped", pin.short()));
    }
    Ok(lines)
}

fn record_stage(feature: &mut Feature, role: &str, stage: Stage) {
    let open = feature.open_change_mut(role).expect("checked above");
    open.stage = stage;
    let now = Some(now_rfc3339());
    if stage == Stage::Dropped {
        open.dropped_at = now;
    } else {
        open.merged_at = now;
    }
}
