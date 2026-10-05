//! `hub feature start-review URL`: a review feature for one role from a
//! GitLab merge request or GitHub pull request. Every safety check lives
//! here; the bundled skill is not the safety boundary.

use std::path::PathBuf;

use crate::error::{HubError, Result};
use crate::feature::{Change, Feature, Owner, Stage, Worktree};
use crate::git::{Git, same_path};
use crate::hub::Hub;
use crate::manifest::RepoSpec;
use crate::names;
use crate::ops::resolve::{Created, copy_local_config};
use crate::ops::session::{after_save, converge_tmux, write_workspace};
use crate::ops::{start, status};
use crate::review::{self, ReviewInfo};
use crate::skill;

pub struct StartReviewOptions {
    pub url: String,
    /// Feature and hub branch name; `<role>-review-<number>` when `None`.
    pub name: Option<String>,
    pub checkout: Option<String>,
}

/// How the member's local copy of the source branch relates to the review head.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LocalSource {
    Absent,
    AtHead,
    /// Strictly behind: carries the current local tip.
    Behind(String),
}

/// Everything the member checks established, pinned for the mutation step.
struct MemberPlan {
    clone_dir: PathBuf,
    worktree_name: String,
    path: PathBuf,
    local: LocalSource,
    base_sha: String,
}

pub fn start_review(hub: &Hub, opts: &StartReviewOptions) -> Result<Vec<String>> {
    let url = review::parse_review_url(&opts.url)?;
    let repo = review::match_role(&hub.manifest, &url)?.clone();
    require_no_drift(hub)?;
    let info = review::fetch_review(&url)?;
    let name = opts
        .name
        .clone()
        .unwrap_or_else(|| format!("{}-review-{}", repo.role, url.number));
    let checkout = start::preflight(hub, &name, opts.checkout.as_deref())?;
    check_hub(hub, &name, &checkout)?;
    let member = check_member(hub, &repo, &info)?;
    // The remote may have moved while the checks ran; nothing is created
    // from metadata that no longer holds.
    let again = review::fetch_review(&url)?;
    if again != info {
        return Err(HubError::Precondition(format!(
            "{} changed while checking (source, target, state, or head); rerun to start from its current state",
            url.canonical()
        )));
    }

    // Transaction: everything up to and including the record write is
    // rolled back on failure. After the write, workspace and tmux failures
    // are warnings (see `after_save`), never a rollback.
    let mut created = Created::new();
    let (feature, mut lines) =
        match create(hub, &name, &checkout, &repo, &info, &member, &mut created) {
            Ok(done) => done,
            Err(err) => {
                let mut warnings = created.rollback();
                if warnings.is_empty() {
                    return Err(err);
                }
                warnings.insert(0, err.to_string());
                return Err(HubError::Precondition(warnings.join("\n")));
            }
        };
    after_save(
        &mut lines,
        write_workspace(hub, &feature).map(|_| Vec::new()),
        &format!("hub sync --feature {}", feature.name),
    );
    after_save(
        &mut lines,
        converge_tmux(hub, &feature),
        &format!("hub tmux --feature {}", feature.name),
    );
    if let Err(err) = skill::install(&hub.hub_worktree_path(&feature.checkout)) {
        lines.push(format!(
            "warning: {err}; any hub command retries the skill install"
        ));
    }
    Ok(lines)
}

/// Structural drift anywhere the command would rely on stops it before any
/// provider call: the base checkouts always, the invoking feature when
/// run from one of its worktrees.
fn require_no_drift(hub: &Hub) -> Result<()> {
    if status::base_status(hub)?.drift {
        return Err(HubError::Drift);
    }
    if let Some(name) = &hub.cwd_feature {
        let feature = hub.load_feature(name)?;
        if status::status(hub, &feature)?.drift {
            return Err(HubError::Drift);
        }
    }
    Ok(())
}

/// The coordinator branch and session must not exist anywhere: this
/// command never adopts.
fn check_hub(hub: &Hub, name: &str, checkout: &str) -> Result<()> {
    let git = hub.git();
    if git.has_remotes()? {
        git.fetch("origin").map_err(|e| {
            HubError::Precondition(format!(
                "fetching the hub's origin failed: {e}\nstart-review needs current refs to check for branch {name}"
            ))
        })?;
        if git.remote_branch_exists(name)? {
            return Err(HubError::Precondition(format!(
                "hub branch {name} already exists on origin; pass --name to choose another"
            )));
        }
    }
    if git.local_branch_exists(name)? {
        return Err(HubError::Precondition(format!(
            "hub branch {name} already exists; pass --name to choose another"
        )));
    }
    if let Some(tmux) = &hub.tmux
        && tmux.has_session(checkout)?
    {
        return Err(HubError::Precondition(format!(
            "tmux session {checkout} already exists; pass --checkout to choose another"
        )));
    }
    Ok(())
}

/// Fetch the member and pin what the mutation step will rely on. Refuses
/// anything that would need a reset, stash, or adoption.
fn check_member(hub: &Hub, repo: &RepoSpec, info: &ReviewInfo) -> Result<MemberPlan> {
    let clone_dir = hub.env.clone_path(&repo.clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "clone for role '{}' is missing: {}",
            repo.role,
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
    git.fetch("origin").map_err(|e| {
        HubError::Precondition(format!(
            "fetching origin in {} failed: {e}\nstart-review needs the review's current refs",
            clone_dir.display()
        ))
    })?;
    let source = &info.source_branch;
    let source_ref = format!("origin/{source}");
    let target_ref = format!("origin/{}", info.target_branch);
    let head = git.rev_parse(&source_ref).map_err(|_| {
        HubError::Precondition(format!(
            "{source_ref} not found in {}; the review's source branch is gone or was not fetched",
            clone_dir.display()
        ))
    })?;
    if head != info.head_sha {
        return Err(HubError::Precondition(format!(
            "{source_ref} is at {head} but the review reports {}; the fetch is stale or the branch moved, rerun",
            info.head_sha
        )));
    }
    if git.rev_parse(&target_ref).is_err() {
        return Err(HubError::Precondition(format!(
            "{target_ref} not found in {}; the review's target branch must exist on origin",
            clone_dir.display()
        )));
    }
    let base_sha = git.merge_base(&head, &target_ref)?.ok_or_else(|| {
        HubError::Precondition(format!(
            "{source_ref} and {target_ref} share no history; cannot compare the review with its target"
        ))
    })?;

    if let Some(w) = git
        .worktrees()?
        .into_iter()
        .find(|w| w.branch.as_deref() == Some(source.as_str()))
    {
        let path = w.path.canonicalize().unwrap_or(w.path);
        let place = if same_path(&path, &clone_dir) {
            "the main clone".to_string()
        } else {
            "a worktree".to_string()
        };
        return Err(HubError::Precondition(format!(
            "{source} is checked out in {place} at {}; start-review never adopts a checkout. \
             Finish or move that work first",
            path.display()
        )));
    }
    let local = if git.local_branch_exists(source)? {
        let tip = git.rev_parse(&format!("refs/heads/{source}"))?;
        if tip == head {
            LocalSource::AtHead
        } else if git.is_ancestor(&tip, &head)? {
            LocalSource::Behind(tip)
        } else {
            return Err(HubError::Precondition(format!(
                "branch {source} in {} is ahead of or has diverged from {source_ref} (local {tip}); \
                 reconcile it first, start-review never resets a branch",
                clone_dir.display()
            )));
        }
    } else {
        LocalSource::Absent
    };
    let worktree_name = names::flatten_branch(source);
    names::validate_worktree_name(&worktree_name)?;
    let path = hub.env.worktree_path(&repo.clone, &worktree_name);
    if path.exists() {
        return Err(HubError::Precondition(format!(
            "{} already exists; remove it or review from another hub",
            path.display()
        )));
    }
    Ok(MemberPlan {
        clone_dir,
        worktree_name,
        path,
        local,
        base_sha,
    })
}

/// Git work and the one record write. Returns the saved feature and the
/// lines describing what was created.
fn create(
    hub: &Hub,
    name: &str,
    checkout: &str,
    repo: &RepoSpec,
    info: &ReviewInfo,
    member: &MemberPlan,
    created: &mut Created,
) -> Result<(Feature, Vec<String>)> {
    let source = &info.source_branch;
    let mut lines = vec![
        format!("Review feature {name} for {}", repo.role),
        format!("  {}", info.url.canonical()),
        format!("  source {source} at {}", info.head_sha),
        format!("  target {}", info.target_branch),
    ];
    let git = hub.git();
    let main_tip = git.rev_parse("main")?;
    git.create_branch(name, "main")?;
    created
        .conditional_branches
        .push((hub.root.clone(), name.to_string(), main_tip));
    let hub_wt = hub.hub_worktree_path(checkout);
    git.worktree_add(&hub_wt, name)?;
    created.worktrees.push((hub.root.clone(), hub_wt.clone()));
    lines.push(format!(
        "Hub worktree: {} (branch {name})",
        hub_wt.display()
    ));

    let mgit = Git::new(&member.clone_dir);
    let mut notes = Vec::new();
    match &member.local {
        LocalSource::Absent => {
            mgit.create_tracking_branch(source)?;
            // The branch starts at whatever origin/<source> is now, which a
            // concurrent fetch may have moved since the check: rollback has to
            // expect the tip that was really created, not the reviewed head.
            let tip = mgit.rev_parse(source)?;
            created.conditional_branches.push((
                member.clone_dir.clone(),
                source.clone(),
                tip.clone(),
            ));
            if tip != info.head_sha {
                return Err(HubError::Precondition(format!(
                    "origin/{source} moved during setup; rerun"
                )));
            }
            notes.push(format!("Created branch {source} tracking origin/{source}"));
        }
        LocalSource::Behind(old) => {
            mgit.update_ref_if(&format!("refs/heads/{source}"), &info.head_sha, old)?;
            created.fast_forwards.push((
                member.clone_dir.clone(),
                source.clone(),
                old.clone(),
                info.head_sha.clone(),
            ));
            notes.push(format!("Fast-forwarded {source} to {}", info.head_sha));
        }
        LocalSource::AtHead => notes.push(format!("Reusing branch {source}")),
    }
    // `git worktree add` itself refuses a branch another worktree took meanwhile.
    mgit.worktree_add(&member.path, source)?;
    created
        .worktrees
        .push((member.clone_dir.clone(), member.path.clone()));
    notes.push(format!("Created worktree {}", member.path.display()));
    notes.extend(copy_local_config(&mgit, &member.clone_dir, &member.path)?);
    let wt = Git::new(&member.path);
    if wt.current_branch()?.as_deref() != Some(source.as_str())
        || wt.rev_parse("HEAD")? != info.head_sha
    {
        return Err(HubError::Precondition(format!(
            "{} is not on {source} at {} after setup",
            member.path.display(),
            info.head_sha
        )));
    }
    lines.push(format!(
        "{}: {source} at {}",
        repo.role,
        member.path.display()
    ));
    lines.extend(notes.into_iter().map(|l| format!("  {l}")));

    let mut feature = Feature::new(name, checkout);
    feature.push_change(Change {
        role: repo.role.clone(),
        clone: repo.clone.clone(),
        branch: source.clone(),
        worktree: Some(Worktree {
            name: member.worktree_name.clone(),
            owner: Owner::Hub,
        }),
        stage: Stage::Review,
        review_url: Some(info.url.canonical()),
        merged_at: None,
        origin_seen: true,
        base: Some(info.target_branch.clone()),
        base_sha: Some(member.base_sha.clone()),
    })?;
    hub.save_feature(&feature)?;
    Ok((feature, lines))
}
