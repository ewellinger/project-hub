use std::collections::HashSet;

use crate::error::{HubError, Result};
use crate::feature::Feature;
use crate::hub::Hub;
use crate::names;
use crate::ops::resolve::{AddOptions, Created, open_change};
use crate::ops::session::{after_save, converge_tmux, write_workspace};
use crate::skill;

pub struct StartOptions {
    pub name: String,
    pub checkout: Option<String>,
    /// `(role, branch override)` as given on the command line.
    pub repos: Vec<(String, Option<String>)>,
}

/// The checks `start` runs before touching anything: feature-name validation,
/// the checkout name (`--checkout` or the template) plus its validation, that
/// no feature file exists yet, and that the hub worktree path is free.
/// Returns the resolved checkout name.
pub fn preflight(hub: &Hub, name: &str, checkout: Option<&str>) -> Result<String> {
    names::validate_name("feature name", name)?;
    let checkout = checkout.map(str::to_string).unwrap_or_else(|| {
        names::checkout_name(hub.manifest.checkout_template(), name, &hub.manifest.name)
    });
    names::validate_name("checkout name", &checkout)?;
    if hub.feature_path(name).exists() {
        return Err(HubError::Precondition(format!(
            "feature '{name}' already exists"
        )));
    }
    let hub_wt = hub.hub_worktree_path(&checkout);
    if hub_wt.exists() {
        return Err(HubError::Precondition(format!(
            "{} already exists",
            hub_wt.display()
        )));
    }
    Ok(checkout)
}

pub fn start(hub: &Hub, opts: &StartOptions) -> Result<Vec<String>> {
    let checkout = preflight(hub, &opts.name, opts.checkout.as_deref())?;
    let mut seen = HashSet::new();
    for (role, _) in &opts.repos {
        hub.manifest.require_repo(role)?;
        if !seen.insert(role.as_str()) {
            return Err(HubError::Usage(format!(
                "role '{role}' given more than once"
            )));
        }
    }
    let mut repos = opts.repos.clone();
    repos.sort_by_key(|(role, _)| hub.manifest.role_index(role).unwrap_or(usize::MAX));

    // Transaction: everything up to and including the record write is
    // rolled back on failure. After the write, workspace and tmux failures
    // are warnings (see `after_save`), never a rollback.
    let mut created = Created::new();
    let (feature, mut lines) = match start_inner(hub, opts, &checkout, &repos, &mut created) {
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

/// Git work and the record write. Returns the saved feature and the
/// lines describing what was created or adopted.
fn start_inner(
    hub: &Hub,
    opts: &StartOptions,
    checkout: &str,
    repos: &[(String, Option<String>)],
    created: &mut Created,
) -> Result<(Feature, Vec<String>)> {
    let mut lines = Vec::new();
    let git = hub.git();
    // A shared hub may already carry this feature's branch on origin. Fetch
    // first so the adoption below sees it; offline is a warning, not an error.
    if git.has_remotes()?
        && let Err(err) = git.fetch("origin")
    {
        lines.push(format!(
            "warning: fetch failed in {}: {err}; using the last fetched refs",
            hub.root.display()
        ));
    }
    if git.local_branch_exists(&opts.name)? {
        lines.push(format!("Adopting existing hub branch {}", opts.name));
    } else if git.remote_branch_exists(&opts.name)? {
        git.create_tracking_branch(&opts.name)?;
        created.branches.push((hub.root.clone(), opts.name.clone()));
        lines.push(format!("Adopting hub branch {} from origin", opts.name));
    } else {
        git.create_branch(&opts.name, "main")?;
        created.branches.push((hub.root.clone(), opts.name.clone()));
    }
    let hub_wt = hub.hub_worktree_path(checkout);
    git.worktree_add(&hub_wt, &opts.name)?;
    created.worktrees.push((hub.root.clone(), hub_wt.clone()));
    lines.push(format!(
        "Hub worktree: {} (branch {})",
        hub_wt.display(),
        opts.name
    ));

    let mut feature = Feature::new(&opts.name, checkout);
    for (role, branch) in repos {
        let add = AddOptions {
            role,
            branch: branch.as_deref(),
            from: None,
            worktree: None,
            base: None,
        };
        let opened = open_change(hub, &feature, &add, created)?;
        lines.push(format!(
            "{role}: {} at {}",
            opened.change.branch,
            opened.path.display()
        ));
        lines.extend(opened.lines.into_iter().map(|l| format!("  {l}")));
        feature.push_change(opened.change)?;
    }

    hub.save_feature(&feature)?;
    Ok((feature, lines))
}
