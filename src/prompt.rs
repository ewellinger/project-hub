//! Every interactive prompt, and the only module that imports `inquire`.
//! Nothing here touches hub state: callers run these before the lock.

use std::fmt;
use std::io::IsTerminal;

use hub::error::{HubError, Result};
use hub::feature::Feature;
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

/// `start`: when no `--repo` was given, ask which roles take part (none is
/// a docs-only start); then ask for a branch for every role that has none.
pub fn pick_start_repos(
    hub: &Hub,
    name: &str,
    checkout: Option<&str>,
    given: Vec<(String, Option<String>)>,
) -> Result<Vec<(String, Option<String>)>> {
    // Fail the way `start` would before asking anything.
    start::preflight(hub, name, checkout)?;
    let mut repos = given;
    if repos.is_empty() {
        if hub.manifest.repos.is_empty() {
            // No manifest options: `MultiSelect` refuses an empty list, but
            // a docs-only start is valid here just as it is non-interactively.
            eprintln!("No roles selected: docs-only start");
        } else {
            let rows: Vec<RoleRow> = hub.manifest.repos.iter().map(RoleRow).collect();
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
        }
    }
    // The feature does not exist yet; only its name matters to suggestions.
    let feature = Feature::new(name, name);
    for (role, branch) in &mut repos {
        if branch.is_none() {
            *branch = Some(pick_branch(hub, &feature, role)?);
        }
    }
    Ok(repos)
}

/// `add`: ask for the role when missing (only roles without an open change),
/// then for the branch when missing.
pub fn pick_add(
    hub: &Hub,
    feature: &Feature,
    role: Option<String>,
    branch: Option<String>,
) -> Result<(String, Option<String>)> {
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
    Ok((role, Some(branch)))
}

fn pick_branch(hub: &Hub, feature: &Feature, role: &str) -> Result<String> {
    let default = suggest::default_branch(hub, feature, role)?;
    let rows = branch_rows(default, suggest::suggestions(hub, feature, role)?);
    let choice = Select::new(&format!("Branch for {role}:"), rows)
        .with_help_message("type to filter; enter accepts the highlighted row")
        .prompt()
        .map_err(prompt_error)?;
    match choice {
        BranchRow::Default { branch, .. } => Ok(branch),
        BranchRow::Suggested(s) => Ok(s.branch),
        BranchRow::Other => {
            let git = hub.git();
            Text::new("Branch name:")
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
}
