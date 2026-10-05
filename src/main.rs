use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use hub::env::Env;
use hub::error::{HubError, Result};
use hub::feature::FeatureStatus;
use hub::hub::Hub;
use hub::ops;

mod prompt;
mod ui;

#[derive(Parser)]
#[command(
    name = "hub",
    version,
    about = "Coordinate features that span several git repositories"
)]
struct Cli {
    /// With no subcommand, opens the dashboard when run in a terminal.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Turn the current directory into a hub coordinator
    Init {
        /// Hub name; defaults to the directory name
        #[arg(long)]
        name: Option<String>,
        /// Template for tmux session and hub worktree names, e.g. "acme-{feature_snake}"
        #[arg(long)]
        checkout_template: Option<String>,
        /// Refresh the files init owns in an existing hub: .gitignore, the generated skill; removes a pre-0.14.0 pre-push hook
        #[arg(long)]
        force: bool,
    },
    /// Register or unregister member repos
    Repo {
        #[command(subcommand)]
        command: RepoCommand,
    },
    /// Start, extend, and finish features
    Feature {
        #[command(subcommand)]
        command: FeatureCommand,
    },
    /// Create or repair the feature's tmux session (never attaches)
    Tmux {
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
    },
    /// Open the feature's VS Code workspace, or the base workspace from the main hub
    Open {
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
    },
    /// Regenerate the feature's workspace file
    Sync {
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
    },
    /// Fast-forward each role's checkout to its copy on origin: the base checkouts from the main hub, the feature's open changes from a feature worktree
    Pull {
        /// Feature name; inferred from a feature worktree, or omit to pull the base checkouts from main
        #[arg(long)]
        feature: Option<String>,
        /// Pull the base checkouts even when invoked from a feature worktree
        #[arg(long, conflicts_with = "feature")]
        base: bool,
    },
    /// Compare feature or base checkouts against reality (exit 30 on structural drift)
    Status {
        /// Feature name; inferred from a feature worktree, or omit for base status from main
        #[arg(long)]
        feature: Option<String>,
        /// Report base checkouts even when invoked from a feature worktree
        #[arg(long, conflicts_with = "feature")]
        base: bool,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RepoCommand {
    /// Register a clone at <project_home>/<clone> under a role
    Add {
        /// Role name to register the clone under
        role: String,
        /// Directory name at <project_home>/<clone>
        clone: String,
        /// Branch new work starts from; inferred from origin/HEAD when omitted
        #[arg(long)]
        base: Option<String>,
        /// Default branch name, must contain {feature}
        #[arg(long)]
        branch_template: Option<String>,
        /// Human-readable description of the role
        #[arg(long)]
        description: Option<String>,
    },
    /// Change a registered role's fields in place, keeping its position
    Set {
        /// Role to change
        role: String,
        /// New directory name at <project_home>/<clone>; refused while the role has an open change
        #[arg(long)]
        clone: Option<String>,
        /// New default base for changes opened from now on
        #[arg(long)]
        base: Option<String>,
        /// New default branch name, must contain {feature}
        #[arg(long)]
        branch_template: Option<String>,
        /// New description; "" clears it
        #[arg(long)]
        description: Option<String>,
    },
    /// Rename a role in hub.json and in this machine's feature records
    Rename {
        /// Current role name
        old: String,
        /// New role name
        new: String,
        /// Only rewrite this machine's records (after pulling a rename, or to repair them)
        #[arg(long)]
        records_only: bool,
        /// With --records-only: only changes recorded with this clone
        #[arg(long)]
        clone: Option<String>,
    },
    /// Unregister a role; the clone is untouched
    Remove {
        /// Role to unregister
        role: String,
    },
    /// Show the member repos in deployment order
    List {
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FeatureCommand {
    /// Create the hub branch and worktree, open a change per --repo, and the tmux session
    Start {
        /// Feature name
        name: String,
        /// tmux session and hub worktree name; defaults to checkout_template
        #[arg(long)]
        checkout: Option<String>,
        /// Role to include, optionally with a branch: --repo ui --repo api:feature/x
        #[arg(long = "repo", value_name = "ROLE[:BRANCH]")]
        repos: Vec<String>,
    },
    /// Create a review feature for one role from a GitLab merge request or GitHub pull request URL
    StartReview {
        /// Merge request or pull request URL
        url: String,
        /// Feature and hub branch name; defaults to <role>-review-<number>
        #[arg(long)]
        name: Option<String>,
        /// tmux session and hub worktree name; defaults to checkout_template
        #[arg(long)]
        checkout: Option<String>,
    },
    /// Open a change for one more role in the current feature
    Add {
        /// Role to bring into the feature; prompted for in a terminal when omitted
        role: Option<String>,
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
        /// Branch name; defaults to the role's template, or the previous branch plus -<n> after a merge
        #[arg(long)]
        branch: Option<String>,
        /// Start point for a brand-new branch; defaults to origin/<base>
        #[arg(long)]
        from: Option<String>,
        /// Branch this change merges into; defaults to the role's base in hub.json
        #[arg(long)]
        base: Option<String>,
        /// Directory name under <worktree_dir>/<clone>/; defaults to the flattened branch
        #[arg(long)]
        worktree: Option<String>,
    },
    /// Record the branch a role's open change merges into; the member branch is not touched
    SetBase {
        /// Role whose open change gets the new base
        role: String,
        /// Base branch name, without origin/
        branch: String,
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
    },
    /// Mark a role's open change as in review and record its MR/PR URL
    Review {
        /// Role whose change is in review
        role: String,
        /// Merge/pull request URL
        url: Option<String>,
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
    },
    /// Close a role's open change: remove its worktree, keep the branch
    Merged {
        /// Role whose change merged
        role: String,
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
        /// Remove even if dirty or adopted
        #[arg(long)]
        force: bool,
    },
    /// Remove the feature's worktrees and tmux session; keeps every branch
    Finish {
        /// Feature name; inferred from the hub worktree when omitted
        #[arg(long)]
        feature: Option<String>,
        /// Remove dirty and adopted worktrees without asking
        #[arg(long)]
        force: bool,
    },
    /// Show open features with the stage of each role
    List {
        /// Include finished features
        #[arg(long)]
        all: bool,
        /// Print as JSON
        #[arg(long)]
        json: bool,
    },
}

fn parse_repo_flag(value: &str) -> (String, Option<String>) {
    match value.split_once(':') {
        Some((role, branch)) => (role.to_string(), Some(branch.to_string())),
        None => (value.to_string(), None),
    }
}

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
        }
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(err.exit_code());
        }
    }
}

fn cwd() -> Result<PathBuf> {
    std::env::current_dir().map_err(|e| HubError::io("reading current directory", e))
}

/// Locate the hub, then bring the generated agent skill in every worktree
/// up to this binary's copy. Skill problems are warnings on stderr so
/// stdout stays clean for `--json`.
fn locate() -> Result<Hub> {
    let hub = Hub::locate(&cwd()?, Env::from_process()?)?;
    for warning in hub::skill::refresh(&hub) {
        eprintln!("warning: {warning}");
    }
    Ok(hub)
}

/// Bare `hub`: the dashboard in a terminal, clap's missing-subcommand error
/// on stderr (exit 2) otherwise, so scripts and agents that forget a
/// subcommand see the same thing as before.
fn dashboard() -> Result<Vec<String>> {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        Cli::command()
            .error(
                ErrorKind::MissingSubcommand,
                "requires a subcommand; run `hub` in a terminal for the dashboard",
            )
            .exit();
    }
    ui::run(locate()?)?;
    Ok(Vec::new())
}

/// Ask before removing a worktree the hub did not create. Non-interactive: no.
fn confirm_removal(path: &Path) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("Remove adopted worktree {}? [y/N] ", path.display());
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "y" | "Y" | "yes")
}

fn run(cli: Cli) -> Result<Vec<String>> {
    let Some(command) = cli.command else {
        return dashboard();
    };
    match command {
        Command::Init {
            name,
            checkout_template,
            force,
        } => ops::init::init(
            &cwd()?,
            &ops::init::InitOptions {
                name,
                checkout_template,
                force,
            },
        ),
        Command::Repo { command } => {
            let hub = locate()?;
            match command {
                RepoCommand::Add {
                    role,
                    clone,
                    base,
                    branch_template,
                    description,
                } => {
                    let opts = ops::repo::AddRepoOptions {
                        role,
                        clone,
                        base,
                        branch_template,
                        description,
                    };
                    hub.with_lock(|hub| ops::repo::add(hub, &opts))
                }
                RepoCommand::Set {
                    role,
                    clone,
                    base,
                    branch_template,
                    description,
                } => {
                    let opts = ops::repo::SetRepoOptions {
                        role,
                        clone,
                        base,
                        branch_template,
                        description,
                    };
                    hub.with_lock(|hub| ops::repo::set(hub, &opts))
                }
                RepoCommand::Rename {
                    old,
                    new,
                    records_only,
                    clone,
                } => {
                    let opts = ops::repo::RenameOptions {
                        old,
                        new,
                        records_only,
                        clone,
                    };
                    hub.with_lock(|hub| ops::repo::rename(hub, &opts))
                }
                RepoCommand::Remove { role } => hub.with_lock(|hub| ops::repo::remove(hub, &role)),
                RepoCommand::List { json } => {
                    if json {
                        Ok(vec![ops::repo::list_json(&hub)?])
                    } else {
                        Ok(ops::repo::list_table(&hub))
                    }
                }
            }
        }
        Command::Feature { command } => {
            let hub = locate()?;
            match command {
                FeatureCommand::Start {
                    name,
                    checkout,
                    repos,
                } => {
                    let mut repos: Vec<(String, Option<String>)> =
                        repos.iter().map(|r| parse_repo_flag(r)).collect();
                    // Prompts finish before the lock; cancelling touches nothing.
                    if prompt::is_interactive() {
                        repos = prompt::pick_start_repos(&hub, &name, checkout.as_deref(), repos)?;
                    }
                    let opts = ops::start::StartOptions {
                        name,
                        checkout,
                        repos,
                    };
                    hub.with_lock(|hub| ops::start::start(hub, &opts))
                }
                FeatureCommand::StartReview {
                    url,
                    name,
                    checkout,
                } => {
                    let opts = ops::start_review::StartReviewOptions {
                        url,
                        name,
                        checkout,
                    };
                    hub.with_lock(|hub| ops::start_review::start_review(hub, &opts))
                }
                FeatureCommand::Add {
                    role,
                    feature,
                    branch,
                    from,
                    base,
                    worktree,
                } => {
                    let (role, branch) = match role {
                        Some(role) if branch.is_some() || !prompt::is_interactive() => {
                            (role, branch)
                        }
                        None if !prompt::is_interactive() => Cli::command()
                            .error(
                                ErrorKind::MissingRequiredArgument,
                                "role is required when not running in a terminal",
                            )
                            .exit(),
                        role => {
                            // Load and check the feature before asking anything so a
                            // finished feature fails the same way it does with flags.
                            let f = hub.resolve_feature(feature.as_deref())?;
                            f.require_open()?;
                            prompt::pick_add(&hub, &f, role, branch)?
                        }
                    };
                    let opts = ops::resolve::AddOptions {
                        role: &role,
                        branch: branch.as_deref(),
                        from: from.as_deref(),
                        worktree: worktree.as_deref(),
                        base: base.as_deref(),
                    };
                    hub.with_lock(|hub| {
                        let mut f = hub.resolve_feature(feature.as_deref())?;
                        ops::add::add(hub, &mut f, &opts)
                    })
                }
                FeatureCommand::SetBase {
                    role,
                    branch,
                    feature,
                } => hub.with_lock(|hub| {
                    let mut f = hub.resolve_feature(feature.as_deref())?;
                    ops::set_base::set_base(hub, &mut f, &role, &branch)
                }),
                FeatureCommand::Review { role, url, feature } => hub.with_lock(|hub| {
                    let mut f = hub.resolve_feature(feature.as_deref())?;
                    ops::review::review(hub, &mut f, &role, url.as_deref())
                }),
                FeatureCommand::Merged {
                    role,
                    feature,
                    force,
                } => hub.with_lock(|hub| {
                    let mut f = hub.resolve_feature(feature.as_deref())?;
                    ops::merged::merged(hub, &mut f, &role, force, &mut confirm_removal)
                }),
                FeatureCommand::Finish { feature, force } => hub.with_lock(|hub| {
                    let mut f = hub.resolve_feature(feature.as_deref())?;
                    ops::finish::finish(hub, &mut f, force, &mut confirm_removal)
                }),
                FeatureCommand::List { all, json } => {
                    let mut summaries = ops::list::list(&hub)?;
                    let hidden = if all {
                        0
                    } else {
                        let before = summaries.len();
                        summaries.retain(|s| s.status == FeatureStatus::Open);
                        before - summaries.len()
                    };
                    if json {
                        Ok(vec![ops::list::render_json(&summaries)?])
                    } else {
                        let mut lines = ops::list::render_table(&summaries);
                        if summaries.is_empty() && hidden > 0 {
                            lines.push(
                                "no open features; pass --all to include finished ones".into(),
                            );
                        }
                        Ok(lines)
                    }
                }
            }
        }
        Command::Tmux { feature } => {
            let hub = locate()?;
            if let Some(off) = &hub.tmux_off {
                return Err(HubError::Usage(format!("tmux is off ({})", off.reason)));
            }
            let f = hub.resolve_feature(feature.as_deref())?;
            f.require_open()?;
            ops::session::converge_tmux(&hub, &f)
        }
        Command::Open { feature } => {
            let hub = locate()?;
            let path = if feature.is_some() || hub.cwd_feature.is_some() {
                let f = hub.resolve_feature(feature.as_deref())?;
                f.require_open()?;
                ops::session::write_workspace(&hub, &f)?
            } else {
                ops::session::write_base_workspace(&hub)?
            };
            hub::code::open_workspace(&hub.env.editor, &path)?;
            Ok(vec![format!("Opened {}", path.display())])
        }
        Command::Sync { feature } => {
            let hub = locate()?;
            let f = hub.resolve_feature(feature.as_deref())?;
            f.require_open()?;
            let path = ops::session::write_workspace(&hub, &f)?;
            Ok(vec![format!("Wrote {}", path.display())])
        }
        Command::Pull { feature, base } => {
            let hub = locate()?;
            if !base && (feature.is_some() || hub.cwd_feature.is_some()) {
                hub.with_lock(|hub| {
                    let f = hub.resolve_feature(feature.as_deref())?;
                    ops::pull::pull_feature(hub, &f)
                })
            } else {
                hub.with_lock(ops::pull::pull_base)
            }
        }
        Command::Status {
            feature,
            base,
            json,
        } => {
            let hub = locate()?;
            let (lines, drift) = if !base && (feature.is_some() || hub.cwd_feature.is_some()) {
                let f = hub.resolve_feature(feature.as_deref())?;
                let report = ops::status::status(&hub, &f)?;
                let lines = if json {
                    vec![ops::status::render_json(&report)?]
                } else {
                    ops::status::render_table(&report, hub.tmux.is_some())
                };
                (lines, report.drift)
            } else {
                let report = ops::status::base_status(&hub)?;
                let lines = if json {
                    vec![ops::status::render_base_json(&report)?]
                } else {
                    ops::status::render_base_table(&report)
                };
                (lines, report.drift)
            };
            for line in lines {
                println!("{line}");
            }
            if drift {
                return Err(HubError::Drift);
            }
            Ok(Vec::new())
        }
    }
}
