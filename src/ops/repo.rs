use crate::error::{HubError, Result};
use crate::feature::Feature;
use crate::git::Git;
use crate::hub::Hub;
use crate::manifest::{Manifest, RepoSpec};
use crate::names;
use crate::ops::session::{after_save, write_workspace};
use crate::ops::table;

pub struct AddRepoOptions {
    pub role: String,
    pub clone: String,
    pub base: Option<String>,
    pub branch_template: Option<String>,
    pub description: Option<String>,
}

/// The clone directory must be a git repository with an `origin`; returns
/// its `Git` and the origin URL.
fn inspect_clone(hub: &Hub, clone: &str) -> Result<(Git, String)> {
    names::validate_clone(clone)?;
    let clone_dir = hub.env.clone_path(clone);
    let git = Git::new(&clone_dir);
    if !git.is_repo() {
        return Err(HubError::Precondition(format!(
            "{} is not a git repository",
            clone_dir.display()
        )));
    }
    let remote = git.remote_url("origin")?.ok_or_else(|| {
        HubError::Precondition(format!("{} has no 'origin' remote", clone_dir.display()))
    })?;
    Ok((git, remote))
}

fn check_template(template: &str) -> Result<()> {
    if !template.contains("{feature}") {
        return Err(HubError::Usage(format!(
            "--branch-template '{template}' must contain {{feature}}"
        )));
    }
    Ok(())
}

/// No role other than `except_role` registers `clone`.
fn check_clone_free(manifest: &Manifest, clone: &str, except_role: Option<&str>) -> Result<()> {
    if let Some(existing) = manifest
        .repos
        .iter()
        .find(|r| r.clone == clone && Some(r.role.as_str()) != except_role)
    {
        return Err(HubError::Precondition(format!(
            "clone '{clone}' is already registered as role '{}'",
            existing.role
        )));
    }
    Ok(())
}

pub fn add(hub: &Hub, opts: &AddRepoOptions) -> Result<Vec<String>> {
    names::validate_role(&opts.role)?;
    names::validate_clone(&opts.clone)?;
    if hub.manifest.repo(&opts.role).is_some() {
        return Err(HubError::Precondition(format!(
            "duplicate role '{}'",
            opts.role
        )));
    }
    check_clone_free(&hub.manifest, &opts.clone, None)?;
    let branch_template = opts
        .branch_template
        .clone()
        .unwrap_or_else(|| names::DEFAULT_BRANCH_TEMPLATE.to_string());
    check_template(&branch_template)?;
    let clone_dir = hub.env.clone_path(&opts.clone);
    let (git, remote) = inspect_clone(hub, &opts.clone)?;
    let base = match &opts.base {
        Some(base) => base.clone(),
        None => git.default_remote_branch()?.ok_or_else(|| {
            HubError::Precondition(format!(
                "cannot infer the base branch of {} (no origin/HEAD); pass --base",
                clone_dir.display()
            ))
        })?,
    };
    let mut manifest = hub.manifest.clone();
    manifest.repos.push(RepoSpec {
        role: opts.role.clone(),
        clone: opts.clone.clone(),
        remote,
        base: base.clone(),
        branch_template,
        description: opts.description.clone().unwrap_or_default(),
    });
    hub.save_manifest(&manifest, &format!("repo add {}", opts.role))?;
    Ok(vec![format!(
        "Registered role '{}' -> {} (base {base})",
        opts.role,
        clone_dir.display()
    )])
}

pub struct SetRepoOptions {
    pub role: String,
    pub clone: Option<String>,
    pub base: Option<String>,
    pub branch_template: Option<String>,
    pub description: Option<String>,
}

/// Change some of a role's fields in place, keeping its position; one commit.
pub fn set(hub: &Hub, opts: &SetRepoOptions) -> Result<Vec<String>> {
    if opts.clone.is_none()
        && opts.base.is_none()
        && opts.branch_template.is_none()
        && opts.description.is_none()
    {
        return Err(HubError::Usage(
            "repo set needs at least one of --clone, --base, --branch-template, --description"
                .into(),
        ));
    }
    hub.manifest.require_repo(&opts.role)?;
    let index = hub
        .manifest
        .repos
        .iter()
        .position(|r| r.role == opts.role)
        .expect("required above");
    let mut repo = hub.manifest.repos[index].clone();
    let mut changed = Vec::new();
    let mut note = |field: &str, old: &str, new: &str| {
        if old != new {
            changed.push(format!("  {field}: {old:?} -> {new:?}"));
        }
    };
    if let Some(template) = &opts.branch_template {
        check_template(template)?;
        note("branch_template", &repo.branch_template, template);
        repo.branch_template = template.clone();
    }
    if let Some(base) = &opts.base {
        if base.is_empty() || base.starts_with("origin/") {
            return Err(HubError::Usage(format!(
                "--base '{base}' must be a branch name without origin/"
            )));
        }
        note("base", &repo.base, base);
        repo.base = base.clone();
    }
    if let Some(description) = &opts.description {
        note("description", &repo.description, description);
        repo.description = description.clone();
    }
    if let Some(clone) = opts.clone.as_ref().filter(|c| **c != repo.clone) {
        names::validate_clone(clone)?;
        check_clone_free(&hub.manifest, clone, Some(&opts.role))?;
        for feature in hub.list_features()? {
            if feature.is_open() && feature.open_change(&opts.role).is_some() {
                return Err(HubError::Precondition(format!(
                    "feature '{}' has an open change for role '{}' in clone '{}'; \
                     merge or finish it before changing the clone",
                    feature.name, opts.role, repo.clone
                )));
            }
        }
        let (_, remote) = inspect_clone(hub, clone)?;
        note("clone", &repo.clone, clone);
        note("remote", &repo.remote, &remote);
        repo.clone = clone.clone();
        repo.remote = remote;
    }
    if changed.is_empty() {
        return Ok(vec![format!(
            "Role '{}' already has those values; nothing to commit",
            opts.role
        )]);
    }
    let mut manifest = hub.manifest.clone();
    manifest.repos[index] = repo;
    hub.save_manifest(&manifest, &format!("repo set {}", opts.role))?;
    let mut lines = vec![format!("Updated role '{}':", opts.role)];
    lines.extend(changed);
    Ok(lines)
}

pub fn remove(hub: &Hub, role: &str) -> Result<Vec<String>> {
    hub.manifest.require_repo(role)?;
    for feature in hub.list_features()? {
        if feature.is_open() && feature.open_change(role).is_some() {
            return Err(HubError::Precondition(format!(
                "feature '{}' has an open change for role '{role}'; merge or finish it first",
                feature.name
            )));
        }
    }
    let mut manifest = hub.manifest.clone();
    manifest.repos.retain(|r| r.role != role);
    hub.save_manifest(&manifest, &format!("repo remove {role}"))?;
    Ok(vec![format!(
        "Unregistered role '{role}' (its clone and branches are untouched)"
    )])
}

pub fn list_table(hub: &Hub) -> Vec<String> {
    let rows: Vec<Vec<String>> = hub
        .manifest
        .repos
        .iter()
        .map(|r| {
            vec![
                r.role.clone(),
                r.clone.clone(),
                r.base.clone(),
                r.branch_template.clone(),
                r.description.clone(),
            ]
        })
        .collect();
    table(
        &["ROLE", "CLONE", "BASE", "BRANCH TEMPLATE", "DESCRIPTION"],
        &rows,
    )
}

pub fn list_json(hub: &Hub) -> Result<String> {
    serde_json::to_string_pretty(&hub.manifest.repos)
        .map_err(|e| HubError::Precondition(format!("serializing repos: {e}")))
}

pub struct RenameOptions {
    pub old: String,
    pub new: String,
    pub records_only: bool,
    pub clone: Option<String>,
}

/// Rename a role in `hub.json` (keeping its position) and in every local
/// feature record; with `records_only`, only the records.
pub fn rename(hub: &Hub, opts: &RenameOptions) -> Result<Vec<String>> {
    let (old, new) = (opts.old.as_str(), opts.new.as_str());
    if opts.clone.is_some() && !opts.records_only {
        return Err(HubError::Usage(
            "--clone only applies with --records-only".into(),
        ));
    }
    names::validate_role(new)?;
    let features = hub.list_features()?;
    if opts.records_only {
        return rename_records_only(hub, opts, features);
    }
    hub.manifest.require_repo(old)?;
    if hub.manifest.repo(new).is_some() {
        return Err(HubError::Precondition(format!(
            "role '{new}' is already registered"
        )));
    }
    if let Some(f) = features
        .iter()
        .find(|f| f.changes.iter().any(|c| c.role == new))
    {
        return Err(HubError::Precondition(format!(
            "feature '{}' already records changes for role '{new}'; pick another name",
            f.name
        )));
    }
    let (rewritten, count) = rewrite_records(features, old, new, None);
    let mut manifest = hub.manifest.clone();
    manifest
        .repos
        .iter_mut()
        .find(|r| r.role == old)
        .expect("required above")
        .role = new.to_string();
    hub.save_manifest(&manifest, &format!("repo rename {old} {new}"))?;
    // Workspace order comes from the manifest; use the renamed one.
    let mut renamed = hub.clone();
    renamed.manifest = manifest;
    let mut lines = vec![format!("Renamed role '{old}' to '{new}' in hub.json")];
    let recovery = format!(
        "renamed in hub.json; finish the records with: hub repo rename {old} {new} --records-only"
    );
    apply_rename(&renamed, &rewritten, old, new, Some(&recovery), &mut lines)?;
    lines.push(summary_line(old, count, rewritten.len()));
    Ok(lines)
}

/// Catch this machine's records up with a rename already in `hub.json`, or
/// repair records left by an earlier remove and add. Never commits.
fn rename_records_only(
    hub: &Hub,
    opts: &RenameOptions,
    features: Vec<Feature>,
) -> Result<Vec<String>> {
    let (old, new) = (opts.old.as_str(), opts.new.as_str());
    let clone = opts.clone.as_deref();
    if old == new {
        return Err(HubError::Usage(format!(
            "OLD and NEW are the same role '{old}'"
        )));
    }
    names::validate_role(old)?;
    if let Some(clone) = clone {
        names::validate_clone(clone)?;
    }
    if hub.manifest.repo(new).is_none() {
        return Err(HubError::Precondition(format!(
            "role '{new}' is not in hub.json; --records-only catches records up with a rename already there"
        )));
    }
    for f in &features {
        let renames_an_open_change = f
            .changes
            .iter()
            .any(|c| c.is_open() && c.role == old && clone.is_none_or(|k| c.clone == k));
        if renames_an_open_change && f.open_change(new).is_some() {
            return Err(HubError::Precondition(format!(
                "feature '{}' already has an open change for role '{new}'; renaming its open '{old}' change would give it two",
                f.name
            )));
        }
    }
    let (rewritten, count) = rewrite_records(features, old, new, clone);
    let mut lines = Vec::new();
    apply_rename(hub, &rewritten, old, new, None, &mut lines)?;
    lines.push(summary_line(old, count, rewritten.len()));
    Ok(lines)
}

/// The features with at least one change for `old` (and `clone`, when
/// given), with those changes renamed, and how many changes that was.
fn rewrite_records(
    features: Vec<Feature>,
    old: &str,
    new: &str,
    clone: Option<&str>,
) -> (Vec<Feature>, usize) {
    let mut count = 0;
    let rewritten = features
        .into_iter()
        .filter_map(|mut f| {
            let mut touched = false;
            for c in f
                .changes
                .iter_mut()
                .filter(|c| c.role == old && clone.is_none_or(|k| c.clone == k))
            {
                c.role = new.to_string();
                touched = true;
                count += 1;
            }
            touched.then_some(f)
        })
        .collect();
    (rewritten, count)
}

/// Save the rewritten records, then, for each open feature now using
/// `new`, rename its tmux window and regenerate its workspace file. The
/// environment steps only warn. `recovery` is appended to a record-write
/// error.
fn apply_rename(
    hub: &Hub,
    rewritten: &[Feature],
    old: &str,
    new: &str,
    recovery: Option<&str>,
    lines: &mut Vec<String>,
) -> Result<()> {
    for f in rewritten {
        hub.save_feature(f).map_err(|e| match recovery {
            Some(hint) => HubError::Precondition(format!("{e}\n{hint}")),
            None => e,
        })?;
    }
    for f in rewritten
        .iter()
        .filter(|f| f.is_open() && f.open_change(new).is_some())
    {
        if let Some(tmux) = &hub.tmux {
            let step = (|| -> Result<Vec<String>> {
                if tmux.has_session(&f.checkout)?
                    && tmux.windows(&f.checkout)?.iter().any(|w| w == old)
                {
                    tmux.rename_window(&f.checkout, old, new)?;
                    return Ok(vec![format!(
                        "Renamed tmux window {old} to {new} in {}",
                        f.checkout
                    )]);
                }
                Ok(Vec::new())
            })();
            after_save(
                lines,
                step,
                &format!("tmux rename-window -t ={}:{old} {new}", f.checkout),
            );
        }
        after_save(
            lines,
            write_workspace(hub, f).map(|_| Vec::new()),
            &format!("hub sync --feature {}", f.name),
        );
    }
    Ok(())
}

fn summary_line(old: &str, changes: usize, features: usize) -> String {
    if changes == 0 {
        format!("No changes recorded for role {old}")
    } else {
        format!("Renamed {changes} changes in {features} features")
    }
}
