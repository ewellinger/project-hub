use std::path::Path;

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Owner, Pin, Worktree, short_sha};
use crate::git::{Git, WorktreeRemoval};
use crate::hub::Hub;
use crate::ops::merged::refuse_in_use;
use crate::ops::resolve::Created;
use crate::ops::session::{after_save, converge_tmux, pin_path, repoint_window, write_workspace};

/// Directory name of a feature's frozen reference worktree under
/// `<worktree_dir>/<clone>/`.
pub fn frozen_worktree_name(feature: &Feature) -> String {
    format!("{}-frozen", feature.name)
}

/// The commit a pin takes: the tip of the local `base` branch, or `at`,
/// which must be that tip or one of its ancestors. Never fetches.
pub fn resolve_pin(git: &Git, base: &str, at: Option<&str>) -> Result<String> {
    if !git.local_branch_exists(base)? {
        return Err(HubError::Precondition(format!(
            "branch {base} not found in {}; pins are taken from the local base branch",
            git.dir().display()
        )));
    }
    let tip = git.rev_parse(&format!("refs/heads/{base}"))?;
    let Some(at) = at else {
        return Ok(tip);
    };
    let sha = git.rev_parse(at).map_err(|_| {
        HubError::Usage(format!(
            "'{at}' does not name a commit in {}",
            git.dir().display()
        ))
    })?;
    if !git.is_ancestor(&sha, &tip)? {
        return Err(HubError::Precondition(format!(
            "{at} is not on {base}; --at must name a commit the local {base} already contains"
        )));
    }
    Ok(sha)
}

/// The pin an open change takes by default: where its branch left the
/// local `base`. Never fetches.
pub fn change_pin(git: &Git, branch: &str, base: &str) -> Result<String> {
    let tip = resolve_pin(git, base, None)?;
    git.merge_base(branch, &tip)?.ok_or_else(|| {
        HubError::Precondition(format!(
            "{branch} and the local {base} share no history; there is no commit to pin"
        ))
    })
}

/// Pin a role's open change to the merge base of its branch and the local
/// `base`, or to `at`. No worktree: `status` measures the branch against
/// the pin instead of `origin/<base>`.
fn freeze_change(
    hub: &Hub,
    feature: &mut Feature,
    change: &Change,
    base: &str,
    at: Option<&str>,
) -> Result<Vec<String>> {
    if let Some(pin) = feature.pin(&change.role) {
        return Err(HubError::Precondition(format!(
            "role '{}' is already frozen at {}; use `hub feature refreeze {}` or `hub feature unfreeze {}`",
            change.role,
            pin.short(),
            change.role,
            change.role
        )));
    }
    let clone_dir = hub.env.clone_path(&change.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{}' is missing: {}",
            change.role,
            clone_dir.display()
        )));
    }
    let sha = match at {
        Some(_) => resolve_pin(&git, base, at)?,
        None => change_pin(&git, &change.branch, base)?,
    };
    let behind = git.missing_commits(&change.branch, &sha)?;
    feature.pins.push(Pin {
        role: change.role.clone(),
        clone: change.clone.clone(),
        base: base.to_string(),
        sha: sha.clone(),
        worktree: None,
    });
    if let Err(err) = hub.save_feature(feature) {
        feature.remove_pin(&change.role);
        return Err(err);
    }
    Ok(vec![format!(
        "{}: frozen at {} ({base}); {} is {behind} behind the pin",
        change.role,
        short_sha(&sha),
        change.branch
    )])
}

/// The step `freeze` and `start --freeze` share: add a frozen pin and its
/// reference worktree to `feature`. Refuses a role that is already frozen,
/// has an open change, or whose clone or worktree path is unusable. Checks
/// the local base tip (or `at`) out, detached, in a new worktree, records
/// that worktree in `created` so the caller can roll it back, and pushes
/// the `Pin` onto `feature.pins`. Does not save. On error nothing was
/// created and `feature` is unchanged.
pub fn add_frozen(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    at: Option<&str>,
    created: &mut Created,
) -> Result<String> {
    let repo = hub.manifest.require_repo(role)?.clone();
    if let Some(pin) = feature.pin(role) {
        return Err(HubError::Precondition(format!(
            "role '{role}' is already frozen at {}; use `hub feature refreeze {role}` or `hub feature unfreeze {role}`",
            pin.short()
        )));
    }
    if let Some(change) = feature.open_change(role) {
        return Err(HubError::Precondition(format!(
            "role '{role}' has an open change on branch '{}'; only roles outside the feature can be frozen",
            change.branch
        )));
    }
    let clone_dir = hub.env.clone_path(&repo.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{role}' is missing: {}",
            clone_dir.display()
        )));
    }
    let sha = resolve_pin(&git, &repo.base, at)?;
    let name = frozen_worktree_name(feature);
    let path = hub.env.worktree_path(&repo.clone, &name);
    if path.exists() {
        return Err(HubError::Precondition(format!(
            "{} already exists; move it aside or remove it first",
            path.display()
        )));
    }
    git.worktree_add_detached(&path, &sha)?;
    created.worktrees.push((clone_dir, path.clone()));
    let line = format!(
        "{role}: frozen at {} ({}) in {}",
        short_sha(&sha),
        repo.base,
        path.display()
    );
    feature.pins.push(Pin {
        role: role.to_string(),
        clone: repo.clone,
        base: repo.base,
        sha,
        worktree: Some(Worktree {
            name,
            owner: Owner::Hub,
        }),
    });
    Ok(line)
}

/// Pin a role to its local base (or `at`). A role outside the feature gets
/// a detached, hub-owned reference worktree; an open change gets a pin at
/// its merge base with the local base, and no worktree.
pub fn freeze(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    at: Option<&str>,
) -> Result<Vec<String>> {
    feature.require_open()?;
    // An open change is pinned in place; `add_frozen` stays for roles
    // outside the feature.
    if let Some(change) = feature.open_change(role).cloned() {
        let base = match change.effective_base(&hub.manifest) {
            Some(base) => base,
            None => hub.manifest.require_repo(role)?.base.clone(),
        };
        return freeze_change(hub, feature, &change, &base, at);
    }
    // Transaction: worktree and record write succeed together or the
    // worktree is removed again. Workspace and tmux come after the write.
    let mut created = Created::new();
    let mut lines = vec![add_frozen(hub, feature, role, at, &mut created)?];
    if let Err(err) = hub.save_feature(feature) {
        feature.remove_pin(role);
        let mut warnings = created.rollback();
        if warnings.is_empty() {
            return Err(err);
        }
        warnings.insert(0, err.to_string());
        return Err(HubError::Precondition(warnings.join("\n")));
    }

    let path = feature
        .pin(role)
        .and_then(|p| pin_path(hub, p))
        .expect("add_frozen pushed a pin with a worktree");
    after_save(
        &mut lines,
        write_workspace(hub, feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );
    // A window left from a merged change points at the main clone; re-point it.
    let tmux_step = repoint_window(hub, feature, role, &path).and_then(|mut out| {
        out.extend(converge_tmux(hub, feature)?);
        Ok(out)
    });
    after_save(
        &mut lines,
        tmux_step,
        &format!("hub tmux --feature {}", feature.name),
    );
    Ok(lines)
}

/// Refuse to move or remove a frozen worktree whose HEAD left its pin:
/// commits made on its detached HEAD are on no branch, so checking another
/// commit out or deleting the worktree would lose them. The dirty check
/// cannot see them. Returns the HEAD it found.
pub fn require_on_pin(path: &Path, pin: &Pin) -> Result<String> {
    let head = Git::new(path).rev_parse("HEAD")?;
    if head != pin.sha {
        return Err(HubError::Precondition(format!(
            "role '{}': frozen worktree {} is off its pin {} (HEAD is {}); it may hold commits that are on no branch. \
             Keep them with `git -C {} branch <name> {}` first; unfreeze and finish discard them with --force",
            pin.role,
            path.display(),
            pin.short(),
            short_sha(&head),
            path.display(),
            short_sha(&head)
        )));
    }
    Ok(head)
}

/// The line describing a frozen worktree's removal, including git's reason
/// when the directory could not be fully deleted.
pub fn frozen_removal_line(removal: &WorktreeRemoval, path: &Path, role: &str) -> String {
    match removal {
        WorktreeRemoval::Removed => format!("{role}: removed frozen worktree {}", path.display()),
        WorktreeRemoval::ResidueLeft(reason) => format!(
            "warning: {role}: frozen worktree {} unregistered but its directory remains ({reason}); inspect and delete it by hand",
            path.display()
        ),
    }
}

/// Release a role's pin. A frozen reference worktree is removed after its
/// tmux window is re-pointed at the main clone; a dirty worktree, or one
/// whose HEAD left the pin, needs `force`.
pub fn unfreeze(hub: &Hub, feature: &mut Feature, role: &str, force: bool) -> Result<Vec<String>> {
    feature.require_open()?;
    let pin = feature.pin(role).cloned().ok_or_else(|| {
        HubError::Precondition(format!(
            "role '{role}' is not frozen in feature '{}'",
            feature.name
        ))
    })?;
    let mut lines = Vec::new();
    let path = pin_path(hub, &pin).filter(|p| p.exists());
    if let Some(path) = &path {
        if !force {
            // Stray commits first: the answer to a dirty refusal is --force,
            // which would discard them.
            require_on_pin(path, &pin)?;
            let git = Git::new(path);
            if git.is_dirty()? {
                return Err(HubError::Dirty {
                    path: path.clone(),
                    status: git.status_short()?,
                });
            }
        }
        lines.extend(hub.tmux_warning());
        after_save(
            &mut lines,
            repoint_window(hub, feature, role, &hub.env.clone_path(&pin.clone)),
            &format!("hub tmux --feature {}", feature.name),
        );
        lines.extend(refuse_in_use(std::slice::from_ref(path), force)?);
        // Transaction: the removal is irreversible and comes first; if the
        // record write then fails, rerunning finds the worktree gone and completes.
        let removal = Git::new(hub.env.clone_path(&pin.clone)).worktree_remove(path, force)?;
        lines.push(frozen_removal_line(&removal, path, role));
    }
    feature.remove_pin(role);
    hub.save_feature(feature)?;
    after_save(
        &mut lines,
        write_workspace(hub, feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );
    lines.push(format!("{role}: unfrozen (was {})", pin.short()));
    Ok(lines)
}

/// Move a role's pin to the current tip of its local base and, for a
/// frozen reference, detach its worktree there. Never merges, and never
/// moves a worktree whose HEAD left the pin, even with nothing to move.
pub fn refreeze(hub: &Hub, feature: &mut Feature, role: &str) -> Result<Vec<String>> {
    feature.require_open()?;
    let pin = feature.pin(role).cloned().ok_or_else(|| {
        HubError::Precondition(format!(
            "role '{role}' is not frozen in feature '{}'",
            feature.name
        ))
    })?;
    let clone_dir = hub.env.clone_path(&pin.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{role}' is missing: {}",
            clone_dir.display()
        )));
    }
    let path = pin_path(hub, &pin);
    // The HEAD a failed record write puts the worktree back on.
    let head = match &path {
        Some(path) if !path.exists() => {
            return Err(HubError::Precondition(format!(
                "frozen worktree {} is missing; run hub status",
                path.display()
            )));
        }
        Some(path) => Some(require_on_pin(path, &pin)?),
        None => None,
    };
    let new = resolve_pin(&git, &pin.base, None)?;
    // A pinned change's lag against the new pin; refreezing never merges.
    let behind = match feature.open_change(role) {
        Some(change) => Some(format!(
            "  {} is {} behind the pin",
            change.branch,
            git.missing_commits(&change.branch, &new)?
        )),
        None => None,
    };
    if new == pin.sha {
        let mut lines = vec![format!(
            "{role}: already frozen at the tip of {} ({})",
            pin.base,
            pin.short()
        )];
        lines.extend(behind);
        return Ok(lines);
    }
    let moved = git.missing_commits(&pin.sha, &new)?;
    if let Some(path) = &path {
        let wt = Git::new(path);
        if wt.is_dirty()? {
            return Err(HubError::Dirty {
                path: path.clone(),
                status: wt.status_short()?,
            });
        }
        wt.checkout_detached(&new)?;
    }
    feature.pin_mut(role).expect("checked above").sha = new.clone();
    if let Err(err) = hub.save_feature(feature) {
        // Put the worktree back on the HEAD it had before this command.
        if let (Some(path), Some(head)) = (&path, &head)
            && let Err(back) = Git::new(path).checkout_detached(head)
        {
            return Err(HubError::Precondition(format!(
                "{err}\nrollback: could not move {} back to {}: {back}",
                path.display(),
                short_sha(head)
            )));
        }
        return Err(err);
    }
    let mut lines = vec![format!(
        "{role}: refrozen at {} ({} +{moved} since {})",
        short_sha(&new),
        pin.base,
        pin.short()
    )];
    lines.extend(behind);
    Ok(lines)
}
