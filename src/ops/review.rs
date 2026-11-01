use crate::error::{HubError, Result};
use crate::feature::{Feature, Stage};
use crate::git::Git;
use crate::hub::Hub;

/// Mark the role's open change as in review, record the URL, and note whether
/// the branch is on origin (feeds the "gone from origin" hint in `status`).
pub fn review(
    hub: &Hub,
    feature: &mut Feature,
    role: &str,
    url: Option<&str>,
) -> Result<Vec<String>> {
    feature.require_open()?;
    hub.manifest.require_repo(role)?;
    let (clone, branch) = feature
        .open_change(role)
        .map(|c| (c.clone.clone(), c.branch.clone()))
        .ok_or_else(|| {
            HubError::Precondition(format!(
                "no open change for role '{role}' in feature '{}'",
                feature.name
            ))
        })?;
    let git = Git::new(hub.env.clone_path(&clone));
    // The fetch only feeds the origin note; offline is a warning, not an error.
    let mut lines = Vec::new();
    if let Err(err) = git.fetch("origin") {
        lines.push(format!(
            "warning: fetch failed in {}: {err}; using the last fetched refs",
            git.dir().display()
        ));
    }
    let on_origin = git.remote_branch_exists(&branch)?;
    let change = feature.open_change_mut(role).expect("checked above");
    change.stage = Stage::Review;
    if url.is_some() {
        change.review_url = url.map(str::to_string);
    }
    change.origin_seen = change.origin_seen || on_origin;
    hub.save_feature(feature)?;
    lines.push(format!("{role}: {branch} is in review"));
    if let Some(url) = url {
        lines.push(format!("  {url}"));
    }
    if !on_origin {
        lines.push(format!("  note: {branch} is not on origin yet"));
    }
    Ok(lines)
}
