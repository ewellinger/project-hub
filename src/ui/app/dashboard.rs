//! Dashboard keys: selection, refresh, open workspace, pull.

use crossterm::event::{KeyCode, KeyEvent};
use hub::feature::FeatureStatus;

use super::flow::NewFeature;
use super::overlay::{DASHBOARD_HELP, Overlay};
use super::{Action, App, Effect, Flow, RowId, Screen};

impl App {
    pub(super) fn dashboard_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                // Quitting mid-action would kill the worker's process with
                // the op half done, so make it deliberate: Ctrl-C still goes
                // through, the same as Ctrl-C after a CLI op took the lock.
                if let Some(action) = &self.running {
                    let text = format!("busy: {action}…; Ctrl-C quits anyway");
                    self.say(text, true);
                    return Vec::new();
                }
                self.quit = true;
                Vec::new()
            }
            KeyCode::Char('n') => {
                if self.busy() {
                    return Vec::new();
                }
                self.start_flow(Flow::NewFeature(NewFeature::new()))
            }
            KeyCode::Up | KeyCode::Char('k') => self.select(self.selected.checked_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                let next = self.selected + 1;
                self.select((next < self.rows.len()).then_some(next))
            }
            KeyCode::Char('r') => vec![Effect::ReloadList],
            KeyCode::Enter => self.open_feature(),
            KeyCode::Char('a') => {
                let keep = self.selected_row().id.clone();
                self.show_finished = !self.show_finished;
                self.rebuild_rows(Some(&keep));
                Vec::new()
            }
            KeyCode::Char('o') => {
                let row = self.selected_row();
                match &row.summary {
                    Some(s) if s.status == FeatureStatus::Finished => {
                        let text = format!("feature '{}' is finished", s.name);
                        self.say(text, true);
                        Vec::new()
                    }
                    _ => vec![Effect::OpenWorkspace(row.id.clone())],
                }
            }
            KeyCode::Char('p') => {
                let row = self.selected_row();
                match &row.summary {
                    Some(s) if s.status == FeatureStatus::Finished => {
                        let text = format!("feature '{}' is finished", s.name);
                        self.say(text, true);
                        Vec::new()
                    }
                    _ => {
                        let row = row.id.clone();
                        self.run(Action::Pull { row })
                    }
                }
            }
            KeyCode::Char('?') => {
                if self.busy() {
                    return Vec::new();
                }
                self.overlay = Some(Overlay::help("keys", DASHBOARD_HELP));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn select(&mut self, index: Option<usize>) -> Vec<Effect> {
        match index {
            Some(i) if i != self.selected => {
                self.selected = i;
                self.request_selected()
            }
            _ => Vec::new(),
        }
    }

    fn open_feature(&mut self) -> Vec<Effect> {
        match self.selected_row().summary.as_ref().map(|s| s.name.clone()) {
            None => {
                self.say("base has no feature view".into(), true);
                Vec::new()
            }
            Some(name) => self.enter_feature(&name),
        }
    }

    /// Put the dashboard cursor on `name`'s row and open its view, so `Esc`
    /// lands back on that row; the status request is the same one `Enter`
    /// makes. Refused, with the reason in the footer, when the feature is
    /// finished or unknown. Shared by `Enter` and startup from a feature
    /// worktree (EW-71).
    pub fn enter_feature(&mut self, name: &str) -> Vec<Effect> {
        match self.feature_summary(name).map(|s| s.status) {
            None => {
                self.say(format!("no feature named '{name}'"), true);
                Vec::new()
            }
            Some(FeatureStatus::Finished) => {
                self.say(format!("feature '{name}' is finished"), true);
                Vec::new()
            }
            Some(FeatureStatus::Open) => {
                let id = RowId::Feature(name.to_string());
                self.select_row(&id);
                self.screen = Screen::Feature {
                    name: name.to_string(),
                    selected: None,
                };
                self.request(id)
            }
        }
    }
}
