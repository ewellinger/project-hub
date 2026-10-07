//! Multi-step flows: what happens after each overlay answer. A flow holds
//! what it has collected; `advance` returns the next overlay, a load to
//! wait on, the action to run, or a cancellation.

use hub::env::{home_dir, shorten_home};
use hub::feature::{Change, Feature, Owner};
use hub::names;
use hub::ops::status::StatusRow;
use hub::ops::suggest::BranchRow;

use super::{Action, Answer, App, Effect, Loaded, Overlay, Preview, Tone};

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Show this overlay and wait for its answer.
    Show(Overlay),
    /// Show this overlay (a spinner) and dispatch the effect; the flow
    /// continues on `Answer::Loaded`.
    ShowAndLoad(Overlay, Effect),
    /// The flow is complete; run the action.
    Run(Action),
    /// The flow is complete; dispatch this effect (no action, no lock).
    Effect(Effect),
    /// The flow ends with this footer error.
    Cancel(String),
    /// The event was not for this flow; leave the overlay and flow exactly
    /// as they are.
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    Review {
        feature: String,
        role: String,
    },
    SetBase {
        feature: String,
        role: String,
    },
    Merged {
        feature: String,
        role: String,
    },
    Finish {
        feature: String,
    },
    Copy {
        feature: String,
        change: usize,
        /// `(label, text)` pairs shown by the last `Show`, snapshotted at
        /// `Begin` so a background refresh landing while the overlay is
        /// open can't retarget what Enter copies.
        options: Vec<(String, String)>,
    },
    AddRole(AddRole),
    NewFeature(NewFeature),
}

impl Flow {
    pub fn advance(&mut self, answer: Answer, app: &App) -> Step {
        match self {
            Flow::Review { feature, role } => review(feature, role, answer, app),
            Flow::SetBase { feature, role } => set_base(feature, role, answer, app),
            Flow::Merged { feature, role } => merged(feature, role, answer, app),
            Flow::Finish { feature } => finish(feature, answer, app),
            Flow::Copy {
                feature,
                change,
                options,
            } => copy(feature, *change, options, answer, app),
            Flow::AddRole(state) => add_role(state, answer, app),
            Flow::NewFeature(state) => new_feature(state, answer, app),
        }
    }
}

/// The current feature's record when the flow's feature is the one on screen.
fn feature_record<'a>(app: &'a App, feature: &str) -> Option<&'a hub::feature::Feature> {
    app.feature_report()
        .filter(|(f, _)| f.name == feature)
        .map(|(f, _)| f)
}

fn review(feature: &str, role: &str, answer: Answer, app: &App) -> Step {
    match answer {
        Answer::Begin => {
            let current = feature_record(app, feature)
                .and_then(|f| f.open_change(role))
                .and_then(|c| c.review_url.clone())
                .unwrap_or_default();
            Step::Show(Overlay::input(
                format!("Review URL for {role}"),
                current,
                None,
            ))
        }
        Answer::Text(text) => {
            let url = Some(text.trim().to_string()).filter(|s| !s.is_empty());
            Step::Run(Action::Review {
                feature: feature.to_string(),
                role: role.to_string(),
                url,
            })
        }
        _ => Step::Cancel("unexpected answer".into()),
    }
}

/// `(label, text)` pairs offered for change `change`: the branch always, the
/// worktree path when it exists, the review URL when the change has one.
fn copy_options(app: &App, feature: &str, change: usize) -> Option<Vec<(&'static str, String)>> {
    let (record, report) = app.feature_report().filter(|(f, _)| f.name == feature)?;
    let c = record.changes.get(change)?;
    let row = App::change_rows(report).get(change)?;
    let mut out = vec![("branch", c.branch.clone())];
    if row.exists {
        out.push(("path", row.path.display().to_string()));
    }
    if let Some(url) = &c.review_url {
        out.push(("review", url.clone()));
    }
    Some(out)
}

fn copy(
    feature: &str,
    change: usize,
    options: &mut Vec<(String, String)>,
    answer: Answer,
    app: &App,
) -> Step {
    match answer {
        Answer::Begin => {
            let Some(fresh) = copy_options(app, feature, change) else {
                return Step::Cancel("status not loaded yet; press r".into());
            };
            *options = fresh
                .into_iter()
                .map(|(label, text)| (label.to_string(), text))
                .collect();
            Step::Show(Overlay::select(
                "copy".into(),
                options
                    .iter()
                    .map(|(label, text)| format!("{label}  {text}"))
                    .collect(),
            ))
        }
        Answer::Picked(i) => match options.get(i) {
            Some((label, text)) => Step::Effect(Effect::Copy {
                label: label.clone(),
                text: text.clone(),
            }),
            None => Step::Cancel("unexpected answer".into()),
        },
        _ => Step::Cancel("unexpected answer".into()),
    }
}

fn set_base(feature: &str, role: &str, answer: Answer, app: &App) -> Step {
    let title = format!("Base branch for {role}");
    match answer {
        Answer::Begin => {
            let current = feature_record(app, feature)
                .and_then(|f| f.open_change(role))
                .and_then(|c| c.base.clone())
                .or_else(|| {
                    app.repos
                        .iter()
                        .find(|r| r.role == role)
                        .map(|r| r.base.clone())
                })
                .unwrap_or_default();
            Step::Show(Overlay::input(title, current, None))
        }
        Answer::Text(text) => {
            let branch = text.trim().to_string();
            if branch.is_empty() {
                return Step::Show(
                    Overlay::input(title, text, None)
                        .with_error("a branch name is required".into()),
                );
            }
            if let Some(bare) = branch.strip_prefix("origin/") {
                let msg = format!("pass the branch name without origin/: {bare}");
                return Step::Show(Overlay::input(title, text, None).with_error(msg));
            }
            Step::Run(Action::SetBase {
                feature: feature.to_string(),
                role: role.to_string(),
                branch,
            })
        }
        _ => Step::Cancel("unexpected answer".into()),
    }
}

/// Two lines describing a change's worktree for a confirm overlay:
/// `label  path` then `owner · state`. A change checked out in the main
/// clone has no worktree to remove and gets a single dim line.
pub fn worktree_lines(
    label: &str,
    change: &Change,
    row: Option<&StatusRow>,
) -> Vec<(String, Tone)> {
    let pad = " ".repeat(label.chars().count() + 2);
    let Some(wt) = &change.worktree else {
        return vec![(format!("{label}  main clone, not removed"), Tone::Dim)];
    };
    let path = row
        .map(|r| shorten_home(&r.path, home_dir().as_deref()))
        .unwrap_or_else(|| "path unknown".into());
    let owner = match wt.owner {
        Owner::Hub => "hub-created",
        Owner::Adopted => "adopted",
    };
    let (state, tone) = match row {
        None => ("unknown", Tone::Dim),
        Some(r) if !r.exists => ("missing", Tone::Warn),
        Some(r) if r.dirty => ("dirty", Tone::Warn),
        Some(_) => ("clean", Tone::Dim),
    };
    vec![
        (format!("{label}  {path}"), Tone::Plain),
        (format!("{pad}{owner} · {state}"), tone),
    ]
}

/// The status row for a role's open change. `status` emits one row per
/// change in record order after the hub row, so the open change's index in
/// `record.changes` is its row's index in `change_rows`; a role that was
/// merged and reopened has two rows, and matching on the role name alone
/// would find the merged one.
fn open_row<'a>(
    record: &Feature,
    report: &'a hub::ops::status::StatusReport,
    role: &str,
) -> Option<&'a StatusRow> {
    let i = record
        .changes
        .iter()
        .position(|c| c.role == role && c.is_open())?;
    App::change_rows(report).get(i)
}

fn merged(feature: &str, role: &str, answer: Answer, app: &App) -> Step {
    match answer {
        Answer::Begin => {
            let Some((record, report)) = app.feature_report().filter(|(f, _)| f.name == feature)
            else {
                return Step::Cancel("status not loaded yet; press r".into());
            };
            let Some(change) = record.open_change(role) else {
                return Step::Cancel(format!(
                    "no open change for role '{role}' in feature '{feature}'"
                ));
            };
            let row = open_row(record, report, role);
            let mut lines = vec![(
                format!("Close change     {role}: {}", change.branch),
                Tone::Plain,
            )];
            for (i, (text, tone)) in worktree_lines(role, change, row).into_iter().enumerate() {
                let prefix = if i == 0 {
                    "Remove worktree  "
                } else {
                    "                 "
                };
                lines.push((format!("{prefix}{text}"), tone));
            }
            lines.push((format!("Keep branch      {}", change.branch), Tone::Plain));
            lines.push((String::new(), Tone::Plain));
            if row.is_some_and(|r| r.dirty) {
                lines.push((
                    format!("! {role} is dirty; merged will refuse without force"),
                    Tone::Warn,
                ));
            }
            if change
                .worktree
                .as_ref()
                .is_some_and(|w| w.owner == Owner::Adopted)
            {
                lines.push(("! adopted worktree will be removed".into(), Tone::Warn));
            }
            Step::Show(Overlay::confirm(
                format!("Merge {role} in {feature}"),
                lines,
                true,
            ))
        }
        Answer::Confirmed { force } => Step::Run(Action::Merged {
            feature: feature.to_string(),
            role: role.to_string(),
            force,
        }),
        _ => Step::Cancel("unexpected answer".into()),
    }
}

fn finish(feature: &str, answer: Answer, app: &App) -> Step {
    match answer {
        Answer::Begin => {
            let Some((record, report)) = app.feature_report().filter(|(f, _)| f.name == feature)
            else {
                return Step::Cancel("status not loaded yet; press r".into());
            };
            let mut lines = Vec::new();
            if app.tmux_on {
                lines.push((
                    format!("Kill tmux session   {}", record.checkout),
                    Tone::Plain,
                ));
            }
            let mut first = true;
            let mut push_wt = |lines: &mut Vec<(String, Tone)>, text: String, tone: Tone| {
                let prefix = if first {
                    "Remove worktrees    "
                } else {
                    "                    "
                };
                first = false;
                lines.push((format!("{prefix}{text}"), tone));
            };
            for change in record.open_changes() {
                let row = open_row(record, report, &change.role);
                for (text, tone) in worktree_lines(&change.role, change, row) {
                    push_wt(&mut lines, text, tone);
                }
            }
            // The hub worktree: row 0 of the report. Described like a hub-created tree.
            if let Some(hub_row) = report.rows.first() {
                let path = shorten_home(&hub_row.path, home_dir().as_deref());
                let (state, tone) = if !hub_row.exists {
                    ("missing", Tone::Warn)
                } else if hub_row.dirty {
                    ("dirty", Tone::Warn)
                } else {
                    ("clean", Tone::Dim)
                };
                push_wt(&mut lines, format!("hub  {path}"), Tone::Plain);
                push_wt(&mut lines, format!("     hub-created · {state}"), tone);
            }
            let mut kept: Vec<String> = Vec::new();
            for c in &record.changes {
                if !kept.contains(&c.role) {
                    kept.push(c.role.clone());
                }
            }
            lines.push((
                format!(
                    "Keep branches       {}, and hub branch {}",
                    kept.join(", "),
                    record.name
                ),
                Tone::Plain,
            ));
            lines.push((String::new(), Tone::Plain));
            let unmerged: Vec<String> = record
                .open_changes()
                .iter()
                .map(|c| format!("{} ({})", c.role, c.stage.as_str()))
                .collect();
            if !unmerged.is_empty() {
                lines.push((format!("! not merged: {}", unmerged.join(", ")), Tone::Warn));
            }
            for change in record.open_changes() {
                if open_row(record, report, &change.role).is_some_and(|r| r.dirty) {
                    lines.push((
                        format!(
                            "! {} is dirty; finish will refuse without force",
                            change.role
                        ),
                        Tone::Warn,
                    ));
                }
            }
            if report.rows.first().is_some_and(|r| r.dirty) {
                lines.push((
                    "! hub worktree is dirty; finish will refuse without force".into(),
                    Tone::Warn,
                ));
            }
            let mut overlay = Overlay::confirm(format!("Finish {feature}"), lines, true);
            if app.current_session.as_deref() == Some(record.checkout.as_str())
                && let Overlay::Confirm { blocked, .. } = &mut overlay
            {
                *blocked = Some(format!(
                    "running inside tmux session {}, which finish would kill along with this dashboard; \
                         run it from another terminal: hub feature finish --feature {}",
                    record.checkout, record.name
                ));
            }
            Step::Show(overlay)
        }
        Answer::Confirmed { force } => Step::Run(Action::Finish {
            feature: feature.to_string(),
            force,
        }),
        _ => Step::Cancel("unexpected answer".into()),
    }
}

/// The branch step shared by add-role and the wizard: load rows for a
/// role, let the user pick one, or type a name after `Other`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchPick {
    pub role: String,
    pub rows: Vec<BranchRow>,
    /// `Other` was chosen; the next answer is typed text.
    pub typing: bool,
}

impl BranchPick {
    pub fn begin(feature: Feature, role: String) -> (BranchPick, Step) {
        let title = format!("Branch for {role}");
        let pick = BranchPick {
            role: role.clone(),
            rows: Vec::new(),
            typing: false,
        };
        (
            pick,
            Step::ShowAndLoad(
                Overlay::loading(title),
                Effect::LoadBranches { feature, role },
            ),
        )
    }

    /// Rows loaded for `role`; a stale load from a role the user has since
    /// backed away from is ignored and the spinner kept up.
    pub fn loaded(&mut self, role: &str, rows: Vec<BranchRow>) -> Step {
        if role != self.role {
            return Step::Show(Overlay::loading(format!("Branch for {}", self.role)));
        }
        let labels = rows.iter().map(|r| r.to_string()).collect();
        let other = rows.iter().position(|r| r.branch().is_none());
        self.rows = rows;
        let overlay = Overlay::select(format!("Branch for {}", self.role), labels);
        Step::Show(match other {
            Some(i) => overlay.with_pinned(i),
            None => overlay,
        })
    }

    /// `Ok(branch)` for a named row; `Err(step)` shows the text input for `Other`.
    // `Step` is large because `ShowAndLoad` carries an `Effect` (which can
    // hold a whole `Feature`); this runs on a keypress, not in a hot loop.
    #[allow(clippy::result_large_err)]
    pub fn picked(&mut self, index: usize) -> Result<String, Step> {
        match self.rows.get(index).and_then(|r| r.branch()) {
            Some(branch) => Ok(branch.to_string()),
            None => {
                self.typing = true;
                Err(Step::Show(Overlay::input(
                    format!("Branch for {}", self.role),
                    String::new(),
                    None,
                )))
            }
        }
    }

    /// Text answered on the branch step: the typed name once `Other` is
    /// open, or else the filter the user typed before picking `Other`, which
    /// opens the name input prefilled with it.
    #[allow(clippy::result_large_err)]
    pub fn text(&mut self, text: String) -> Result<String, Step> {
        if self.typing {
            return self.typed(text);
        }
        self.typing = true;
        Err(Step::Show(Overlay::input(
            format!("Branch for {}", self.role),
            text,
            None,
        )))
    }

    #[allow(clippy::result_large_err)]
    fn typed(&self, text: String) -> Result<String, Step> {
        let name = text.trim().to_string();
        match names::validate_branch_name(&name) {
            Ok(()) => Ok(name),
            Err(e) => Err(Step::Show(
                Overlay::input(format!("Branch for {}", self.role), text, None)
                    .with_error(e.to_string()),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRole {
    pub feature: String,
    /// Addable roles in manifest order, filled at `Begin`.
    pub roles: Vec<String>,
    pub pick: Option<BranchPick>,
}

impl AddRole {
    pub fn new(feature: String) -> AddRole {
        AddRole {
            feature,
            roles: Vec::new(),
            pick: None,
        }
    }
}

/// `role  clone  description`, the CLI's `RoleRow` text.
pub fn role_label(repo: &hub::manifest::RepoSpec) -> String {
    let mut s = format!("{}  {}", repo.role, repo.clone);
    if !repo.description.is_empty() {
        s.push_str("  ");
        s.push_str(&repo.description);
    }
    s
}

fn add_role(state: &mut AddRole, answer: Answer, app: &App) -> Step {
    let feature = state.feature.clone();
    // Match on whether a branch pick is in progress, not on `&mut state.pick`
    // itself: the arms below assign to `state.pick` and read `state.roles`.
    let picking = state.pick.is_some();
    match (picking, answer) {
        (false, Answer::Begin) => {
            let Some(record) = feature_record(app, &feature) else {
                return Step::Cancel("status not loaded yet; press r".into());
            };
            let addable: Vec<&hub::manifest::RepoSpec> = app
                .repos
                .iter()
                .filter(|r| record.open_change(&r.role).is_none())
                .collect();
            if addable.is_empty() {
                return Step::Cancel(format!(
                    "every role already has an open change in {feature}"
                ));
            }
            state.roles = addable.iter().map(|r| r.role.clone()).collect();
            let labels = addable.iter().map(|r| role_label(r)).collect();
            Step::Show(Overlay::select(format!("Role to add to {feature}"), labels))
        }
        (false, Answer::Picked(i)) => {
            let Some(role) = state.roles.get(i).cloned() else {
                return Step::Cancel("unexpected answer".into());
            };
            let Some(record) = feature_record(app, &feature) else {
                return Step::Cancel("status not loaded yet; press r".into());
            };
            let (pick, step) = BranchPick::begin(record.clone(), role);
            state.pick = Some(pick);
            step
        }
        (true, Answer::Loaded(Ok(Loaded::Branches { role, rows }))) => {
            state.pick.as_mut().expect("picking").loaded(&role, rows)
        }
        (true, Answer::Loaded(Err(e))) => Step::Cancel(e),
        (true, Answer::Picked(i)) => {
            let pick = state.pick.as_mut().expect("picking");
            match pick.picked(i) {
                Ok(branch) => Step::Run(Action::Add {
                    feature,
                    role: pick.role.clone(),
                    branch,
                }),
                Err(step) => step,
            }
        }
        (true, Answer::Text(text)) => {
            let pick = state.pick.as_mut().expect("picking");
            match pick.text(text) {
                Ok(branch) => Step::Run(Action::Add {
                    feature,
                    role: pick.role.clone(),
                    branch,
                }),
                Err(step) => step,
            }
        }
        _ => Step::Cancel("unexpected answer".into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardStage {
    Name,
    Roles,
    /// Choosing the branch for `roles[index]`.
    Branch(usize),
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFeature {
    pub name: String,
    pub checkout: String,
    pub roles: Vec<String>,
    pub branches: Vec<(String, String)>,
    pub stage: WizardStage,
    pub pick: Option<BranchPick>,
    /// The name whose preflight is currently in flight, if any; correlates
    /// a late `Loaded::Checkout`/preflight error to the request that asked
    /// for it, so a stale result from an abandoned wizard can be ignored.
    pending: Option<String>,
}

impl Default for NewFeature {
    fn default() -> NewFeature {
        NewFeature::new()
    }
}

impl NewFeature {
    pub fn new() -> NewFeature {
        NewFeature {
            name: String::new(),
            checkout: String::new(),
            roles: Vec::new(),
            branches: Vec::new(),
            stage: WizardStage::Name,
            pick: None,
            pending: None,
        }
    }

    fn name_input(&self, app: &App) -> Overlay {
        Overlay::input(
            "New feature".into(),
            self.name.clone(),
            Some(Preview::Checkout {
                template: app.checkout_template.clone(),
                hub: app.hub_name.clone(),
            }),
        )
    }

    /// Move to the branch step for `roles[index]`, or to the summary when
    /// every role has one.
    fn next_branch(&mut self, index: usize, tmux_on: bool) -> Step {
        match self.roles.get(index).cloned() {
            Some(role) => {
                self.stage = WizardStage::Branch(index);
                let feature = Feature::new(&self.name, &self.checkout);
                let (pick, step) = BranchPick::begin(feature, role);
                self.pick = Some(pick);
                step
            }
            None => {
                self.stage = WizardStage::Summary;
                Step::Show(self.summary(tmux_on))
            }
        }
    }

    fn summary(&self, tmux_on: bool) -> Overlay {
        let checkout = if tmux_on {
            format!(
                "Checkout  {} (tmux session {})",
                self.checkout, self.checkout
            )
        } else {
            format!("Checkout  {}", self.checkout)
        };
        let mut lines = vec![
            (format!("Feature   {}", self.name), Tone::Plain),
            (checkout, Tone::Plain),
            ("Roles".into(), Tone::Plain),
        ];
        if self.branches.is_empty() {
            lines.push(("  docs-only start (no roles)".into(), Tone::Dim));
        } else {
            let width = self
                .branches
                .iter()
                .map(|(r, _)| r.chars().count())
                .max()
                .unwrap_or(0);
            for (role, branch) in &self.branches {
                lines.push((format!("  {role:<width$}  {branch}"), Tone::Plain));
            }
        }
        Overlay::confirm(format!("Start {}", self.name), lines, false)
    }

    fn accept_branch(&mut self, branch: String, tmux_on: bool) -> Step {
        let index = match self.stage {
            WizardStage::Branch(i) => i,
            _ => return Step::Cancel("unexpected answer".into()),
        };
        let role = self.roles[index].clone();
        self.branches.push((role, branch));
        self.pick = None;
        self.next_branch(index + 1, tmux_on)
    }
}

fn new_feature(state: &mut NewFeature, answer: Answer, app: &App) -> Step {
    match (&state.stage, answer) {
        (WizardStage::Name, Answer::Begin) => Step::Show(state.name_input(app)),
        (WizardStage::Name, Answer::Text(text)) => {
            let name = text.trim().to_string();
            if name.is_empty() {
                return Step::Show(
                    state
                        .name_input(app)
                        .with_error("a feature name is required".into()),
                );
            }
            state.name = name.clone();
            state.pending = Some(name.clone());
            let mut overlay = state.name_input(app);
            if let Overlay::Input { busy, .. } = &mut overlay {
                *busy = true;
            }
            Step::ShowAndLoad(overlay, Effect::Preflight { name })
        }
        (WizardStage::Name, Answer::Loaded(Ok(Loaded::Checkout { name, checkout }))) => {
            if state.pending.as_deref() != Some(name.as_str()) {
                return Step::Ignore;
            }
            state.pending = None;
            state.checkout = checkout;
            if app.repos.is_empty() {
                state.stage = WizardStage::Summary;
                return Step::Show(state.summary(app.tmux_on));
            }
            state.stage = WizardStage::Roles;
            let rows = app.repos.iter().map(role_label).collect();
            Step::Show(Overlay::multi(
                format!("Roles for {}", state.name),
                rows,
                "nothing selected = docs-only start".into(),
            ))
        }
        (WizardStage::Name, Answer::Loaded(Err(e))) => {
            if state.pending.is_none() {
                return Step::Ignore;
            }
            state.pending = None;
            Step::Show(state.name_input(app).with_error(e))
        }
        (WizardStage::Roles, Answer::PickedMany(indices)) => {
            state.roles = indices
                .iter()
                .filter_map(|i| app.repos.get(*i))
                .map(|r| r.role.clone())
                .collect();
            state.next_branch(0, app.tmux_on)
        }
        (WizardStage::Branch(_), Answer::Loaded(Ok(Loaded::Branches { role, rows }))) => {
            match &mut state.pick {
                Some(pick) => pick.loaded(&role, rows),
                None => Step::Cancel("unexpected answer".into()),
            }
        }
        (WizardStage::Branch(_), Answer::Loaded(Err(e))) => Step::Cancel(e),
        (WizardStage::Branch(_), Answer::Picked(i)) => {
            let picked = match &mut state.pick {
                Some(pick) => pick.picked(i),
                None => return Step::Cancel("unexpected answer".into()),
            };
            match picked {
                Ok(branch) => state.accept_branch(branch, app.tmux_on),
                Err(step) => step,
            }
        }
        (WizardStage::Branch(_), Answer::Text(text)) => {
            let typed = match &mut state.pick {
                Some(pick) => pick.text(text),
                None => return Step::Cancel("unexpected answer".into()),
            };
            match typed {
                Ok(branch) => state.accept_branch(branch, app.tmux_on),
                Err(step) => step,
            }
        }
        (WizardStage::Summary, Answer::Confirmed { .. }) => Step::Run(Action::Start {
            name: state.name.clone(),
            repos: state
                .branches
                .iter()
                .map(|(r, b)| (r.clone(), Some(b.clone())))
                .collect(),
        }),
        // A late preflight result for a stage that has already moved on
        // (or a different wizard entirely): not ours, ignore it rather
        // than tearing down whatever the user is doing now.
        (_, Answer::Loaded(Ok(Loaded::Checkout { .. }))) => Step::Ignore,
        (_, Answer::Loaded(Err(_))) => Step::Ignore,
        _ => Step::Cancel("unexpected answer".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::feature::tests::{change, on_feature, status_row, suspense};
    use crate::ui::app::{Cached, Loaded, Preview, Report, RowId};
    use hub::feature::{Feature, FeatureStatus, Owner, Stage};
    use hub::ops::status::StatusReport;
    use hub::ops::suggest::{BranchRow, Suggestion};

    fn rows() -> Vec<BranchRow> {
        vec![
            BranchRow::Default {
                branch: "feature/y".into(),
                existing: None,
            },
            BranchRow::Suggested(Suggestion {
                branch: "main-ish".into(),
                local: true,
                origin: false,
                worktree: None,
            }),
            BranchRow::Other,
        ]
    }

    fn dashboard_app() -> App {
        let mut a = App::new("acme", vec![]);
        a.checkout_template = "acme-{feature}".into();
        a.repos = vec![
            repo("api", "api-clone", ""),
            repo("ui", "ui-clone", "web ui"),
        ];
        a
    }

    #[test]
    fn wizard_walks_name_roles_branches_and_summary() {
        let app = dashboard_app();
        let mut flow = Flow::NewFeature(NewFeature::new());
        let Step::Show(Overlay::Input {
            title,
            preview: Some(Preview::Checkout { template, hub }),
            ..
        }) = flow.advance(Answer::Begin, &app)
        else {
            panic!()
        };
        assert_eq!(title, "New feature");
        assert_eq!(
            (template.as_str(), hub.as_str()),
            ("acme-{feature}", "acme")
        );
        let Step::ShowAndLoad(
            Overlay::Input {
                busy: true, value, ..
            },
            Effect::Preflight { name },
        ) = flow.advance(Answer::Text("suspense".into()), &app)
        else {
            panic!()
        };
        assert_eq!((value.as_str(), name.as_str()), ("suspense", "suspense"));
        let Step::Show(Overlay::MultiSelect {
            title,
            rows: role_rows,
            ..
        }) = flow.advance(
            Answer::Loaded(Ok(Loaded::Checkout {
                name: "suspense".into(),
                checkout: "acme-suspense".into(),
            })),
            &app,
        )
        else {
            panic!()
        };
        assert_eq!(title, "Roles for suspense");
        assert_eq!(role_rows, vec!["api  api-clone", "ui  ui-clone  web ui"]);
        let Step::ShowAndLoad(
            Overlay::Select {
                title,
                loading: true,
                ..
            },
            Effect::LoadBranches { role, feature },
        ) = flow.advance(Answer::PickedMany(vec![0, 1]), &app)
        else {
            panic!()
        };
        assert_eq!((title.as_str(), role.as_str()), ("Branch for api", "api"));
        assert_eq!(
            (feature.name.as_str(), feature.checkout.as_str()),
            ("suspense", "acme-suspense")
        );
        flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "api".into(),
                rows: rows(),
            })),
            &app,
        );
        let Step::ShowAndLoad(Overlay::Select { title, .. }, _) =
            flow.advance(Answer::Picked(0), &app)
        else {
            panic!()
        };
        assert_eq!(title, "Branch for ui", "second role follows");
        flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "ui".into(),
                rows: rows(),
            })),
            &app,
        );
        flow.advance(Answer::Picked(2), &app); // other
        let Step::Show(Overlay::Confirm {
            title,
            lines,
            force,
            ..
        }) = flow.advance(Answer::Text("feature/custom".into()), &app)
        else {
            panic!()
        };
        assert_eq!(title, "Start suspense");
        assert!(!force);
        let text: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text[0], "Feature   suspense");
        assert_eq!(
            text[1],
            "Checkout  acme-suspense (tmux session acme-suspense)"
        );
        assert_eq!(text[2], "Roles");
        assert_eq!(text[3], "  api  feature/y");
        assert_eq!(text[4], "  ui   feature/custom");
        assert_eq!(
            flow.advance(Answer::Confirmed { force: false }, &app),
            Step::Run(Action::Start {
                name: "suspense".into(),
                repos: vec![
                    ("api".into(), Some("feature/y".into())),
                    ("ui".into(), Some("feature/custom".into()))
                ]
            })
        );
    }

    #[test]
    fn wizard_preflight_failure_stays_on_the_name_step() {
        let app = dashboard_app();
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("suspense".into()), &app);
        let Step::Show(Overlay::Input {
            error: Some(e),
            value,
            busy: false,
            ..
        }) = flow.advance(
            Answer::Loaded(Err("feature 'suspense' already exists".into())),
            &app,
        )
        else {
            panic!()
        };
        assert_eq!(e, "feature 'suspense' already exists");
        assert_eq!(value, "suspense");
        let Step::Show(Overlay::Input { error: Some(e), .. }) =
            flow.advance(Answer::Text("   ".into()), &app)
        else {
            panic!()
        };
        assert_eq!(e, "a feature name is required");
    }

    #[test]
    fn wizard_summary_drops_the_tmux_session_when_tmux_is_off() {
        let mut app = dashboard_app();
        app.tmux_on = false;
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("docs".into()), &app);
        flow.advance(
            Answer::Loaded(Ok(Loaded::Checkout {
                name: "docs".into(),
                checkout: "acme-docs".into(),
            })),
            &app,
        );
        let Step::Show(Overlay::Confirm { lines, .. }) =
            flow.advance(Answer::PickedMany(vec![]), &app)
        else {
            panic!()
        };
        assert_eq!(lines[1].0, "Checkout  acme-docs");
    }

    #[test]
    fn wizard_docs_only_start_skips_branches_and_says_so() {
        let mut app = dashboard_app();
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("docs".into()), &app);
        flow.advance(
            Answer::Loaded(Ok(Loaded::Checkout {
                name: "docs".into(),
                checkout: "acme-docs".into(),
            })),
            &app,
        );
        let Step::Show(Overlay::Confirm { lines, .. }) =
            flow.advance(Answer::PickedMany(vec![]), &app)
        else {
            panic!()
        };
        assert!(
            lines.contains(&("  docs-only start (no roles)".to_string(), Tone::Dim)),
            "{lines:?}"
        );
        assert_eq!(
            flow.advance(Answer::Confirmed { force: false }, &app),
            Step::Run(Action::Start {
                name: "docs".into(),
                repos: vec![]
            })
        );
        // A hub with no repos skips the role step entirely.
        app.repos.clear();
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("docs".into()), &app);
        assert!(matches!(
            flow.advance(
                Answer::Loaded(Ok(Loaded::Checkout {
                    name: "docs".into(),
                    checkout: "acme-docs".into()
                })),
                &app
            ),
            Step::Show(Overlay::Confirm { .. })
        ));
    }

    #[test]
    fn a_stale_preflight_after_restart_is_ignored() {
        let app = dashboard_app();
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("abc".into()), &app);
        // The wizard was cancelled (Esc) and started fresh; the earlier
        // preflight for "abc" is still out there somewhere.
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        assert_eq!(
            flow.advance(
                Answer::Loaded(Ok(Loaded::Checkout {
                    name: "abc".into(),
                    checkout: "acme-abc".into()
                })),
                &app
            ),
            Step::Ignore
        );
    }

    #[test]
    fn a_preflight_for_another_name_is_ignored_while_one_is_pending() {
        let app = dashboard_app();
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("xyz".into()), &app);
        assert_eq!(
            flow.advance(
                Answer::Loaded(Ok(Loaded::Checkout {
                    name: "abc".into(),
                    checkout: "acme-abc".into()
                })),
                &app
            ),
            Step::Ignore
        );
        let Step::Show(Overlay::MultiSelect { title, .. }) = flow.advance(
            Answer::Loaded(Ok(Loaded::Checkout {
                name: "xyz".into(),
                checkout: "acme-xyz".into(),
            })),
            &app,
        ) else {
            panic!()
        };
        assert_eq!(title, "Roles for xyz");
    }

    #[test]
    fn add_role_picks_a_role_loads_branches_and_runs() {
        let app = on_feature(); // repos empty: fill two
        let mut app = app;
        app.repos = vec![
            repo("api", "api-clone", ""),
            repo("ui", "ui-clone", "web ui"),
        ];
        let mut flow = Flow::AddRole(AddRole::new("suspense".into()));
        let Step::Show(Overlay::Select {
            title,
            rows: labels,
            ..
        }) = flow.advance(Answer::Begin, &app)
        else {
            panic!()
        };
        assert_eq!(title, "Role to add to suspense");
        assert_eq!(
            labels,
            vec!["ui  ui-clone  web ui"],
            "api already has an open change"
        );
        let Step::ShowAndLoad(
            Overlay::Select {
                loading: true,
                title,
                ..
            },
            Effect::LoadBranches { role, feature },
        ) = flow.advance(Answer::Picked(0), &app)
        else {
            panic!()
        };
        assert_eq!(title, "Branch for ui");
        assert_eq!(role, "ui");
        assert_eq!(feature.name, "suspense");
        let Step::Show(Overlay::Select {
            rows: labels,
            loading: false,
            ..
        }) = flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "ui".into(),
                rows: rows(),
            })),
            &app,
        )
        else {
            panic!()
        };
        assert_eq!(
            labels,
            vec!["feature/y  new", "main-ish  local", "type another name"]
        );
        assert_eq!(
            flow.advance(Answer::Picked(1), &app),
            Step::Run(Action::Add {
                feature: "suspense".into(),
                role: "ui".into(),
                branch: "main-ish".into()
            })
        );
    }

    #[test]
    fn add_role_other_asks_for_a_name_and_validates_it() {
        let mut app = on_feature();
        app.repos = vec![repo("ui", "ui-clone", "")];
        let mut flow = Flow::AddRole(AddRole::new("suspense".into()));
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Picked(0), &app);
        flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "ui".into(),
                rows: rows(),
            })),
            &app,
        );
        let Step::Show(Overlay::Input { title, .. }) = flow.advance(Answer::Picked(2), &app) else {
            panic!()
        };
        assert_eq!(title, "Branch for ui");
        let Step::Show(Overlay::Input { error: Some(e), .. }) =
            flow.advance(Answer::Text("bad name".into()), &app)
        else {
            panic!()
        };
        assert!(e.contains("not a valid branch name"), "{e}");
        assert_eq!(
            flow.advance(Answer::Text("feature/z".into()), &app),
            Step::Run(Action::Add {
                feature: "suspense".into(),
                role: "ui".into(),
                branch: "feature/z".into()
            })
        );
    }

    #[test]
    fn wizard_branch_filter_text_prefills_the_name_input() {
        let app = dashboard_app();
        let mut flow = Flow::NewFeature(NewFeature::new());
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Text("suspense".into()), &app);
        flow.advance(
            Answer::Loaded(Ok(Loaded::Checkout {
                name: "suspense".into(),
                checkout: "acme-suspense".into(),
            })),
            &app,
        );
        flow.advance(Answer::PickedMany(vec![0]), &app);
        let Step::Show(Overlay::Select { pinned, .. }) = flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "api".into(),
                rows: rows(),
            })),
            &app,
        ) else {
            panic!()
        };
        assert_eq!(pinned, Some(2), "the other row survives any filter");
        // Enter on the pinned row with `fix/zz` typed into the filter.
        let Step::Show(Overlay::Input { title, value, .. }) =
            flow.advance(Answer::Text("fix/zz".into()), &app)
        else {
            panic!()
        };
        assert_eq!(
            (title.as_str(), value.as_str()),
            ("Branch for api", "fix/zz")
        );
        let Step::Show(Overlay::Confirm { lines, .. }) =
            flow.advance(Answer::Text("fix/zz".into()), &app)
        else {
            panic!()
        };
        assert_eq!(lines[3].0, "  api  fix/zz");
    }

    #[test]
    fn add_role_branch_filter_text_prefills_the_name_input() {
        let mut app = on_feature();
        app.repos = vec![repo("ui", "ui-clone", "")];
        let mut flow = Flow::AddRole(AddRole::new("suspense".into()));
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Picked(0), &app);
        flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "ui".into(),
                rows: rows(),
            })),
            &app,
        );
        let Step::Show(Overlay::Input { value, .. }) =
            flow.advance(Answer::Text("fix/zz".into()), &app)
        else {
            panic!()
        };
        assert_eq!(value, "fix/zz");
        assert_eq!(
            flow.advance(Answer::Text("fix/zz".into()), &app),
            Step::Run(Action::Add {
                feature: "suspense".into(),
                role: "ui".into(),
                branch: "fix/zz".into()
            })
        );
    }

    #[test]
    fn a_failed_branch_load_cancels_with_the_error() {
        let mut app = on_feature();
        app.repos = vec![repo("ui", "ui-clone", "")];
        let mut flow = Flow::AddRole(AddRole::new("suspense".into()));
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Picked(0), &app);
        assert_eq!(
            flow.advance(
                Answer::Loaded(Err("clone for role 'ui' is missing: /p/ui-clone".into())),
                &app
            ),
            Step::Cancel("clone for role 'ui' is missing: /p/ui-clone".into())
        );
    }

    #[test]
    fn a_branches_event_for_another_role_keeps_the_spinner() {
        let mut app = on_feature();
        app.repos = vec![repo("ui", "ui-clone", "")];
        let mut flow = Flow::AddRole(AddRole::new("suspense".into()));
        flow.advance(Answer::Begin, &app);
        flow.advance(Answer::Picked(0), &app);
        let Step::Show(Overlay::Select {
            loading: true,
            title,
            ..
        }) = flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "api".into(),
                rows: rows(),
            })),
            &app,
        )
        else {
            panic!()
        };
        assert_eq!(title, "Branch for ui");
        let Step::Show(Overlay::Select {
            rows: labels,
            loading: false,
            ..
        }) = flow.advance(
            Answer::Loaded(Ok(Loaded::Branches {
                role: "ui".into(),
                rows: rows(),
            })),
            &app,
        )
        else {
            panic!()
        };
        assert_eq!(
            labels,
            vec!["feature/y  new", "main-ish  local", "type another name"]
        );
    }

    fn repo(role: &str, clone: &str, description: &str) -> hub::manifest::RepoSpec {
        hub::manifest::RepoSpec {
            role: role.into(),
            clone: clone.into(),
            remote: "origin".into(),
            base: "main".into(),
            branch_template: "feature/{feature}".into(),
            description: description.into(),
        }
    }

    #[test]
    fn review_asks_for_a_url_prefilled_and_runs_with_none_when_empty() {
        let app = on_feature();
        let mut flow = Flow::Review {
            feature: "suspense".into(),
            role: "api".into(),
        };
        let Step::Show(Overlay::Input { title, value, .. }) = flow.advance(Answer::Begin, &app)
        else {
            panic!("expected an input");
        };
        assert_eq!(title, "Review URL for api");
        assert_eq!(value, "");
        assert_eq!(
            flow.advance(Answer::Text("  ".into()), &app),
            Step::Run(Action::Review {
                feature: "suspense".into(),
                role: "api".into(),
                url: None
            })
        );
        assert_eq!(
            flow.advance(Answer::Text("https://x/1".into()), &app),
            Step::Run(Action::Review {
                feature: "suspense".into(),
                role: "api".into(),
                url: Some("https://x/1".into())
            })
        );
    }

    #[test]
    fn set_base_prefills_the_effective_base_and_rejects_origin_prefix() {
        let app = on_feature();
        let mut flow = Flow::SetBase {
            feature: "suspense".into(),
            role: "api".into(),
        };
        let Step::Show(Overlay::Input { title, value, .. }) = flow.advance(Answer::Begin, &app)
        else {
            panic!("expected an input");
        };
        assert_eq!(title, "Base branch for api");
        assert_eq!(value, "main", "the change's recorded base");
        let Step::Show(Overlay::Input {
            error: Some(e),
            value,
            ..
        }) = flow.advance(Answer::Text("origin/dev".into()), &app)
        else {
            panic!("expected the input again with an error");
        };
        assert_eq!(e, "pass the branch name without origin/: dev");
        assert_eq!(value, "origin/dev", "typed text kept for editing");
        let Step::Show(Overlay::Input { error: Some(e), .. }) =
            flow.advance(Answer::Text("".into()), &app)
        else {
            panic!("expected an error for an empty base");
        };
        assert_eq!(e, "a branch name is required");
        assert_eq!(
            flow.advance(Answer::Text("dev".into()), &app),
            Step::Run(Action::SetBase {
                feature: "suspense".into(),
                role: "api".into(),
                branch: "dev".into()
            })
        );
    }

    #[test]
    fn worktree_lines_describe_owner_state_and_path() {
        let mut c = change("api", "feature/x", Stage::Working);
        let mut row = status_row("api", "feature/x", "working");
        assert_eq!(
            worktree_lines("api", &c, Some(&row)),
            vec![
                ("api  /wt/api/feature/x".to_string(), Tone::Plain),
                ("     hub-created · clean".to_string(), Tone::Dim),
            ]
        );
        row.dirty = true;
        c.worktree.as_mut().unwrap().owner = Owner::Adopted;
        assert_eq!(
            worktree_lines("api", &c, Some(&row))[1],
            ("     adopted · dirty".to_string(), Tone::Warn)
        );
        assert_eq!(
            worktree_lines("api", &c, None)[1],
            ("     adopted · unknown".to_string(), Tone::Dim)
        );
        c.worktree = None;
        assert_eq!(
            worktree_lines("api", &c, Some(&row)),
            vec![("api  main clone, not removed".to_string(), Tone::Dim)]
        );
    }

    #[test]
    fn merged_confirms_with_the_worktree_description_then_runs_with_the_chosen_force() {
        let app = on_feature();
        let mut flow = Flow::Merged {
            feature: "suspense".into(),
            role: "api".into(),
        };
        let Step::Show(Overlay::Confirm {
            title,
            lines,
            force,
            blocked,
        }) = flow.advance(Answer::Begin, &app)
        else {
            panic!("expected a confirm");
        };
        assert_eq!(title, "Merge api in suspense");
        assert!(force);
        assert!(blocked.is_none());
        let text: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text[0], "Close change     api: feature/x");
        assert_eq!(text[1], "Remove worktree  api  /wt/api/feature/x");
        assert_eq!(text[2].trim(), "hub-created · clean");
        assert!(
            text[2].starts_with("                 "),
            "continuation lines are indented past the label: {:?}",
            text[2]
        );
        assert_eq!(text[3], "Keep branch      feature/x");
        assert_eq!(
            flow.advance(Answer::Confirmed { force: true }, &app),
            Step::Run(Action::Merged {
                feature: "suspense".into(),
                role: "api".into(),
                force: true
            })
        );
    }

    #[test]
    fn merged_warns_about_dirty_and_adopted_worktrees() {
        let mut app = on_feature();
        let Report::Feature {
            mut feature,
            mut report,
        } = suspense()
        else {
            unreachable!()
        };
        feature.changes[0].worktree.as_mut().unwrap().owner = Owner::Adopted;
        report.rows[1].dirty = true;
        app.cache.insert(
            RowId::Feature("suspense".into()),
            Cached {
                report: Report::Feature { feature, report },
                at: std::time::Instant::now(),
            },
        );
        let mut flow = Flow::Merged {
            feature: "suspense".into(),
            role: "api".into(),
        };
        let Step::Show(Overlay::Confirm { lines, .. }) = flow.advance(Answer::Begin, &app) else {
            panic!()
        };
        assert!(
            lines.contains(&(
                "! api is dirty; merged will refuse without force".to_string(),
                Tone::Warn
            )),
            "{lines:?}"
        );
        assert!(
            lines.contains(&("! adopted worktree will be removed".to_string(), Tone::Warn)),
            "{lines:?}"
        );
    }

    #[test]
    fn finish_skips_the_session_kill_when_tmux_is_off() {
        let mut app = on_feature();
        app.tmux_on = false;
        let mut flow = Flow::Finish {
            feature: "suspense".into(),
        };
        let Step::Show(Overlay::Confirm { lines, .. }) = flow.advance(Answer::Begin, &app) else {
            panic!()
        };
        let text: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text[0], "Remove worktrees    api  /wt/api/feature/x");
        assert!(!text.iter().any(|t| t.contains("tmux")));
    }

    #[test]
    fn finish_previews_removals_warns_about_unmerged_roles_and_runs() {
        let app = on_feature();
        let mut flow = Flow::Finish {
            feature: "suspense".into(),
        };
        let Step::Show(Overlay::Confirm {
            title,
            lines,
            force,
            blocked,
        }) = flow.advance(Answer::Begin, &app)
        else {
            panic!()
        };
        assert_eq!(title, "Finish suspense");
        assert!(force && blocked.is_none());
        let text: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(text[0], "Kill tmux session   acme-suspense");
        assert_eq!(text[1], "Remove worktrees    api  /wt/api/feature/x");
        assert_eq!(text[2].trim(), "hub-created · clean");
        assert_eq!(
            text[3].trim(),
            "hub  /wt/hub/suspense",
            "the hub worktree is listed last"
        );
        assert!(text[3].starts_with("                    "), "{:?}", text[3]);
        assert_eq!(text[4].trim(), "hub-created · clean");
        assert_eq!(
            text[5], "Keep branches       api, ui, and hub branch suspense",
            "every role that ever had a change, then the hub branch"
        );
        assert!(
            lines.contains(&("! not merged: api (working)".to_string(), Tone::Warn)),
            "{lines:?}"
        );
        assert_eq!(
            flow.advance(Answer::Confirmed { force: false }, &app),
            Step::Run(Action::Finish {
                feature: "suspense".into(),
                force: false
            })
        );
    }

    #[test]
    fn finish_is_blocked_inside_the_features_own_session() {
        let mut app = on_feature();
        app.current_session = Some("acme-suspense".into());
        let mut flow = Flow::Finish {
            feature: "suspense".into(),
        };
        let Step::Show(Overlay::Confirm {
            blocked: Some(why), ..
        }) = flow.advance(Answer::Begin, &app)
        else {
            panic!()
        };
        assert!(
            why.starts_with("running inside tmux session acme-suspense"),
            "{why}"
        );
        assert!(
            why.contains("hub feature finish --feature suspense"),
            "{why}"
        );
    }

    #[test]
    fn finish_keep_branches_lists_each_role_once_in_first_seen_order() {
        let mut app = on_feature();
        let mut f = Feature::new("suspense", "acme-suspense");
        f.changes.push(change("api", "old-api", Stage::Merged));
        f.changes.push(change("ui", "feature/ui", Stage::Working));
        f.changes.push(change("api", "feature/x", Stage::Working));
        let report = StatusReport {
            feature: "suspense".into(),
            checkout: "acme-suspense".into(),
            status: FeatureStatus::Open,
            // `status` emits one row per change, in record order, after the
            // hub row: four rows for three changes.
            rows: vec![
                status_row("hub", "suspense", "-"),
                status_row("api", "old-api", "merged"),
                status_row("ui", "feature/ui", "working"),
                status_row("api", "feature/x", "working"),
            ],
            warnings: vec![],
            drift: false,
        };
        app.cache.insert(
            RowId::Feature("suspense".into()),
            Cached {
                report: Report::Feature { feature: f, report },
                at: std::time::Instant::now(),
            },
        );
        let mut flow = Flow::Finish {
            feature: "suspense".into(),
        };
        let Step::Show(Overlay::Confirm { lines, .. }) = flow.advance(Answer::Begin, &app) else {
            panic!()
        };
        assert!(
            lines.contains(&(
                "Keep branches       api, ui, and hub branch suspense".to_string(),
                Tone::Plain
            )),
            "{lines:?}"
        );
        let text: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            text.iter().any(|l| l.contains("api  /wt/api/feature/x")),
            "the open api change's row, not the merged one: {text:?}"
        );
        assert!(
            !text.iter().any(|l| l.contains("/wt/api/old-api")),
            "the merged api row must not be previewed: {text:?}"
        );
    }
}
