use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Owner, Stage, Worktree};
use crate::git::{Git, same_path};
use crate::hub::Hub;
use crate::manifest::RepoSpec;
use crate::names;

const LOCAL_CONFIG_FILES: [&str; 2] = [".vscode/settings.json", ".env"];

/// Everything one invocation created, so a later failure can undo exactly that.
#[derive(Debug, Default)]
pub struct Created {
    /// `(clone directory, branch name)`: deleted outright on rollback.
    pub branches: Vec<(PathBuf, String)>,
    /// `(clone directory, branch name, tip at creation)`: deleted on
    /// rollback only while the branch still points at that tip and no
    /// worktree has it checked out.
    pub conditional_branches: Vec<(PathBuf, String, String)>,
    /// `(clone directory, branch name, old tip, new tip)`: a pre-existing
    /// branch this invocation fast-forwarded; put back on rollback only if
    /// it still points at the new tip and no worktree has it checked out.
    pub fast_forwards: Vec<(PathBuf, String, String, String)>,
    /// `(clone directory, worktree path)`
    pub worktrees: Vec<(PathBuf, PathBuf)>,
}

impl Created {
    pub fn new() -> Created {
        Created::default()
    }

    /// Undo in reverse order: worktrees, fast-forwards, conditionally
    /// created branches, then plain branches. Every ref move is
    /// conditional on the ref still being where this invocation left it,
    /// and a branch some worktree took meanwhile is never moved. Returns
    /// warnings for anything left as it is.
    pub fn rollback(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        for (clone, path) in self.worktrees.iter().rev() {
            if let Err(e) = Git::new(clone).worktree_remove(path, true) {
                warnings.push(format!(
                    "rollback: could not remove {}: {e}",
                    path.display()
                ));
            }
        }
        for (clone, branch, old, new) in self.fast_forwards.iter().rev() {
            let git = Git::new(clone);
            let occupied = git.worktrees().ok().and_then(|list| {
                list.into_iter()
                    .find(|w| w.branch.as_deref() == Some(branch.as_str()))
            });
            if let Some(w) = occupied {
                warnings.push(format!(
                    "rollback: {branch} in {} is now checked out at {}; left at {new} (was {old})",
                    clone.display(),
                    w.path.display()
                ));
                continue;
            }
            if let Err(e) = git.update_ref_if(&format!("refs/heads/{branch}"), old, new) {
                warnings.push(format!(
                    "rollback: {branch} in {} moved since the fast-forward; left at its current tip (was {old}): {e}",
                    clone.display()
                ));
            }
        }
        for (clone, branch, tip) in self.conditional_branches.iter().rev() {
            let git = Git::new(clone);
            let occupied = git.worktrees().ok().and_then(|list| {
                list.into_iter()
                    .find(|w| w.branch.as_deref() == Some(branch.as_str()))
            });
            if let Some(w) = occupied {
                warnings.push(format!(
                    "rollback: branch {branch} in {} is now checked out at {}; kept",
                    clone.display(),
                    w.path.display()
                ));
                continue;
            }
            if let Err(e) = git.delete_ref_if(&format!("refs/heads/{branch}"), tip) {
                warnings.push(format!(
                    "rollback: branch {branch} in {} moved since it was created; kept: {e}",
                    clone.display()
                ));
            }
        }
        for (clone, branch) in self.branches.iter().rev() {
            if let Err(e) = Git::new(clone).delete_branch(branch) {
                warnings.push(format!(
                    "rollback: could not delete branch {branch} in {}: {e}",
                    clone.display()
                ));
            }
        }
        warnings
    }
}

pub struct AddOptions<'a> {
    pub role: &'a str,
    pub branch: Option<&'a str>,
    pub from: Option<&'a str>,
    pub worktree: Option<&'a str>,
    /// Branch the change merges into, without `origin/`; the role's
    /// manifest base when `None`.
    pub base: Option<&'a str>,
}

#[derive(Debug)]
pub struct Opened {
    pub change: Change,
    pub path: PathBuf,
    pub lines: Vec<String>,
}

/// Resolve the branch and worktree for `role` in `feature` (spec §3). Records
/// anything it creates in `created`; the caller rolls back on later failure.
pub fn open_change(
    hub: &Hub,
    feature: &Feature,
    opts: &AddOptions,
    created: &mut Created,
) -> Result<Opened> {
    let repo = hub.manifest.require_repo(opts.role)?;
    if let Some(existing) = feature.open_change(opts.role) {
        return Err(HubError::Precondition(format!(
            "role '{}' already has an open change on branch '{}'",
            opts.role, existing.branch
        )));
    }
    if let Some(name) = opts.worktree {
        names::validate_worktree_name(name)?;
    }
    let clone_dir = hub.env.clone_path(&repo.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{}' is missing: {}",
            opts.role,
            clone_dir.display()
        )));
    }
    let origin = git.remote_url("origin")?.unwrap_or_default();
    if origin != repo.remote {
        return Err(HubError::Precondition(format!(
            "origin of {} is '{origin}', manifest says '{}'; \
             re-register the role with hub repo remove/add, or edit its remote in hub.json \
             (0.16.0 records the configured URL, not an insteadOf rewrite)",
            clone_dir.display(),
            repo.remote
        )));
    }
    let base = match opts.base {
        Some(b) => {
            if b.starts_with("origin/") || !git.check_ref_format(b)? {
                return Err(HubError::Usage(format!(
                    "--base '{b}' must be a branch name without origin/"
                )));
            }
            b.to_string()
        }
        None => repo.base.clone(),
    };
    git.fetch("origin")?;
    let base_ref = format!("origin/{base}");
    let base_tip = git.rev_parse(&base_ref).map_err(|_| {
        HubError::Precondition(if opts.base.is_some() {
            format!(
                "{base_ref} not found in {}; --base needs a branch that exists on origin",
                clone_dir.display()
            )
        } else {
            format!(
                "{base_ref} not found in {}; check base for role '{}' in hub.json",
                clone_dir.display(),
                opts.role
            )
        })
    })?;

    let branch = match opts.branch {
        Some(b) => b.to_string(),
        None => default_branch(feature, repo, &git)?,
    };
    if !git.check_ref_format(&branch)? {
        return Err(HubError::Usage(format!(
            "'{branch}' is not a valid branch name"
        )));
    }

    let mut lines = Vec::new();
    let local = git.local_branch_exists(&branch)?;
    let remote = git.remote_branch_exists(&branch)?;
    let checked_out = git
        .worktrees()?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(branch.as_str()))
        .map(|w| w.path);

    match (local, remote) {
        (true, true) => {
            let remote_ref = format!("origin/{branch}");
            if git.rev_parse(&branch)? != git.rev_parse(&remote_ref)? {
                let behind = git.is_ancestor(&branch, &remote_ref)?;
                let ahead = git.is_ancestor(&remote_ref, &branch)?;
                if behind && checked_out.is_none() {
                    git.force_branch(&branch, &remote_ref)?;
                    lines.push(format!("Fast-forwarded {branch} to {remote_ref}"));
                } else if !behind && !ahead {
                    return Err(HubError::Precondition(format!(
                        "branch {branch} in {} has diverged from {remote_ref}; reconcile it first",
                        clone_dir.display()
                    )));
                }
            }
            lines.push(format!("Reusing branch {branch}"));
        }
        (true, false) => lines.push(format!("Reusing branch {branch}")),
        (false, true) => {
            git.create_tracking_branch(&branch)?;
            created.branches.push((clone_dir.clone(), branch.clone()));
            lines.push(format!("Created branch {branch} tracking origin/{branch}"));
        }
        (false, false) => {
            let start = opts
                .from
                .map(str::to_string)
                .unwrap_or_else(|| base_ref.clone());
            git.create_branch(&branch, &start)?;
            created.branches.push((clone_dir.clone(), branch.clone()));
            lines.push(format!("Created branch {branch} from {start}"));
        }
    }

    // The fork point: a branch created from --from, or reused with work of
    // its own, records where it actually left the base. A fresh branch from
    // origin/<base> has the base tip as its merge base. Unrelated histories
    // fall back to the tip so an adopted branch is never refused here.
    let base_sha = git.merge_base(&branch, &base_ref)?.unwrap_or(base_tip);

    let (worktree, path) = match checked_out {
        Some(existing) => {
            let existing = existing.canonicalize().unwrap_or(existing);
            if opts.worktree.is_some() {
                return Err(HubError::Usage(format!(
                    "--worktree given but {branch} is already checked out at {}",
                    existing.display()
                )));
            }
            if same_path(&existing, &clone_dir) {
                lines.push(format!(
                    "Using main clone at {} ({branch} is checked out there)",
                    clone_dir.display()
                ));
                (None, clone_dir.clone())
            } else if let Some(name) = hub.env.worktree_name(&repo.clone, &existing) {
                lines.push(format!("Adopting worktree {}", existing.display()));
                (
                    Some(Worktree {
                        name,
                        owner: Owner::Adopted,
                    }),
                    existing,
                )
            } else {
                return Err(HubError::Precondition(format!(
                    "{branch} is checked out at {}, outside <worktree_dir>/{}; move or remove it first",
                    existing.display(),
                    repo.clone
                )));
            }
        }
        None => {
            let name = opts
                .worktree
                .map(str::to_string)
                .unwrap_or_else(|| names::flatten_branch(&branch));
            names::validate_worktree_name(&name)?;
            let path = hub.env.worktree_path(&repo.clone, &name);
            if path.exists() {
                return Err(HubError::Precondition(format!(
                    "{} already exists; pass --worktree <name> to use another directory",
                    path.display()
                )));
            }
            git.worktree_add(&path, &branch)?;
            created.worktrees.push((clone_dir.clone(), path.clone()));
            lines.push(format!("Created worktree {}", path.display()));
            lines.extend(copy_local_config(&git, &clone_dir, &path)?);
            (
                Some(Worktree {
                    name,
                    owner: Owner::Hub,
                }),
                path,
            )
        }
    };

    Ok(Opened {
        change: Change {
            role: opts.role.to_string(),
            clone: repo.clone.clone(),
            branch,
            worktree,
            stage: Stage::Working,
            review_url: None,
            merged_at: None,
            origin_seen: remote,
            base: Some(base),
            base_sha: Some(base_sha),
        },
        path,
        lines,
    })
}

/// Template branch for a first change; `-<n>` follow-up after a merged one.
pub(crate) fn default_branch(feature: &Feature, repo: &RepoSpec, git: &Git) -> Result<String> {
    match feature.last_change(&repo.role) {
        Some(last) => {
            let earlier = feature.branches_for(&repo.role);
            // `next_branch` only stops when `exists` returns false, so a git
            // failure must not be reported as "does not exist" or the loop
            // never terminates. Capture the first error and stop the loop
            // instead, then propagate it once `next_branch` returns.
            let error: RefCell<Option<HubError>> = RefCell::new(None);
            let exists = |candidate: &str| {
                if error.borrow().is_some() {
                    return false;
                }
                match (
                    git.local_branch_exists(candidate),
                    git.remote_branch_exists(candidate),
                ) {
                    (Ok(local), Ok(remote)) => local || remote,
                    (Err(e), _) | (_, Err(e)) => {
                        *error.borrow_mut() = Some(e);
                        false
                    }
                }
            };
            let branch = names::next_branch(&last.branch, &earlier, exists);
            match error.into_inner() {
                Some(e) => Err(e),
                None => Ok(branch),
            }
        }
        None => Ok(names::branch_name(&repo.branch_template, &feature.name)),
    }
}

/// Copy `.vscode/settings.json` and `.env` from the main clone into a new
/// worktree, but only when the clone ignores them: an unignored copy would
/// make the worktree dirty from its first second. One note per skipped file.
pub(crate) fn copy_local_config(git: &Git, from: &Path, to: &Path) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    for rel in LOCAL_CONFIG_FILES {
        let src = from.join(rel);
        let dst = to.join(rel);
        if !src.is_file() || dst.exists() {
            continue;
        }
        if !git.is_ignored(rel)? {
            notes.push(format!(
                "skipped {rel}: not ignored in {}; copying it would dirty the worktree",
                from.display()
            ));
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| HubError::io(format!("creating {}", parent.display()), e))?;
        }
        std::fs::copy(&src, &dst).map_err(|e| {
            HubError::io(format!("copying {} to {}", src.display(), dst.display()), e)
        })?;
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A persistently failing git call must surface as an error, not loop
    /// forever inside `names::next_branch`.
    #[test]
    fn default_branch_propagates_a_git_error_instead_of_looping_forever() {
        let mut feature = Feature::new("feat-1", "feat-1");
        feature.changes.push(Change {
            role: "api".into(),
            clone: "api-clone".into(),
            branch: "feat-1".into(),
            worktree: None,
            stage: Stage::Merged,
            review_url: None,
            merged_at: None,
            origin_seen: false,
            base: None,
            base_sha: None,
        });
        let repo = RepoSpec {
            role: "api".into(),
            clone: "api-clone".into(),
            remote: "git@example.com:x.git".into(),
            base: "master".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        };
        let git = Git::new(Path::new("/nonexistent/definitely-not-a-repo"));
        let err = default_branch(&feature, &repo, &git).unwrap_err();
        assert!(matches!(err, HubError::Io { .. }));
    }

    #[test]
    fn rollback_restores_a_fast_forward_only_while_the_ref_is_unchanged_and_free() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        Git::init(&dir, "main").unwrap();
        let git = Git::new(&dir);
        git.config("user.name", "Test").unwrap();
        git.config("user.email", "test@example.com").unwrap();
        let commit = |name: &str| {
            std::fs::write(dir.join(name), name).unwrap();
            git.add(&[Path::new(name)]).unwrap();
            git.commit(name).unwrap();
            git.rev_parse("HEAD").unwrap()
        };
        let old = commit("a");
        let new = commit("b");
        let third = commit("c");
        git.create_branch("feat", &old).unwrap();
        git.force_branch("feat", &new).unwrap();

        let mut created = Created::new();
        created
            .fast_forwards
            .push((dir.clone(), "feat".into(), old.clone(), new.clone()));
        assert!(created.rollback().is_empty());
        assert_eq!(git.rev_parse("feat").unwrap(), old, "restored");

        git.force_branch("feat", &third).unwrap();
        let warnings = created.rollback();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("feat") && warnings[0].contains("left at"),
            "{warnings:?}"
        );
        assert_eq!(
            git.rev_parse("feat").unwrap(),
            third,
            "concurrent work kept"
        );

        git.force_branch("feat", &new).unwrap();
        let wt = dir.parent().unwrap().join("wt");
        git.worktree_add(&wt, "feat").unwrap();
        let warnings = created.rollback();
        assert!(warnings[0].contains("checked out"), "{warnings:?}");
        assert_eq!(
            git.rev_parse("feat").unwrap(),
            new,
            "an occupied branch is never moved"
        );

        let mut created = Created::new();
        git.create_branch("made", &old).unwrap();
        created
            .conditional_branches
            .push((dir.clone(), "made".into(), old.clone()));
        git.force_branch("made", &new).unwrap();
        assert_eq!(created.rollback().len(), 1, "moved since creation: kept");
        assert!(git.local_branch_exists("made").unwrap());
        git.force_branch("made", &old).unwrap();

        // A conditionally created branch some worktree still holds is kept:
        // a worktree removal that failed earlier in the rollback must not
        // leave `update-ref -d` deleting the ref out from under it.
        let taken = dir.parent().unwrap().join("taken");
        git.create_branch("held", &old).unwrap();
        git.worktree_add(&taken, "held").unwrap();
        created
            .conditional_branches
            .push((dir.clone(), "held".into(), old.clone()));
        let warnings = created.rollback();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("held") && warnings[0].contains("checked out"),
            "{warnings:?}"
        );
        assert!(
            git.local_branch_exists("held").unwrap(),
            "an occupied branch is never deleted"
        );
        assert!(!git.local_branch_exists("made").unwrap());
    }
}
