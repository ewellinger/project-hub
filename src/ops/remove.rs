use std::collections::HashSet;
use std::path::PathBuf;

use crate::error::{HubError, Result};
use crate::feature::{self, Feature};
use crate::git::Git;
use crate::hub::Hub;

/// Delete a finished feature's local branches (every member branch and the
/// hub branch) and then its record, so `start` can use the name again with
/// fresh branches. Every branch is checked first and any refusal changes
/// nothing: a member branch must be marked merged or dropped (squash merges leave no
/// ancestry) or be contained in `origin/<base>`, and must be gone from origin,
/// where `start` would adopt it; the hub branch must have nothing beyond
/// `main`. Branches already deleted are skipped, so a rerun completes.
pub fn remove(hub: &Hub, feature: &Feature) -> Result<Vec<String>> {
    if feature.is_open() {
        return Err(HubError::Precondition(format!(
            "feature '{}' is open; run hub feature finish --feature {} first",
            feature.name, feature.name
        )));
    }
    let mut lines = Vec::new();
    let mut refusals = Vec::new();
    // (repo dir, label, branch) for every branch to delete, hub last.
    let mut doomed: Vec<(PathBuf, String, String)> = Vec::new();
    // Follow-up changes share a clone; fetch it once.
    let mut fetched: HashSet<&str> = HashSet::new();
    // A branch reopened by `feature add --branch` has several records.
    let mut seen: HashSet<(&str, &str)> = HashSet::new();

    for change in &feature.changes {
        if !seen.insert((change.clone.as_str(), change.branch.as_str())) {
            continue;
        }
        let dir = hub.env.clone_path(&change.clone);
        let git = Git::new(&dir);
        if fetched.insert(change.clone.as_str()) {
            fetch(&git, &dir, &mut lines);
        }
        let branch = &change.branch;
        let label = format!("{}: {branch}", change.role);
        if git.remote_branch_exists(branch)? {
            refusals.push(format!("{label} is still on origin; delete it there"));
            continue;
        }
        if !git.local_branch_exists(branch)? {
            continue;
        }
        // Full ref names: a bare name would resolve to a tag of the same name first.
        let head = format!("refs/heads/{branch}");
        let contained = match change.effective_base(&hub.manifest) {
            Some(base) => {
                let base = format!("origin/{base}");
                git.ref_exists(&format!("refs/remotes/{base}"))? && git.is_ancestor(&head, &base)?
            }
            None => false,
        };
        // Any record of the branch can hold it open: `feature add --branch`
        // can reopen a dropped or merged branch, and later records are skipped.
        let open = feature
            .changes
            .iter()
            .any(|c| c.clone == change.clone && c.branch == *branch && c.is_open());
        if open && !contained {
            refusals.push(format!(
                "{label} is not merged; run hub feature merged or hub feature dropped, or delete the branch in {}",
                dir.display()
            ));
            continue;
        }
        refuse_checked_out(&git, branch, &label, &mut refusals)?;
        doomed.push((dir, label, branch.clone()));
    }

    let git = hub.git();
    let branch = &feature.name;
    let label = format!("hub: {branch}");
    if git.has_remotes()? {
        fetch(&git, &hub.root, &mut lines);
    }
    if git.remote_branch_exists(branch)? {
        refusals.push(format!("{label} is still on origin; delete it there"));
    } else if git.local_branch_exists(branch)? {
        if git.is_ancestor(&format!("refs/heads/{branch}"), "main")? {
            refuse_checked_out(&git, branch, &label, &mut refusals)?;
            doomed.push((hub.root.clone(), label, branch.clone()));
        } else {
            refusals.push(format!(
                "{label} has commits not on main; keep what you need and delete the branch in {}",
                hub.root.display()
            ));
        }
    }

    if !refusals.is_empty() {
        refusals.insert(0, format!("not removing feature '{}':", feature.name));
        return Err(HubError::Precondition(refusals.join("\n  ")));
    }
    for (dir, label, branch) in &doomed {
        let git = Git::new(dir);
        let tip = git.rev_parse(&format!("refs/heads/{branch}"))?;
        git.delete_branch(branch)?;
        lines.push(format!(
            "Deleted branch {label} (was {})",
            feature::short_sha(&tip)
        ));
    }
    let path = hub.feature_path(&feature.name);
    std::fs::remove_file(&path)
        .map_err(|e| HubError::io(format!("removing {}", path.display()), e))?;
    // The record is gone; a leftover reviews file or its lock is harmless.
    let reviews = hub.reviews_path(&feature.name);
    let _ = std::fs::remove_file(reviews.with_extension("lock"));
    let _ = std::fs::remove_file(reviews);
    lines.push(format!("Removed feature '{}'", feature.name));
    Ok(lines)
}

/// Fetch so a branch deleted on origin is seen as gone; offline is a warning.
fn fetch(git: &Git, dir: &std::path::Path, lines: &mut Vec<String>) {
    if let Err(err) = git.fetch("origin") {
        lines.push(format!(
            "warning: fetch failed in {}: {err}; using the last fetched refs",
            dir.display()
        ));
    }
}

/// `git branch -D` refuses a branch checked out anywhere.
fn refuse_checked_out(
    git: &Git,
    branch: &str,
    label: &str,
    refusals: &mut Vec<String>,
) -> Result<()> {
    if let Some(wt) = git
        .worktrees()?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch))
    {
        refusals.push(format!("{label} is checked out at {}", wt.path.display()));
    }
    Ok(())
}
