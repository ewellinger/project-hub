//! Dashboard state and the pure update function. No terminal, no threads,
//! no git: everything here is unit-testable with synthetic events.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use hub::error::HubError;
use hub::feature::{Feature, FeatureStatus};
use hub::manifest::RepoSpec;
use hub::ops::list::FeatureSummary;
use hub::ops::status::{BaseStatusReport, StatusReport};
use hub::ops::suggest::BranchRow;

pub mod dashboard;
pub mod feature;
pub mod flow;
pub mod overlay;
pub use flow::{Flow, Step};
pub use overlay::{Outcome, Overlay, Preview, Tone};

/// Which screen the keys go to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    /// One open feature. `selected` is an index into `feature.changes`
    /// (changes are append-only, so it is a stable key); `None` means the
    /// first change in display order, so a fresh view lands on an open
    /// role and stays there across refreshes.
    Feature {
        name: String,
        selected: Option<usize>,
    },
}

/// A row of the list pane: the hub's base checkouts, or one feature.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowId {
    Base,
    Feature(String),
}

/// One hub op with its resolved arguments; runs under the lock on a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Start {
        name: String,
        repos: Vec<(String, Option<String>)>,
    },
    Add {
        feature: String,
        role: String,
        branch: String,
    },
    Review {
        feature: String,
        role: String,
        url: Option<String>,
    },
    SetBase {
        feature: String,
        role: String,
        branch: String,
    },
    Merged {
        feature: String,
        role: String,
        force: bool,
    },
    Finish {
        feature: String,
        force: bool,
    },
    /// `hub pull` for one row: the base checkouts, or a feature's open changes.
    Pull {
        row: RowId,
    },
}

impl Action {
    /// The feature the action changes; `None` for a start or a base pull.
    pub fn feature(&self) -> Option<&str> {
        match self {
            Action::Start { .. } | Action::Pull { row: RowId::Base } => None,
            Action::Add { feature, .. }
            | Action::Review { feature, .. }
            | Action::SetBase { feature, .. }
            | Action::Merged { feature, .. }
            | Action::Finish { feature, .. }
            | Action::Pull {
                row: RowId::Feature(feature),
            } => Some(feature),
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Start { name, .. } => write!(f, "start {name}"),
            Action::Add { feature, role, .. } => write!(f, "add {role} to {feature}"),
            Action::Review { role, .. } => write!(f, "review {role}"),
            Action::SetBase { role, .. } => write!(f, "set-base {role}"),
            Action::Merged { role, .. } => write!(f, "merged {role}"),
            Action::Finish { feature, .. } => write!(f, "finish {feature}"),
            Action::Pull { row: RowId::Base } => write!(f, "pull base"),
            Action::Pull {
                row: RowId::Feature(name),
            } => write!(f, "pull {name}"),
        }
    }
}

#[derive(Debug)]
pub enum Report {
    Base(BaseStatusReport),
    Feature {
        feature: Feature,
        report: StatusReport,
    },
}

/// What an overlay hands back when the user accepts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The first step of a flow, before any overlay was shown.
    Begin,
    Text(String),
    Picked(usize),
    PickedMany(Vec<usize>),
    Confirmed {
        force: bool,
    },
    Dismissed,
    /// A worker result a flow was waiting for. Fed by the flows added in
    /// Tasks 12-13.
    Loaded(Result<Loaded, String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loaded {
    /// `start::preflight` succeeded with this checkout name for `name`, the
    /// feature name it was asked to preflight.
    Checkout {
        name: String,
        checkout: String,
    },
    Branches {
        role: String,
        rows: Vec<BranchRow>,
    },
}

/// Everything that can happen to the app: a key, a timer tick, or a result
/// coming back from the event loop's side effects.
#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Tick,
    Report {
        row: RowId,
        r#gen: u64,
        result: Result<Report, HubError>,
    },
    List(Result<Vec<FeatureSummary>, HubError>),
    Opened(Result<PathBuf, HubError>),
    /// The clipboard worker finished; `Ok` carries the label of what was copied.
    Copied(Result<String, HubError>),
    Done {
        action: Action,
        result: Result<Vec<String>, HubError>,
    },
    Branches {
        role: String,
        result: Result<Vec<BranchRow>, HubError>,
    },
    Preflighted {
        name: String,
        result: Result<String, HubError>,
    },
}

/// What the event loop must do on the app's behalf. `update` never does
/// any of these itself.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Request {
        row: RowId,
        r#gen: u64,
    },
    OpenWorkspace(RowId),
    /// Put `text` on the clipboard; `label` names it in the footer.
    Copy {
        label: String,
        text: String,
    },
    ReloadList,
    Run(Action),
    LoadBranches {
        feature: Feature,
        role: String,
    },
    Preflight {
        name: String,
    },
}

#[derive(Debug)]
pub struct Row {
    pub id: RowId,
    /// `None` for the base row.
    pub summary: Option<FeatureSummary>,
}

pub struct Cached {
    pub report: Report,
    pub at: Instant,
}

pub struct Footer {
    pub text: String,
    pub is_error: bool,
    pub at: Instant,
}

impl Footer {
    const TTL: Duration = Duration::from_secs(3);

    pub fn visible(&self, now: Instant) -> bool {
        now.duration_since(self.at) < Self::TTL
    }
}

pub struct App {
    pub hub_name: String,
    pub screen: Screen,
    /// Manifest repos, in manifest order; filled by `ui::run`.
    pub repos: Vec<RepoSpec>,
    /// The hub's checkout template, for the wizard's preview.
    pub checkout_template: String,
    /// The tmux session hub runs inside, read once at startup.
    pub current_session: Option<String>,
    /// False when sessions are off; hides attach hints and the session kill.
    pub tmux_on: bool,
    /// Every feature the list returned; `rows` is the visible subset.
    features: Vec<FeatureSummary>,
    pub show_finished: bool,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Last good report per row.
    pub cache: HashMap<RowId, Cached>,
    /// Generation of the newest outstanding request per row.
    pub in_flight: HashMap<RowId, u64>,
    /// Last failure per row; cleared by the next success.
    pub errors: HashMap<RowId, String>,
    pub footer: Option<Footer>,
    pub spinner: usize,
    pub quit: bool,
    pub overlay: Option<Overlay>,
    pub flow: Option<Flow>,
    /// The action currently running on the worker, if any.
    pub running: Option<Action>,
    /// A feature to select once the next list lands.
    pub select_next: Option<String>,
    next_gen: u64,
}

impl App {
    pub fn new(hub_name: &str, features: Vec<FeatureSummary>) -> App {
        let mut app = App {
            hub_name: hub_name.to_string(),
            screen: Screen::Dashboard,
            repos: Vec::new(),
            checkout_template: String::new(),
            current_session: None,
            tmux_on: true,
            features: Vec::new(),
            show_finished: false,
            rows: Vec::new(),
            selected: 0,
            cache: HashMap::new(),
            in_flight: HashMap::new(),
            errors: HashMap::new(),
            footer: None,
            spinner: 0,
            quit: false,
            overlay: None,
            flow: None,
            running: None,
            select_next: None,
            next_gen: 0,
        };
        app.set_features(features);
        app
    }

    pub fn selected_row(&self) -> &Row {
        &self.rows[self.selected]
    }

    /// Request `id`'s status unless one is already in flight.
    pub fn request(&mut self, id: RowId) -> Vec<Effect> {
        if self.in_flight.contains_key(&id) {
            return Vec::new();
        }
        self.request_fresh(id)
    }

    /// Request `id`'s status even when one is in flight, superseding it. A
    /// request made before an action ran was answered from pre-action
    /// state, so after an action the refresh must not be skipped; the
    /// generation guard in `Event::Report` drops the older reply.
    fn request_fresh(&mut self, id: RowId) -> Vec<Effect> {
        self.next_gen += 1;
        self.in_flight.insert(id.clone(), self.next_gen);
        vec![Effect::Request {
            row: id,
            r#gen: self.next_gen,
        }]
    }

    /// Request the selected row's status unless one is already in flight.
    pub fn request_selected(&mut self) -> Vec<Effect> {
        let id = self.selected_row().id.clone();
        self.request(id)
    }

    /// The effects the event loop starts with. From a feature worktree
    /// (`cwd_feature`) the feature view opens first, as `Enter` on its row
    /// would; anywhere else, or when that feature is finished, the dashboard
    /// opens on the base row and requests it (EW-71).
    pub fn start(&mut self, cwd_feature: Option<&str>) -> Vec<Effect> {
        let mut effects = match cwd_feature {
            Some(name) => self.enter_feature(name),
            None => Vec::new(),
        };
        effects.extend(self.request_selected());
        effects
    }

    /// The listed summary of `name`, visible or not.
    pub fn feature_summary(&self, name: &str) -> Option<&FeatureSummary> {
        self.features.iter().find(|s| s.name == name)
    }

    pub fn update(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.key(key),
            Event::Key(_) => Vec::new(),
            Event::Tick => {
                self.spinner = self.spinner.wrapping_add(1);
                Vec::new()
            }
            Event::Report { row, r#gen, result } => {
                if !Self::is_known(&self.features, &row) {
                    self.in_flight.remove(&row);
                    return Vec::new();
                }
                if self.in_flight.get(&row) != Some(&r#gen) {
                    return Vec::new();
                }
                self.in_flight.remove(&row);
                match result {
                    Ok(report) => {
                        self.errors.remove(&row);
                        self.cache.insert(
                            row,
                            Cached {
                                report,
                                at: Instant::now(),
                            },
                        );
                    }
                    Err(err) => {
                        self.errors.insert(row, err.to_string());
                    }
                }
                Vec::new()
            }
            Event::List(Ok(features)) => {
                self.set_features(features);
                if let Some(name) = self.select_next.take() {
                    self.select_row(&RowId::Feature(name));
                }
                self.request_selected()
            }
            Event::List(Err(err)) => {
                self.say(err.to_string(), true);
                self.request_selected()
            }
            Event::Opened(Ok(path)) => {
                self.say(format!("Opened {}", path.display()), false);
                Vec::new()
            }
            Event::Opened(Err(err)) => {
                self.say(err.to_string(), true);
                Vec::new()
            }
            Event::Copied(Ok(label)) => {
                self.say(format!("copied {label}"), false);
                Vec::new()
            }
            Event::Copied(Err(err)) => {
                self.say(err.to_string(), true);
                Vec::new()
            }
            Event::Done { action, result } => self.done(action, result),
            Event::Branches { role, result } => self.feed_flow(Answer::Loaded(
                result
                    .map(|rows| Loaded::Branches { role, rows })
                    .map_err(|e| e.to_string()),
            )),
            Event::Preflighted { name, result } => self.feed_flow(Answer::Loaded(
                result
                    .map(|checkout| Loaded::Checkout { name, checkout })
                    .map_err(|e| e.to_string()),
            )),
        }
    }

    fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return Vec::new();
        }
        if let Some(overlay) = self.overlay.as_mut() {
            return match overlay.key(key) {
                Outcome::Stay => Vec::new(),
                Outcome::Cancel => {
                    self.overlay = None;
                    if self.flow.take().is_some() {
                        self.say("cancelled".into(), false);
                    }
                    Vec::new()
                }
                Outcome::Answer(answer) => {
                    self.overlay = None;
                    self.feed_flow(answer)
                }
            };
        }
        match self.screen {
            Screen::Dashboard => self.dashboard_key(key),
            Screen::Feature { .. } => self.feature_key(key),
        }
    }

    /// Begin a flow: store it and take its first step.
    pub fn start_flow(&mut self, flow: Flow) -> Vec<Effect> {
        self.flow = Some(flow);
        self.feed_flow(Answer::Begin)
    }

    /// Hand an overlay answer (or a worker result) to the open flow. A
    /// worker result arrives while the flow's spinner overlay is still up,
    /// so the overlay is cleared here and re-set by `Show`; `Ignore` puts
    /// the untouched overlay back for a stale result the flow didn't ask for.
    fn feed_flow(&mut self, answer: Answer) -> Vec<Effect> {
        let Some(mut flow) = self.flow.take() else {
            return Vec::new();
        };
        let overlay = self.overlay.take();
        match flow.advance(answer, self) {
            Step::Show(overlay) => {
                self.overlay = Some(overlay);
                self.flow = Some(flow);
                Vec::new()
            }
            Step::ShowAndLoad(overlay, effect) => {
                self.overlay = Some(overlay);
                self.flow = Some(flow);
                vec![effect]
            }
            Step::Run(action) => self.run(action),
            Step::Effect(effect) => vec![effect],
            Step::Cancel(message) => {
                self.say(message, true);
                Vec::new()
            }
            Step::Ignore => {
                self.overlay = overlay;
                self.flow = Some(flow);
                Vec::new()
            }
        }
    }

    /// Store a fresh list and rebuild the visible rows, keeping the selection
    /// on `keep` when that row is still visible.
    fn set_features(&mut self, features: Vec<FeatureSummary>) {
        self.features = features;
        let keep = self.rows.get(self.selected).map(|r| r.id.clone());
        self.rebuild_rows(keep.as_ref());
    }

    /// Base row first, then every feature that passes the finished filter.
    /// Cached reports for rows that survive stay; state for rows that left
    /// the list is pruned.
    pub fn rebuild_rows(&mut self, keep: Option<&RowId>) {
        let mut rows = vec![Row {
            id: RowId::Base,
            summary: None,
        }];
        rows.extend(
            self.features
                .iter()
                .filter(|s| self.show_finished || s.status == FeatureStatus::Open)
                .map(|s| Row {
                    id: RowId::Feature(s.name.clone()),
                    summary: Some(s.clone()),
                }),
        );
        self.rows = rows;
        if let Some(id) = keep {
            if !self.select_row(id) && self.selected >= self.rows.len() {
                self.selected = self.rows.len() - 1;
            }
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
        let features = &self.features;
        self.cache.retain(|id, _| Self::is_known(features, id));
        self.in_flight.retain(|id, _| Self::is_known(features, id));
        self.errors.retain(|id, _| Self::is_known(features, id));
    }

    /// Move the selection to the visible row with `id`.
    pub fn select_row(&mut self, id: &RowId) -> bool {
        match self.rows.iter().position(|r| &r.id == id) {
            Some(i) => {
                self.selected = i;
                true
            }
            None => false,
        }
    }

    /// Whether `id` is the base row or names a feature in `features`,
    /// regardless of whether it is currently visible. Takes `features`
    /// explicitly (rather than `&self`) so callers can call it while a
    /// different field of `self` is mutably borrowed, e.g. inside
    /// `HashMap::retain` over `self.cache`. Used to decide what survives
    /// `rebuild_rows`'s pruning and what a `Report` event is allowed to
    /// update, so the two checks cannot drift apart.
    fn is_known(features: &[FeatureSummary], id: &RowId) -> bool {
        match id {
            RowId::Base => true,
            RowId::Feature(name) => features.iter().any(|s| &s.name == name),
        }
    }

    pub(super) fn say(&mut self, text: String, is_error: bool) {
        self.footer = Some(Footer {
            text,
            is_error,
            at: Instant::now(),
        });
    }

    /// Footer `busy: …` and `true` while an action runs.
    pub(super) fn busy(&mut self) -> bool {
        match &self.running {
            Some(action) => {
                let text = format!("busy: {action}…");
                self.say(text, true);
                true
            }
            None => false,
        }
    }

    /// Dispatch `action` unless one is already running.
    pub fn run(&mut self, action: Action) -> Vec<Effect> {
        if self.busy() {
            return Vec::new();
        }
        self.running = Some(action.clone());
        vec![Effect::Run(action)]
    }

    fn done(&mut self, action: Action, result: Result<Vec<String>, HubError>) -> Vec<Effect> {
        self.running = None;
        let ok = result.is_ok();
        match result {
            Ok(lines) if lines.len() == 1 => self.say(lines[0].clone(), false),
            Ok(lines) => self.overlay = Some(Overlay::result(action.to_string(), lines, false)),
            Err(err) => {
                let lines = err.to_string().lines().map(str::to_string).collect();
                self.overlay = Some(Overlay::result(action.to_string(), lines, true));
            }
        }
        match &action {
            Action::Start { name, .. } => {
                if ok {
                    self.select_next = Some(name.clone());
                    vec![Effect::ReloadList]
                } else {
                    Vec::new()
                }
            }
            Action::Finish { feature, .. } if ok => {
                if self.feature_name() == Some(feature.as_str()) {
                    self.screen = Screen::Dashboard;
                }
                vec![Effect::ReloadList]
            }
            Action::Pull { row } => self.request_fresh(row.clone()),
            other => {
                let name = other.feature().expect("not a start").to_string();
                self.request_fresh(RowId::Feature(name))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::overlay::{DASHBOARD_HELP, FEATURE_HELP};
    use super::*;
    use hub::feature::{FeatureStatus, Stage};
    use hub::ops::list::RoleSummary;
    use hub::ops::status::StatusRow;

    fn summary(name: &str, status: FeatureStatus) -> FeatureSummary {
        FeatureSummary {
            name: name.into(),
            checkout: name.into(),
            status,
            roles: vec![RoleSummary {
                role: "api".into(),
                branch: format!("feature/{name}"),
                stage: Stage::Working,
            }],
        }
    }

    fn app() -> App {
        App::new(
            "acme",
            vec![
                summary("alpha", FeatureStatus::Open),
                summary("old", FeatureStatus::Finished),
            ],
        )
    }

    fn review_action() -> Action {
        Action::Review {
            feature: "alpha".into(),
            role: "api".into(),
            url: None,
        }
    }

    #[test]
    fn run_dispatches_once_and_refuses_while_running() {
        let mut a = app();
        assert_eq!(a.run(review_action()), vec![Effect::Run(review_action())]);
        assert_eq!(a.running, Some(review_action()));
        assert!(a.run(review_action()).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "busy: review api…")
        );
    }

    #[test]
    fn a_one_line_success_goes_to_the_footer_and_refreshes_the_feature() {
        let mut a = app();
        a.run(review_action());
        let effects = a.update(Event::Done {
            action: review_action(),
            result: Ok(vec!["api: feature/alpha is in review".into()]),
        });
        assert!(a.running.is_none());
        assert!(a.overlay.is_none());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| !f.is_error && f.text == "api: feature/alpha is in review")
        );
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("alpha".into()),
                r#gen: 1
            }]
        );
    }

    /// A report requested before the action ran carries pre-action data, so
    /// the refresh after the action must supersede it, not be skipped.
    #[test]
    fn a_finished_action_refreshes_over_a_request_already_in_flight() {
        let mut a = app();
        let id = RowId::Feature("alpha".into());
        assert_eq!(
            a.request(id.clone()),
            vec![Effect::Request {
                row: id.clone(),
                r#gen: 1
            }]
        );
        a.run(review_action());
        let effects = a.update(Event::Done {
            action: review_action(),
            result: Ok(vec!["api: feature/alpha is in review".into()]),
        });
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: id.clone(),
                r#gen: 2
            }]
        );
        a.update(Event::Report {
            row: id.clone(),
            r#gen: 1,
            result: Ok(empty_feature_report("alpha")),
        });
        assert!(!a.cache.contains_key(&id), "pre-action report cached");
        assert_eq!(a.in_flight.get(&id), Some(&2), "the fresh request stands");
    }

    #[test]
    fn longer_results_and_errors_open_a_result_overlay() {
        let mut a = app();
        a.run(review_action());
        a.update(Event::Done {
            action: review_action(),
            result: Ok(vec!["a".into(), "b".into()]),
        });
        assert_eq!(
            a.overlay,
            Some(Overlay::result(
                "review api".into(),
                vec!["a".into(), "b".into()],
                false
            ))
        );
        a.overlay = None;
        a.run(review_action());
        a.update(Event::Done {
            action: review_action(),
            result: Err(HubError::Precondition(
                "no open change for role 'api'".into(),
            )),
        });
        assert_eq!(
            a.overlay,
            Some(Overlay::result(
                "review api".into(),
                vec!["no open change for role 'api'".into()],
                true
            ))
        );
    }

    #[test]
    fn a_finished_feature_returns_to_the_dashboard_and_reloads() {
        let mut a = app();
        a.screen = Screen::Feature {
            name: "alpha".into(),
            selected: None,
        };
        let finish = Action::Finish {
            feature: "alpha".into(),
            force: false,
        };
        a.run(finish.clone());
        let effects = a.update(Event::Done {
            action: finish,
            result: Ok(vec![
                "Killed tmux session alpha".into(),
                "Feature 'alpha' finished; no branches were deleted".into(),
            ]),
        });
        assert_eq!(a.screen, Screen::Dashboard);
        assert_eq!(effects, vec![Effect::ReloadList]);
    }

    #[test]
    fn a_started_feature_is_selected_when_the_list_arrives() {
        let mut a = app();
        let start = Action::Start {
            name: "beta".into(),
            repos: vec![],
        };
        a.run(start.clone());
        let effects = a.update(Event::Done {
            action: start,
            result: Ok(vec!["Started beta".into()]),
        });
        assert_eq!(effects, vec![Effect::ReloadList]);
        a.update(Event::List(Ok(vec![
            summary("alpha", FeatureStatus::Open),
            summary("beta", FeatureStatus::Open),
        ])));
        assert_eq!(a.selected_row().id, RowId::Feature("beta".into()));
    }

    #[test]
    fn quitting_the_dashboard_is_refused_while_an_action_runs() {
        let mut a = app();
        a.run(review_action());
        assert!(a.update(key(KeyCode::Char('q'))).is_empty());
        assert!(!a.quit);
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "busy: review api…; Ctrl-C quits anyway")
        );
        assert!(a.update(key(KeyCode::Esc)).is_empty());
        assert!(!a.quit);
        a.update(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(a.quit, "Ctrl-C quits anyway");
    }

    #[test]
    fn action_display_names_the_op_and_its_target() {
        assert_eq!(
            Action::Start {
                name: "x".into(),
                repos: vec![]
            }
            .to_string(),
            "start x"
        );
        assert_eq!(
            Action::Add {
                feature: "x".into(),
                role: "ui".into(),
                branch: "b".into()
            }
            .to_string(),
            "add ui to x"
        );
        assert_eq!(review_action().to_string(), "review api");
        assert_eq!(
            Action::SetBase {
                feature: "x".into(),
                role: "api".into(),
                branch: "dev".into()
            }
            .to_string(),
            "set-base api"
        );
        assert_eq!(
            Action::Merged {
                feature: "x".into(),
                role: "api".into(),
                force: true
            }
            .to_string(),
            "merged api"
        );
        assert_eq!(
            Action::Finish {
                feature: "x".into(),
                force: false
            }
            .to_string(),
            "finish x"
        );
        assert_eq!(Action::Pull { row: RowId::Base }.to_string(), "pull base");
        assert_eq!(
            Action::Pull {
                row: RowId::Feature("x".into())
            }
            .to_string(),
            "pull x"
        );
        assert_eq!(Action::Pull { row: RowId::Base }.feature(), None);
        assert_eq!(
            Action::Pull {
                row: RowId::Feature("x".into())
            }
            .feature(),
            Some("x")
        );
    }

    #[test]
    fn p_pulls_the_selected_row_and_refreshes_it_when_done() {
        let mut a = app();
        let pull_base = Action::Pull { row: RowId::Base };
        assert_eq!(
            a.update(key(KeyCode::Char('p'))),
            vec![Effect::Run(pull_base.clone())]
        );
        assert_eq!(a.running, Some(pull_base.clone()));
        assert!(a.update(key(KeyCode::Char('p'))).is_empty(), "busy");
        let effects = a.update(Event::Done {
            action: pull_base,
            result: Ok(vec![
                "api: master fast-forwarded 2 commits".into(),
                "ui: develop is up to date".into(),
            ]),
        });
        assert!(a.running.is_none());
        assert!(
            matches!(&a.overlay, Some(Overlay::Result { title, is_error: false, .. }) if title == "pull base"),
            "{:?}",
            a.overlay
        );
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Base,
                r#gen: 1
            }],
            "the base row is refreshed even though nothing was in flight"
        );
        a.overlay = None;

        a.update(key(KeyCode::Char('j'))); // alpha, requests gen 2
        let pull_alpha = Action::Pull {
            row: RowId::Feature("alpha".into()),
        };
        assert_eq!(
            a.update(key(KeyCode::Char('p'))),
            vec![Effect::Run(pull_alpha.clone())]
        );
        let effects = a.update(Event::Done {
            action: pull_alpha,
            result: Ok(vec!["api: feature/alpha is up to date".into()]),
        });
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| !f.is_error && f.text == "api: feature/alpha is up to date")
        );
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("alpha".into()),
                r#gen: 3
            }],
            "supersedes the request already in flight"
        );
    }

    #[test]
    fn p_is_refused_on_a_finished_feature() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        a.update(key(KeyCode::Char('j')));
        a.update(key(KeyCode::Char('j'))); // old
        assert!(a.update(key(KeyCode::Char('p'))).is_empty());
        assert!(a.running.is_none());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "feature 'old' is finished")
        );
    }

    #[test]
    fn p_in_the_feature_view_pulls_that_feature() {
        let mut a = crate::ui::app::feature::tests::on_feature();
        assert_eq!(
            a.update(key(KeyCode::Char('p'))),
            vec![Effect::Run(Action::Pull {
                row: RowId::Feature("suspense".into())
            })]
        );
        assert!(matches!(a.screen, Screen::Feature { .. }));
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn empty_feature_report(name: &str) -> Report {
        Report::Feature {
            feature: Feature::new(name, name),
            report: StatusReport {
                feature: name.into(),
                checkout: name.into(),
                status: FeatureStatus::Open,
                rows: vec![],
                warnings: vec![],
                drift: false,
            },
        }
    }

    fn base_report() -> Report {
        Report::Base(BaseStatusReport {
            hub: "acme".into(),
            rows: vec![StatusRow {
                role: "hub".into(),
                branch: "main".into(),
                stage: "-".into(),
                path: PathBuf::from("/p/acme"),
                exists: true,
                registered: true,
                branch_matches: true,
                dirty: false,
                ahead: Some(0),
                behind: Some(0),
                base: None,
                custom_base: false,
                base_behind: None,
                hint: None,
                review_url: None,
                drift: false,
            }],
            warnings: vec![],
            drift: false,
        })
    }

    #[test]
    fn startup_from_a_feature_worktree_opens_that_feature_with_the_cursor_on_it() {
        let mut a = app();
        let effects = a.start(Some("alpha"));
        assert_eq!(
            a.screen,
            Screen::Feature {
                name: "alpha".into(),
                selected: None
            }
        );
        assert_eq!(a.selected_row().id, RowId::Feature("alpha".into()));
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("alpha".into()),
                r#gen: 1
            }],
            "the feature is requested once, not again for the selected row"
        );
        assert!(a.footer.is_none());
        a.update(key(KeyCode::Esc));
        assert_eq!(a.screen, Screen::Dashboard);
        assert_eq!(a.selected, 1, "Esc leaves the cursor on alpha");
    }

    #[test]
    fn startup_from_a_finished_or_unknown_feature_stays_on_the_dashboard() {
        for name in ["old", "ghost"] {
            let mut a = app();
            let effects = a.start(Some(name));
            assert_eq!(a.screen, Screen::Dashboard, "{name}");
            assert_eq!(a.selected, 0, "{name}");
            assert_eq!(
                effects,
                vec![Effect::Request {
                    row: RowId::Base,
                    r#gen: 1
                }],
                "{name}"
            );
        }
        let mut a = app();
        a.start(Some("old"));
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "feature 'old' is finished")
        );
    }

    #[test]
    fn startup_without_a_cwd_feature_requests_the_base_row() {
        let mut a = app();
        assert_eq!(
            a.start(None),
            vec![Effect::Request {
                row: RowId::Base,
                r#gen: 1
            }]
        );
        assert_eq!(a.screen, Screen::Dashboard);
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn rows_are_base_then_features_and_start_selected_on_base() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        assert_eq!(a.rows.len(), 3);
        assert_eq!(a.rows[0].id, RowId::Base);
        assert_eq!(a.rows[1].id, RowId::Feature("alpha".into()));
        assert_eq!(a.selected, 0);
    }

    #[test]
    fn request_selected_emits_once_until_the_report_lands() {
        let mut a = app();
        let first = a.request_selected();
        assert_eq!(
            first,
            vec![Effect::Request {
                row: RowId::Base,
                r#gen: 1
            }]
        );
        assert!(a.request_selected().is_empty(), "already in flight");
        let effects = a.update(Event::Report {
            row: RowId::Base,
            r#gen: 1,
            result: Ok(base_report()),
        });
        assert!(effects.is_empty());
        assert!(a.cache.contains_key(&RowId::Base));
        assert!(!a.in_flight.contains_key(&RowId::Base));
        assert_eq!(
            a.request_selected(),
            vec![Effect::Request {
                row: RowId::Base,
                r#gen: 2
            }]
        );
    }

    #[test]
    fn moving_the_selection_clamps_and_requests_the_new_row() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        assert!(a.update(key(KeyCode::Up)).is_empty(), "clamped at the top");
        assert_eq!(a.selected, 0);
        let effects = a.update(key(KeyCode::Char('j')));
        assert_eq!(a.selected, 1);
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("alpha".into()),
                r#gen: 1
            }]
        );
        a.update(key(KeyCode::Down));
        assert_eq!(a.selected, 2);
        assert!(
            a.update(key(KeyCode::Down)).is_empty(),
            "clamped at the bottom"
        );
        assert_eq!(a.selected, 2);
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.selected, 1);
    }

    #[test]
    fn stale_and_unknown_reports_are_dropped() {
        let mut a = app();
        a.request_selected(); // gen 1 for Base
        a.in_flight.insert(RowId::Base, 2); // a newer request superseded it
        a.update(Event::Report {
            row: RowId::Base,
            r#gen: 1,
            result: Ok(base_report()),
        });
        assert!(!a.cache.contains_key(&RowId::Base), "stale report ignored");
        assert_eq!(a.in_flight.get(&RowId::Base), Some(&2));
        a.update(Event::Report {
            row: RowId::Feature("ghost".into()),
            r#gen: 9,
            result: Ok(base_report()),
        });
        assert!(!a.cache.contains_key(&RowId::Feature("ghost".into())));
    }

    #[test]
    fn a_failed_report_records_the_error_and_keeps_the_cache() {
        let mut a = app();
        a.request_selected();
        a.update(Event::Report {
            row: RowId::Base,
            r#gen: 1,
            result: Ok(base_report()),
        });
        a.request_selected();
        a.update(Event::Report {
            row: RowId::Base,
            r#gen: 2,
            result: Err(HubError::Precondition("clone missing".into())),
        });
        assert_eq!(
            a.errors.get(&RowId::Base).map(String::as_str),
            Some("clone missing")
        );
        assert!(a.cache.contains_key(&RowId::Base));
        a.request_selected();
        a.update(Event::Report {
            row: RowId::Base,
            r#gen: 3,
            result: Ok(base_report()),
        });
        assert!(!a.errors.contains_key(&RowId::Base), "cleared on success");
    }

    #[test]
    fn refresh_reloads_the_list_then_requests_the_selected_row() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        a.update(key(KeyCode::Char('j'))); // requests alpha, gen 1
        a.update(Event::Report {
            row: RowId::Feature("alpha".into()),
            r#gen: 1,
            result: Ok(base_report()),
        }); // alpha is no longer in flight
        a.update(key(KeyCode::Char('j'))); // on "old", requests gen 2
        assert_eq!(a.update(key(KeyCode::Char('r'))), vec![Effect::ReloadList]);
        let effects = a.update(Event::List(Ok(vec![summary("alpha", FeatureStatus::Open)])));
        assert_eq!(a.rows.len(), 2, "old is gone");
        assert_eq!(a.selected, 1, "selection clamped to the last row");
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("alpha".into()),
                r#gen: 3
            }]
        );
        let effects = a.update(Event::List(Err(HubError::Precondition("bad json".into()))));
        assert_eq!(a.rows.len(), 2, "previous list kept");
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "bad json")
        );
        assert!(effects.is_empty(), "alpha is still in flight");
    }

    #[test]
    fn reload_prunes_state_for_vanished_rows() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        a.update(key(KeyCode::Char('j'))); // requests alpha, gen 1
        a.update(Event::Report {
            row: RowId::Feature("alpha".into()),
            r#gen: 1,
            result: Ok(base_report()),
        }); // alpha is cached
        a.update(key(KeyCode::Char('j'))); // requests "old", gen 2, left in flight
        let effects = a.update(Event::List(Ok(vec![summary("alpha", FeatureStatus::Open)])));
        assert!(a.cache.contains_key(&RowId::Feature("alpha".into())));
        assert!(!a.in_flight.contains_key(&RowId::Feature("old".into())));
        assert!(a.errors.is_empty());
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("alpha".into()),
                r#gen: 3
            }]
        );
    }

    #[test]
    fn open_is_refused_for_a_finished_feature_and_reports_results() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        a.update(key(KeyCode::Char('j')));
        assert_eq!(
            a.update(key(KeyCode::Char('o'))),
            vec![Effect::OpenWorkspace(RowId::Feature("alpha".into()))]
        );
        a.update(Event::Opened(Ok(PathBuf::from("/w/alpha.code-workspace"))));
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| !f.is_error && f.text == "Opened /w/alpha.code-workspace")
        );
        a.update(key(KeyCode::Char('j')));
        assert!(a.update(key(KeyCode::Char('o'))).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "feature 'old' is finished")
        );
        a.update(Event::Opened(Err(HubError::Precondition("no code".into()))));
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "no code")
        );
    }

    #[test]
    fn finished_features_are_hidden_until_toggled() {
        let mut a = app();
        assert_eq!(a.rows.len(), 2, "base + alpha; old is finished");
        a.update(key(KeyCode::Char('j'))); // alpha
        a.update(key(KeyCode::Char('a')));
        assert_eq!(a.rows.len(), 3);
        assert_eq!(
            a.selected_row().id,
            RowId::Feature("alpha".into()),
            "selection kept"
        );
        a.update(key(KeyCode::Char('j'))); // old
        a.update(key(KeyCode::Char('a')));
        assert_eq!(a.rows.len(), 2);
        assert_eq!(a.selected, 1, "clamped onto alpha when old vanished");
    }

    #[test]
    fn select_row_finds_visible_rows_only() {
        let mut a = app();
        assert!(a.select_row(&RowId::Feature("alpha".into())));
        assert_eq!(a.selected, 1);
        assert!(!a.select_row(&RowId::Feature("old".into())), "hidden");
        assert_eq!(a.selected, 1);
    }

    #[test]
    fn a_report_for_a_hidden_feature_is_still_cached() {
        let mut a = app();
        a.show_finished = true;
        a.rebuild_rows(None);
        a.update(key(KeyCode::Char('j'))); // alpha
        let effects = a.update(key(KeyCode::Char('j'))); // old, requests gen N
        let r#gen = match &effects[..] {
            [Effect::Request { r#gen, .. }] => *r#gen,
            other => panic!("unexpected effects {other:?}"),
        };
        a.update(key(KeyCode::Char('a'))); // hide finished features, old vanishes
        a.update(Event::Report {
            row: RowId::Feature("old".into()),
            r#gen,
            result: Ok(base_report()),
        });
        assert!(
            a.cache.contains_key(&RowId::Feature("old".into())),
            "report for a hidden-but-known feature is still cached"
        );
        assert!(!a.in_flight.contains_key(&RowId::Feature("old".into())));
        a.update(key(KeyCode::Char('a'))); // show finished features again
        assert!(a.rows.iter().any(|r| r.id == RowId::Feature("old".into())));
        assert!(
            a.cache.contains_key(&RowId::Feature("old".into())),
            "cache survived the round trip"
        );
    }

    #[test]
    fn quit_keys_and_ticks() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut a = app();
            a.update(key(code));
            assert!(a.quit);
        }
        let mut a = app();
        a.update(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(a.quit);
        let mut a = app();
        a.update(Event::Tick);
        a.update(Event::Tick);
        assert_eq!(a.spinner, 2);
        assert!(
            a.update(key(KeyCode::Char('x'))).is_empty(),
            "unknown key is a no-op"
        );
        assert!(!a.quit);
    }

    #[test]
    fn released_keys_are_ignored() {
        let mut a = app();
        let mut k = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        k.kind = KeyEventKind::Release;
        a.update(Event::Key(k));
        assert!(!a.quit);
    }

    #[test]
    fn an_open_overlay_takes_every_key_and_a_result_dismisses() {
        let mut a = app();
        a.overlay = Some(Overlay::result(
            "x".into(),
            vec!["a".into(), "b".into()],
            false,
        ));
        assert!(a.update(key(KeyCode::Char('j'))).is_empty());
        assert_eq!(a.selected, 0, "j went to the overlay, not the list");
        assert!(!a.quit);
        a.update(key(KeyCode::Enter));
        assert!(a.overlay.is_none());
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.selected, 1);
    }

    #[test]
    fn ctrl_c_quits_even_under_an_overlay() {
        let mut a = app();
        a.overlay = Some(Overlay::input("x".into(), String::new(), None));
        a.update(Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(a.quit);
    }

    #[test]
    fn a_branches_event_reaches_the_flow() {
        let mut a = crate::ui::app::feature::tests::on_feature();
        a.repos = vec![hub::manifest::RepoSpec {
            role: "ui".into(),
            clone: "ui-clone".into(),
            remote: "origin".into(),
            base: "main".into(),
            branch_template: "{feature}".into(),
            description: String::new(),
        }];
        a.update(key(KeyCode::Char('+')));
        let effects = a.update(key(KeyCode::Enter)); // picks ui
        assert!(matches!(effects[..], [Effect::LoadBranches { .. }]));
        a.update(Event::Branches {
            role: "ui".into(),
            result: Ok(vec![hub::ops::suggest::BranchRow::Other]),
        });
        assert!(matches!(
            a.overlay,
            Some(Overlay::Select { loading: false, .. })
        ));
        a.update(Event::Branches {
            role: "ui".into(),
            result: Err(HubError::Precondition("late".into())),
        });
        assert!(a.flow.is_none(), "an error ends the flow");
        assert!(a.footer.as_ref().is_some_and(|f| f.text == "late"));
    }

    #[test]
    fn n_starts_the_wizard_and_preflighted_feeds_it() {
        let mut a = app();
        a.checkout_template = "{feature}".into();
        assert!(a.update(key(KeyCode::Char('n'))).is_empty());
        assert!(
            matches!(a.overlay, Some(Overlay::Input { ref title, .. }) if title == "New feature")
        );
        for c in "beta".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        assert_eq!(
            a.update(key(KeyCode::Enter)),
            vec![Effect::Preflight {
                name: "beta".into()
            }]
        );
        a.update(Event::Preflighted {
            name: "beta".into(),
            result: Ok("beta".into()),
        });
        assert!(
            matches!(a.overlay, Some(Overlay::Confirm { .. })),
            "no repos: straight to the summary"
        );
        a.running = Some(review_action());
        assert!(
            a.update(key(KeyCode::Enter)).is_empty(),
            "run refused while busy"
        );
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text == "busy: review api…")
        );
    }

    #[test]
    fn n_is_refused_while_an_action_runs() {
        let mut a = app();
        a.running = Some(review_action());
        assert!(a.update(key(KeyCode::Char('n'))).is_empty());
        assert!(a.overlay.is_none());
    }

    #[test]
    fn a_stale_preflighted_leaves_the_typed_name_alone() {
        let mut a = app();
        a.update(key(KeyCode::Char('n')));
        for c in "ab".chars() {
            a.update(key(KeyCode::Char(c)));
        }
        a.update(Event::Preflighted {
            name: "zz".into(),
            result: Ok("zz".into()),
        });
        assert!(matches!(a.overlay, Some(Overlay::Input { ref value, .. }) if value == "ab"));
        assert!(a.flow.is_some(), "the wizard is still open");
    }

    #[test]
    fn footer_is_visible_for_three_seconds() {
        let f = Footer {
            text: "x".into(),
            is_error: false,
            at: Instant::now(),
        };
        assert!(f.visible(f.at + Duration::from_secs(2)));
        assert!(!f.visible(f.at + Duration::from_secs(4)));
    }

    #[test]
    fn question_mark_toggles_help_on_both_screens_and_is_refused_while_busy() {
        let mut a = app();
        assert!(a.update(key(KeyCode::Char('?'))).is_empty());
        assert!(
            matches!(&a.overlay, Some(Overlay::Help { rows, .. }) if rows.len() == DASHBOARD_HELP.len()),
            "{:?}",
            a.overlay
        );
        // Other keys are inert; ? closes without a "cancelled" footer.
        a.update(key(KeyCode::Char('j')));
        assert!(matches!(a.overlay, Some(Overlay::Help { .. })));
        assert_eq!(a.selected, 0, "j did not reach the dashboard");
        a.update(key(KeyCode::Char('?')));
        assert!(a.overlay.is_none());
        assert!(a.footer.is_none());
        for code in [KeyCode::Esc, KeyCode::Char('q')] {
            a.update(key(KeyCode::Char('?')));
            a.update(key(code));
            assert!(a.overlay.is_none(), "{code:?} closes help");
        }
        assert!(!a.quit, "q closed the overlay, it did not quit");

        let mut a = crate::ui::app::feature::tests::on_feature();
        a.update(key(KeyCode::Char('?')));
        assert!(
            matches!(&a.overlay, Some(Overlay::Help { rows, .. }) if rows.len() == FEATURE_HELP.len())
        );
        a.update(key(KeyCode::Esc));
        assert!(a.overlay.is_none());
        assert!(
            matches!(a.screen, Screen::Feature { .. }),
            "Esc closed help, not the view"
        );

        a.running = Some(Action::Finish {
            feature: "suspense".into(),
            force: false,
        });
        a.update(key(KeyCode::Char('?')));
        assert!(a.overlay.is_none());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text.starts_with("busy: "))
        );
    }

    #[test]
    fn copied_reports_the_label_or_the_error_in_the_footer() {
        let mut a = app();
        a.update(Event::Copied(Ok("branch".into())));
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| !f.is_error && f.text == "copied branch")
        );
        a.update(Event::Copied(Err(HubError::Precondition(
            "no clipboard available".into(),
        ))));
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text.contains("no clipboard available"))
        );
    }
}
