use crate::error::{HubError, Result};
use crate::feature::{Feature, Owner, Pin, Worktree, short_sha};
use crate::git::Git;
use crate::hub::Hub;
use crate::ops::resolve::Created;
use crate::ops::session::{after_save, converge_tmux, repoint_window, write_workspace};

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

/// Pin a role with no open change to its local base (or `at`) and check
/// the commit out, detached, in a hub-owned reference worktree.
pub fn freeze(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    at: Option<&str>,
) -> Result<Vec<String>> {
    feature.require_open()?;
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

    // Transaction: worktree and record write succeed together or the
    // worktree is removed again. Workspace and tmux come after the write.
    let mut created = Created::new();
    let saved = git.worktree_add_detached(&path, &sha).and_then(|()| {
        created.worktrees.push((clone_dir.clone(), path.clone()));
        feature.pins.push(Pin {
            role: role.to_string(),
            clone: repo.clone.clone(),
            base: repo.base.clone(),
            sha: sha.clone(),
            worktree: Some(Worktree {
                name,
                owner: Owner::Hub,
            }),
        });
        hub.save_feature(feature)
    });
    if let Err(err) = saved {
        feature.remove_pin(role);
        let mut warnings = created.rollback();
        if warnings.is_empty() {
            return Err(err);
        }
        warnings.insert(0, err.to_string());
        return Err(HubError::Precondition(warnings.join("\n")));
    }

    let mut lines = vec![format!(
        "{role}: frozen at {} ({}) in {}",
        short_sha(&sha),
        repo.base,
        path.display()
    )];
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
