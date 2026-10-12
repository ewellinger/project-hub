//! Every interactive prompt, and the only module that imports `inquire`.
//! Nothing here touches hub state: callers run these before the lock.

use std::cell::RefCell;
use std::fmt;
use std::io::IsTerminal;

use hub::error::{HubError, Result};
use hub::feature::Feature;
use hub::git::Git;
use hub::hub::Hub;
use hub::manifest::RepoSpec;
use hub::ops::start;
use hub::ops::suggest::{self, BranchRow, branch_rows};
use inquire::validator::Validation;
use inquire::{InquireError, MultiSelect, Select, Text};

/// Prompts need a keyboard and a screen: stdin and stderr must both be
/// terminals. Everything else (pipes, agents, CI) is non-interactive.
pub fn is_interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Roles to work on, each with its branch when already known.
type StartRepos = Vec<(String, Option<String>)>;

/// `start`: when no `--repo` was given, ask which roles take part (none is
/// a docs-only start) and then which of the remaining roles to freeze on
/// top of any `--freeze` roles (default none); then ask for a branch for
/// every role that has none. Returns the roles to work on and the roles to
/// freeze.
pub fn pick_start_repos(
    hub: &Hub,
    name: &str,
    checkout: Option<&str>,
    given: StartRepos,
    freeze: Vec<String>,
) -> Result<(StartRepos, Vec<String>)> {
    // Fail the way `start` would before asking anything.
    start::preflight(hub, name, checkout)?;
    start::check_roles(hub, &given, &freeze)?;
    let mut repos = given;
    let mut freeze = freeze;
    if repos.is_empty() {
        let frozen: Vec<&str> = freeze.iter().map(String::as_str).collect();
        let rows = role_rows(&hub.manifest.repos, &frozen);
        if rows.is_empty() {
            // `MultiSelect` refuses an empty list, but a docs-only start is
            // valid here just as it is non-interactively.
            eprintln!("No roles selected: docs-only start");
        } else {
            let chosen = MultiSelect::new(&format!("Roles for {name}:"), rows)
                .with_help_message(
                    "space toggles, enter confirms; nothing selected = docs-only start",
                )
                .prompt()
                .map_err(prompt_error)?;
            if chosen.is_empty() {
                eprintln!("No roles selected: docs-only start");
            }
            repos = chosen
                .into_iter()
                .map(|r| (r.0.role.clone(), None))
                .collect();
            // Roles already worked on or frozen by `--freeze` aren't offered.
            let taken: Vec<&str> = repos
                .iter()
                .map(|(r, _)| r.as_str())
                .chain(freeze.iter().map(String::as_str))
                .collect();
            let rest = role_rows(&hub.manifest.repos, &taken);
            if !rest.is_empty() {
                let picked =
                    MultiSelect::new("Freeze any other roles at their local base tip?", rest)
                        .with_help_message(
                            "space toggles, enter confirms; nothing selected = freeze none",
                        )
                        .prompt()
                        .map_err(prompt_error)?;
                freeze.extend(picked.into_iter().map(|r| r.0.role.clone()));
            }
        }
    }
    // The feature does not exist yet; only its name matters to suggestions.
    let feature = Feature::new(name, name);
    for (role, branch) in &mut repos {
        if branch.is_none() {
            *branch = Some(pick_branch(hub, &feature, role)?);
        }
    }
    Ok((repos, freeze))
}

/// Manifest roles not named in `taken`, as prompt rows in manifest order.
fn role_rows<'a>(repos: &'a [RepoSpec], taken: &[&str]) -> Vec<RoleRow<'a>> {
    repos
        .iter()
        .filter(|r| !taken.contains(&r.role.as_str()))
        .map(RoleRow)
        .collect()
}

/// `add`: ask for the role when missing (only roles without an open change),
/// then for the branch when missing, then, for a new branch of a frozen role,
/// whether it starts from the pin or from the latest base.
pub fn pick_add(
    hub: &Hub,
    feature: &Feature,
    role: Option<String>,
    branch: Option<String>,
    ask_start: bool,
) -> Result<(String, Option<String>, bool)> {
    let role = match role {
        Some(r) => {
            // Fail the way `add` would before asking anything.
            if let Some(existing) = feature.open_change(&r) {
                return Err(HubError::Precondition(format!(
                    "role '{r}' already has an open change on branch '{}'",
                    existing.branch
                )));
            }
            r
        }
        None => {
            let rows: Vec<RoleRow> = suggest::addable_roles(hub, feature)?
                .into_iter()
                .map(RoleRow)
                .collect();
            Select::new(&format!("Role to add to {}:", feature.name), rows)
                .prompt()
                .map_err(prompt_error)?
                .0
                .role
                .clone()
        }
    };
    let branch = match branch {
        Some(b) => b,
        None => pick_branch(hub, feature, &role)?,
    };
    let from_latest = ask_start && pick_start(hub, feature, &role, &branch)?;
    Ok((role, Some(branch), from_latest))
}

/// A brand-new branch for a frozen role starts at the pin or at
/// `origin/<base>`; true means the latter. Existing branches keep the pin
/// without asking.
fn pick_start(hub: &Hub, feature: &Feature, role: &str, branch: &str) -> Result<bool> {
    let Some(pin) = feature.pin(role).filter(|p| p.worktree.is_some()) else {
        return Ok(false);
    };
    let git = Git::new(hub.env.clone_path(&pin.clone));
    if git.local_branch_exists(branch)? || git.remote_branch_exists(branch)? {
        return Ok(false);
    }
    let frozen = format!("from frozen {}", pin.short());
    let latest = format!("from latest origin/{}", pin.base);
    let choice = Select::new(
        &format!("Start {branch} for {role}:"),
        vec![frozen, latest.clone()],
    )
    .with_help_message("from frozen keeps the pin; from latest drops it")
    .prompt()
    .map_err(prompt_error)?;
    Ok(choice == latest)
}

fn pick_branch(hub: &Hub, feature: &Feature, role: &str) -> Result<String> {
    let default = suggest::default_branch(hub, feature, role)?;
    let rows = branch_rows(default, suggest::suggestions(hub, feature, role)?);
    // The filter text, kept so `Other` can prefill the name prompt with it.
    let filter = RefCell::new(String::new());
    let scorer = |input: &str, row: &BranchRow, label: &str, _: usize| {
        filter.replace(input.to_string());
        branch_score(input, row, label)
    };
    let choice = Select::new(&format!("Branch for {role}:"), rows)
        .with_help_message("type to filter; enter accepts the highlighted row")
        .with_scorer(&scorer)
        .prompt()
        .map_err(prompt_error)?;
    match choice {
        BranchRow::Default { branch, .. } => Ok(branch),
        BranchRow::Suggested(s) => Ok(s.branch),
        BranchRow::Other => {
            let git = hub.git();
            let typed = filter.take();
            Text::new("Branch name:")
                .with_initial_value(&typed)
                .with_validator(move |input: &str| {
                    let name = input.trim();
                    if name.is_empty() {
                        return Ok(Validation::Invalid("a branch name is required".into()));
                    }
                    match git.check_ref_format(name) {
                        Ok(true) => Ok(Validation::Valid),
                        Ok(false) => Ok(Validation::Invalid(
                            format!("'{name}' is not a valid branch name").into(),
                        )),
                        Err(e) => Err(Box::new(e).into()),
                    }
                })
                .prompt()
                .map_err(prompt_error)
                .map(|s| s.trim().to_string())
        }
    }
}

/// inquire's default scoring, except `Other` survives any filter and sorts
/// last, so a typed name that matches no branch still has a row to pick.
fn branch_score(input: &str, row: &BranchRow, label: &str) -> Option<i64> {
    match row {
        BranchRow::Other => Some(i64::MIN),
        _ => Select::<BranchRow>::DEFAULT_SCORER(input, row, label, 0),
    }
}

/// Esc and Ctrl-C are a cancel (exit 1, nothing changed); anything else is
/// a terminal failure.
fn prompt_error(err: InquireError) -> HubError {
    match err {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => {
            HubError::Usage("cancelled; nothing changed".into())
        }
        InquireError::IO(e) => HubError::io("prompting", e),
        other => HubError::Usage(format!("prompt failed: {other}")),
    }
}

/// One line per manifest repo: `role  clone  description`.
pub struct RoleRow<'a>(pub &'a RepoSpec);

impl fmt::Display for RoleRow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}", self.0.role, self.0.clone)?;
        if !self.0.description.is_empty() {
            write!(f, "  {}", self.0.description)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_error_maps_cancel_and_interrupt_to_the_cancelled_message() {
        for err in [
            InquireError::OperationCanceled,
            InquireError::OperationInterrupted,
        ] {
            match prompt_error(err) {
                HubError::Usage(msg) => assert_eq!(msg, "cancelled; nothing changed"),
                other => panic!("expected HubError::Usage, got {other:?}"),
            }
        }
    }

    #[test]
    fn branch_score_keeps_other_under_any_filter_and_ranks_it_last() {
        let rows = branch_rows("feat-1".into(), vec![]);
        let other = rows.last().unwrap();
        assert_eq!(
            branch_score("fix/zz", other, "type another name"),
            Some(i64::MIN)
        );
        assert_eq!(branch_score("", other, "type another name"), Some(i64::MIN));
        assert_eq!(branch_score("fix/zz", &rows[0], "feat-1  new"), None);
        assert!(branch_score("feat", &rows[0], "feat-1  new").is_some());
    }

    #[test]
    fn role_row_shows_role_clone_and_description() {
        let repo = hub::manifest::RepoSpec {
            role: "api".into(),
            clone: "api-clone".into(),
            remote: "git@example.com:x.git".into(),
            base: "master".into(),
            branch_template: "{feature}".into(),
            description: "API service".into(),
        };
        assert_eq!(RoleRow(&repo).to_string(), "api  api-clone  API service");
        let bare = hub::manifest::RepoSpec {
            description: String::new(),
            ..repo
        };
        assert_eq!(RoleRow(&bare).to_string(), "api  api-clone");
    }

    #[test]
    fn role_rows_leave_out_taken_roles_in_manifest_order() {
        let spec = |role: &str| hub::manifest::RepoSpec {
            role: role.into(),
            clone: format!("{role}-clone"),
            remote: "git@example.com:x.git".into(),
            base: "master".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        };
        let repos = vec![spec("api"), spec("bff"), spec("ui")];
        fn names(rows: Vec<RoleRow<'_>>) -> Vec<String> {
            rows.iter().map(|r| r.0.role.clone()).collect()
        }
        assert_eq!(names(role_rows(&repos, &[])), ["api", "bff", "ui"]);
        assert_eq!(names(role_rows(&repos, &["ui", "api"])), ["bff"]);
        assert!(role_rows(&repos, &["api", "bff", "ui"]).is_empty());
    }
}
