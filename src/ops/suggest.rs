use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use crate::env::{home_dir, shorten_home};
use crate::error::{HubError, Result};
use crate::feature::Feature;
use crate::git::{Git, same_path};
use crate::hub::Hub;
use crate::manifest::RepoSpec;
use crate::ops::resolve;

/// A branch the picker can offer for a role: where git knows it from and,
/// when it is checked out somewhere the hub can adopt, that path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub branch: String,
    pub local: bool,
    pub origin: bool,
    /// Adoptable worktree the branch is checked out in: the main clone or a
    /// directory directly under `$GIT_WORKTREE_DIR/<clone>/`.
    pub worktree: Option<PathBuf>,
}

/// The branch `add` would use for `role` without `--branch`: the template
/// for a first change, `-<n>` after a merged one.
pub fn default_branch(hub: &Hub, feature: &Feature, role: &str) -> Result<String> {
    let (repo, git) = clone_git(hub, role)?;
    resolve::default_branch(feature, repo, &git)
}

/// Manifest repos, in manifest order, whose role has no open change in
/// `feature`. Errors when none remain, so a role picker can guard on it.
pub fn addable_roles<'a>(hub: &'a Hub, feature: &Feature) -> Result<Vec<&'a RepoSpec>> {
    let roles: Vec<&RepoSpec> = hub
        .manifest
        .repos
        .iter()
        .filter(|r| feature.open_change(&r.role).is_none())
        .collect();
    if roles.is_empty() {
        return Err(HubError::Precondition(format!(
            "every role already has an open change in {}",
            feature.name
        )));
    }
    Ok(roles)
}

/// Branches in the role's clone worth offering, in spec order: the default
/// branch first when it exists, then names containing the feature name,
/// then the rest, each group by name. Reads git only; never fetches.
pub fn suggestions(hub: &Hub, feature: &Feature, role: &str) -> Result<Vec<Suggestion>> {
    let (repo, git) = clone_git(hub, role)?;
    let clone_dir = git.dir().to_path_buf();

    let mut found: BTreeMap<String, Suggestion> = BTreeMap::new();
    for refname in git.refnames(&["refs/heads", "refs/remotes/origin"])? {
        let (branch, local) = if let Some(b) = refname.strip_prefix("refs/heads/") {
            (b, true)
        } else if let Some(b) = refname.strip_prefix("refs/remotes/origin/") {
            if b == "HEAD" {
                continue;
            }
            (b, false)
        } else {
            continue;
        };
        let entry = found
            .entry(branch.to_string())
            .or_insert_with(|| Suggestion {
                branch: branch.to_string(),
                local: false,
                origin: false,
                worktree: None,
            });
        if local {
            entry.local = true;
        } else {
            entry.origin = true;
        }
    }
    found.remove(&repo.base);

    for wt in git.worktrees()? {
        let Some(branch) = wt.branch else {
            continue;
        };
        let path = wt.path.canonicalize().unwrap_or(wt.path);
        let adoptable =
            same_path(&path, &clone_dir) || hub.env.worktree_name(&repo.clone, &path).is_some();
        if adoptable {
            if let Some(s) = found.get_mut(&branch) {
                s.worktree = Some(path);
            }
        } else {
            // Checked out somewhere the hub cannot adopt: offering it would
            // only ever produce an error (parent spec §3 refuses it).
            found.remove(&branch);
        }
    }

    let default = resolve::default_branch(feature, repo, &git)?;
    let mut list: Vec<Suggestion> = found.into_values().collect();
    // Stable sort: BTreeMap already yielded names in order within each group.
    list.sort_by_key(|s| (s.branch != default, !s.branch.contains(&feature.name)));
    Ok(list)
}

fn clone_git<'a>(hub: &'a Hub, role: &str) -> Result<(&'a RepoSpec, Git)> {
    let repo = hub.manifest.require_repo(role)?;
    let clone_dir = hub.env.clone_path(&repo.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{role}' is missing: {}",
            clone_dir.display()
        )));
    }
    Ok((repo, git))
}

/// Rows of a branch picker, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchRow {
    /// The branch `add` would pick on its own; `existing` is set when a ref
    /// of that name already exists, so the row shows where it lives.
    Default {
        branch: String,
        existing: Option<Suggestion>,
    },
    Suggested(Suggestion),
    Other,
}

impl BranchRow {
    /// The branch this row stands for; `None` for `Other`.
    pub fn branch(&self) -> Option<&str> {
        match self {
            BranchRow::Default { branch, .. } => Some(branch),
            BranchRow::Suggested(s) => Some(&s.branch),
            BranchRow::Other => None,
        }
    }
}

impl fmt::Display for BranchRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BranchRow::Default {
                branch,
                existing: None,
            } => write!(f, "{branch}  new"),
            BranchRow::Default {
                existing: Some(s), ..
            } => f.write_str(&branch_label(s)),
            BranchRow::Suggested(s) => f.write_str(&branch_label(s)),
            BranchRow::Other => f.write_str("type another name"),
        }
    }
}

/// Default first (pulled out of `suggestions` when present so it is not
/// listed twice), then the remaining suggestions in their order, then `Other`.
pub fn branch_rows(default: String, suggestions: Vec<Suggestion>) -> Vec<BranchRow> {
    let mut existing = None;
    let mut rest = Vec::with_capacity(suggestions.len());
    for s in suggestions {
        if s.branch == default && existing.is_none() {
            existing = Some(s);
        } else {
            rest.push(s);
        }
    }
    let mut rows = vec![BranchRow::Default {
        branch: default,
        existing,
    }];
    rows.extend(rest.into_iter().map(BranchRow::Suggested));
    rows.push(BranchRow::Other);
    rows
}

/// `branch  local, origin, checked out at ~/path`.
pub fn branch_label(s: &Suggestion) -> String {
    let mut notes = vec![
        match (s.local, s.origin) {
            (true, true) => "local, origin",
            (true, false) => "local",
            _ => "origin",
        }
        .to_string(),
    ];
    if let Some(path) = &s.worktree {
        notes.push(format!(
            "checked out at {}",
            shorten_home(path, home_dir().as_deref())
        ));
    }
    format!("{}  {}", s.branch, notes.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn s(branch: &str, local: bool, origin: bool, worktree: Option<&str>) -> Suggestion {
        Suggestion {
            branch: branch.into(),
            local,
            origin,
            worktree: worktree.map(PathBuf::from),
        }
    }

    #[test]
    fn branch_label_names_where_the_branch_lives() {
        assert_eq!(branch_label(&s("a", true, false, None)), "a  local");
        assert_eq!(branch_label(&s("a", false, true, None)), "a  origin");
        assert_eq!(branch_label(&s("a", true, true, None)), "a  local, origin");
        let with_wt = s("a", true, true, Some("/opt/wt/api/a"));
        assert_eq!(
            branch_label(&with_wt),
            "a  local, origin, checked out at /opt/wt/api/a"
        );
    }

    #[test]
    fn branch_rows_put_the_default_first_once_and_end_with_other() {
        let rows = branch_rows(
            "feat-1".into(),
            vec![s("feat-1", true, false, None), s("x", false, true, None)],
        );
        let labels: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            labels,
            vec!["feat-1  local", "x  origin", "type another name"]
        );
        assert!(matches!(
            &rows[0],
            BranchRow::Default {
                existing: Some(_),
                ..
            }
        ));

        let rows = branch_rows("feat-1".into(), vec![s("x", false, true, None)]);
        let labels: Vec<String> = rows.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            labels,
            vec!["feat-1  new", "x  origin", "type another name"]
        );
        assert!(matches!(
            &rows[0],
            BranchRow::Default { existing: None, .. }
        ));
    }

    #[test]
    fn branch_row_branch_is_none_only_for_other() {
        let rows = branch_rows("d".into(), vec![s("x", true, false, None)]);
        assert_eq!(rows[0].branch(), Some("d"));
        assert_eq!(rows[1].branch(), Some("x"));
        assert_eq!(rows[2].branch(), None);
        assert_eq!(rows[2].to_string(), "type another name");
    }
}
