use crate::error::{HubError, Result};
use crate::feature::Feature;
use crate::git::Git;
use crate::hub::Hub;

/// Record `branch` as the base of the role's open change and re-derive
/// `base_sha` as the merge base of the feature branch and `origin/<branch>`.
/// Touches only the feature file: no checkout, reset, merge, rebase, or
/// change to git's upstream configuration.
pub fn set_base(hub: &Hub, feature: &mut Feature, role: &str, branch: &str) -> Result<Vec<String>> {
    feature.require_open()?;
    hub.manifest.require_repo(role)?;
    if let Some(bare) = branch.strip_prefix("origin/") {
        return Err(HubError::Usage(format!(
            "pass the branch name without origin/: {bare}"
        )));
    }
    let change = feature.open_change(role).cloned().ok_or_else(|| {
        HubError::Precondition(format!(
            "no open change for role '{role}' in feature '{}'",
            feature.name
        ))
    })?;
    let clone_dir = hub.env.clone_path(&change.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{role}' is missing: {}",
            clone_dir.display()
        )));
    }
    if !git.check_ref_format(branch)? {
        return Err(HubError::Usage(format!(
            "'{branch}' is not a valid branch name"
        )));
    }
    git.fetch("origin")?;
    let base_ref = format!("origin/{branch}");
    if !git.remote_branch_exists(branch)? {
        return Err(HubError::Precondition(format!(
            "{base_ref} not found in {}; the base must exist on origin",
            clone_dir.display()
        )));
    }
    if !git.local_branch_exists(&change.branch)? {
        return Err(HubError::Precondition(format!(
            "branch {} is missing in {}; run hub status",
            change.branch,
            clone_dir.display()
        )));
    }
    let base_sha = git.merge_base(&change.branch, &base_ref)?.ok_or_else(|| {
        HubError::Precondition(format!("{} and {base_ref} share no history", change.branch))
    })?;
    if change.base.as_deref() == Some(branch) && change.base_sha.as_deref() == Some(&base_sha) {
        return Ok(vec![format!("{role}: base already {branch} at {base_sha}")]);
    }
    let old = change
        .effective_base(&hub.manifest)
        .unwrap_or_else(|| "none".to_string());
    let open = feature.open_change_mut(role).expect("checked above");
    open.base = Some(branch.to_string());
    open.base_sha = Some(base_sha.clone());
    hub.save_feature(feature)?;
    Ok(vec![
        format!("{role}: base {branch} (was {old})"),
        format!("  base_sha {base_sha}"),
    ])
}
