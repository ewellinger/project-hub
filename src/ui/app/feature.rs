//! Feature view: one open feature, keys act on the selected role.

use crossterm::event::{KeyCode, KeyEvent};
use hub::feature::{Change, Feature};
use hub::ops::status::{StatusReport, StatusRow};

use super::flow::AddRole;
use super::overlay::{FEATURE_HELP, Overlay};
use super::{Action, App, Effect, Flow, RowId, Screen, Selected};

/// Row indices of a feature report in display order: the hub row (row 0),
/// then changes whose stage is not `merged`, then merged ones, each group in
/// record order. Pure; both views and the cursor use it, so the
/// table and the keys always agree on which change a row is.
pub fn display_order(rows: &[StatusRow]) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::with_capacity(rows.len());
    if !rows.is_empty() {
        order.push(0);
    }
    order.extend((1..rows.len()).filter(|&i| rows[i].stage != "merged"));
    order.extend((1..rows.len()).filter(|&i| rows[i].stage == "merged"));
    order
}

impl App {
    /// The feature the view shows, when on the feature screen.
    pub fn feature_name(&self) -> Option<&str> {
        match &self.screen {
            Screen::Feature { name, .. } => Some(name),
            Screen::Dashboard => None,
        }
    }

    /// The cached record and report of the current feature.
    pub fn feature_report(&self) -> Option<(&Feature, &StatusReport)> {
        let name = self.feature_name()?;
        match &self.cache.get(&RowId::Feature(name.to_string()))?.report {
            super::Report::Feature { feature, report } => Some((feature, report)),
            super::Report::Base(_) => None,
        }
    }

    /// A feature report's rows after the hub worktree row: one per change,
    /// in record order, then one per frozen role.
    pub fn change_rows(report: &StatusReport) -> &[StatusRow] {
        report.rows.get(1..).unwrap_or(&[])
    }

    /// Row indices after the hub row, in display order: open changes and
    /// frozen references first, then merged changes. Row `i + 1` is change
    /// `i`, or a frozen reference when `i >= feature.changes.len()`.
    pub fn change_order(report: &StatusReport) -> Vec<usize> {
        display_order(&report.rows)
            .into_iter()
            .filter(|&row| row > 0)
            .map(|row| row - 1)
            .collect()
    }

    /// The cursor's index into the report's rows after the hub row. An
    /// index below `feature.changes.len()` is that change; the rest are
    /// frozen reference rows, which `status` appends after the changes.
    /// A stored change resolves to its index while it is in the record, a
    /// stored frozen role to wherever its row is now; either one that is
    /// gone falls back to the last row in display order. The first row
    /// when none was chosen yet; `None` without a report.
    pub fn cursor(&self) -> Option<usize> {
        let Screen::Feature { selected, .. } = &self.screen else {
            return None;
        };
        let (feature, report) = self.feature_report()?;
        let order = Self::change_order(report);
        let rows = Self::change_rows(report);
        let found = match selected {
            None => return order.first().copied(),
            Some(Selected::Change(i)) => (*i < feature.changes.len()).then_some(*i),
            Some(Selected::Frozen(role)) => {
                (feature.changes.len()..rows.len()).find(|&i| rows[i].role == *role)
            }
        };
        found.or_else(|| order.last().copied())
    }

    /// The change under the cursor and its status row. The mapping is
    /// positional: `status` emits one row per change in `feature.changes`
    /// order after the hub row (frozen rows come after them), and returns
    /// `Err` if any row fails, so row `i + 1` is always change `i`. Looking the change up by the row's role
    /// would pick the wrong one for a role that was merged and reopened.
    pub fn selected_change(&self) -> Option<(&Change, &StatusRow)> {
        let i = self.cursor()?;
        let (feature, report) = self.feature_report()?;
        let row = Self::change_rows(report).get(i)?;
        let change = feature.changes.get(i)?;
        Some((change, row))
    }

    /// The status row under the cursor: a change's row, or a frozen
    /// reference row.
    pub fn selected_role_row(&self) -> Option<&StatusRow> {
        let i = self.cursor()?;
        let (_, report) = self.feature_report()?;
        Self::change_rows(report).get(i)
    }

    pub(super) fn feature_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let name = self.feature_name().expect("feature screen").to_string();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Dashboard;
                vec![Effect::ReloadList]
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_role(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_role(1),
            KeyCode::Char('r') => self.request(RowId::Feature(name)),
            KeyCode::Char('o') => vec![Effect::OpenWorkspace(RowId::Feature(name))],
            KeyCode::Char('p') => self.run(Action::Pull {
                row: RowId::Feature(name),
            }),
            KeyCode::Char('v') => match self.role_action() {
                Some((feature, role)) => self.start_flow(Flow::Review { feature, role }),
                None => Vec::new(),
            },
            KeyCode::Char('b') => match self.role_action() {
                Some((feature, role)) => self.start_flow(Flow::SetBase { feature, role }),
                None => Vec::new(),
            },
            KeyCode::Char('m') => match self.role_action() {
                Some((feature, role)) => self.start_flow(Flow::Merged { feature, role }),
                None => Vec::new(),
            },
            KeyCode::Char('f') => {
                if self.busy() {
                    return Vec::new();
                }
                if self.feature_report().is_none() {
                    self.say("status not loaded yet; press r".into(), true);
                    return Vec::new();
                }
                self.start_flow(Flow::Finish { feature: name })
            }
            KeyCode::Char('+') => {
                if self.busy() {
                    return Vec::new();
                }
                if self.feature_report().is_none() {
                    self.say("status not loaded yet; press r".into(), true);
                    return Vec::new();
                }
                self.start_flow(Flow::AddRole(AddRole::new(name)))
            }
            KeyCode::Char('c') => {
                if self.busy() {
                    return Vec::new();
                }
                match self.cursor() {
                    Some(change) => self.start_flow(Flow::Copy {
                        feature: name,
                        change,
                        options: Vec::new(),
                    }),
                    None => {
                        self.say("status not loaded yet; press r".into(), true);
                        Vec::new()
                    }
                }
            }
            KeyCode::Char('?') => {
                if self.busy() {
                    return Vec::new();
                }
                self.overlay = Some(Overlay::help("keys", FEATURE_HELP));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// The feature and role a per-role action applies to, or `None` after
    /// saying why not: an action is running, no status is cached, or the
    /// row is a merged change or a frozen reference.
    fn role_action(&mut self) -> Option<(String, String)> {
        if self.busy() {
            return None;
        }
        let name = self.feature_name()?.to_string();
        if self.feature_report().is_none() {
            self.say("status not loaded yet; press r".into(), true);
            return None;
        }
        let Some((change, _)) = self.selected_change() else {
            // A frozen reference row has a role but no change.
            let text = match self.selected_role_row() {
                Some(row) => format!(
                    "no open change for role '{}' in feature '{}'",
                    row.role, name
                ),
                None => "status not loaded yet; press r".into(),
            };
            self.say(text, true);
            return None;
        };
        if !change.is_open() {
            let text = format!(
                "no open change for role '{}' in feature '{}'",
                change.role, name
            );
            self.say(text, true);
            return None;
        }
        Some((name, change.role.clone()))
    }

    /// Step the cursor through the display order and clamp at its ends.
    /// A change is stored by its record index, a frozen row by its role, so
    /// a refresh that shifts the frozen rows keeps the same role selected.
    fn move_role(&mut self, delta: isize) -> Vec<Effect> {
        let Some(current) = self.cursor() else {
            return Vec::new();
        };
        let Some((feature, report)) = self.feature_report() else {
            return Vec::new();
        };
        let order = Self::change_order(report);
        let pos = order.iter().position(|&i| i == current).unwrap_or(0) as isize;
        let next = (pos + delta).clamp(0, order.len().saturating_sub(1) as isize) as usize;
        let pick = order.get(next).map(|&i| {
            if i < feature.changes.len() {
                Selected::Change(i)
            } else {
                Selected::Frozen(Self::change_rows(report)[i].role.clone())
            }
        });
        if let Screen::Feature { selected, .. } = &mut self.screen {
            *selected = pick;
        }
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::*;
    use super::display_order;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use hub::feature::{Change, Feature, Owner, Pin, Stage, Worktree};
    use hub::ops::list::RoleSummary;
    use hub::ops::status::{StatusReport, StatusRow};
    use std::path::PathBuf;

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    pub(crate) fn summary_of(name: &str) -> FeatureSummary {
        FeatureSummary {
            name: name.into(),
            checkout: format!("acme-{name}"),
            status: FeatureStatus::Open,
            roles: vec![RoleSummary {
                role: "api".into(),
                branch: "feature/x".into(),
                stage: Stage::Working,
            }],
        }
    }

    pub(crate) fn change(role: &str, branch: &str, stage: Stage) -> Change {
        Change {
            role: role.into(),
            clone: format!("{role}-clone"),
            branch: branch.into(),
            worktree: Some(hub::feature::Worktree {
                name: "acme-suspense".into(),
                owner: hub::feature::Owner::Hub,
            }),
            stage,
            review_url: None,
            merged_at: None,
            origin_seen: false,
            base: Some("main".into()),
            base_sha: None,
        }
    }

    pub(crate) fn status_row(role: &str, branch: &str, stage: &str) -> StatusRow {
        StatusRow {
            role: role.into(),
            branch: branch.into(),
            stage: stage.into(),
            path: PathBuf::from(format!("/wt/{role}/{branch}")),
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
            frozen_sha: None,
            base_moved: None,
            drift: false,
        }
    }

    /// A feature with an open `api` change and a merged `ui` change, plus
    /// the report the worker would produce for it: hub row first.
    pub(crate) fn suspense() -> Report {
        let mut f = Feature::new("suspense", "acme-suspense");
        f.changes.push(change("api", "feature/x", Stage::Working));
        f.changes.push(change("ui", "old-ui", Stage::Merged));
        Report::Feature {
            feature: f,
            report: StatusReport {
                feature: "suspense".into(),
                checkout: "acme-suspense".into(),
                status: FeatureStatus::Open,
                rows: vec![
                    status_row("hub", "suspense", "-"),
                    status_row("api", "feature/x", "working"),
                    status_row("ui", "old-ui", "merged"),
                ],
                warnings: vec![],
                drift: false,
            },
        }
    }

    /// A feature whose `api` role was merged and then reopened: two rows
    /// share the role, so the row under the cursor — not the role name —
    /// decides which change the keys act on.
    pub(crate) fn reopened() -> Report {
        let mut f = Feature::new("suspense", "acme-suspense");
        f.changes.push(change("api", "old-api", Stage::Merged));
        f.changes.push(change("api", "feature/x", Stage::Working));
        Report::Feature {
            feature: f,
            report: StatusReport {
                feature: "suspense".into(),
                checkout: "acme-suspense".into(),
                status: FeatureStatus::Open,
                rows: vec![
                    status_row("hub", "suspense", "-"),
                    status_row("api", "old-api", "merged"),
                    status_row("api", "feature/x", "working"),
                ],
                warnings: vec![],
                drift: false,
            },
        }
    }

    pub(crate) const PIN: &str = "0123456789abcdef0123456789abcdef01234567";

    pub(crate) fn pin(role: &str, worktree: bool) -> Pin {
        Pin {
            role: role.into(),
            clone: format!("{role}-clone"),
            base: "main".into(),
            sha: PIN.into(),
            worktree: worktree.then(|| Worktree {
                name: "suspense-frozen".into(),
                owner: Owner::Hub,
            }),
        }
    }

    /// The row `status` emits for a frozen reference worktree.
    pub(crate) fn frozen_status_row(role: &str) -> StatusRow {
        let mut r = status_row(role, "main", "frozen");
        r.path = PathBuf::from(format!("/wt/{role}/suspense-frozen"));
        r.ahead = None;
        r.behind = None;
        r.frozen_sha = Some(PIN.into());
        r.base_moved = Some(3);
        r
    }

    /// `suspense` with `ui` (whose only change is merged) frozen in a
    /// reference worktree. Rows: hub, api working, ui merged, ui frozen.
    pub(crate) fn frozen_ui() -> Report {
        let Report::Feature {
            mut feature,
            mut report,
        } = suspense()
        else {
            unreachable!()
        };
        feature.pins.push(pin("ui", true));
        report.rows.push(frozen_status_row("ui"));
        Report::Feature { feature, report }
    }

    /// Dashboard app on the `suspense` row with its report cached.
    pub(crate) fn on_feature() -> App {
        let mut a = App::new("acme", vec![summary_of("suspense")]);
        a.update(key(KeyCode::Char('j')));
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 1,
            result: Ok(suspense()),
        });
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::Request {
                row: RowId::Feature("suspense".into()),
                r#gen: 2
            }]
        );
        a
    }

    #[test]
    fn enter_opens_the_feature_view_and_esc_returns_with_a_reload() {
        let mut a = on_feature();
        assert_eq!(
            a.screen,
            Screen::Feature {
                name: "suspense".into(),
                selected: None
            }
        );
        assert_eq!(a.feature_name(), Some("suspense"));
        assert_eq!(a.update(key(KeyCode::Esc)), vec![Effect::ReloadList]);
        assert_eq!(a.screen, Screen::Dashboard);
    }

    #[test]
    fn enter_is_refused_on_base_and_on_finished_features() {
        let mut a = App::new("acme", vec![summary_of("suspense")]);
        assert!(a.update(key(KeyCode::Enter)).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "base has no feature view")
        );
        assert_eq!(a.screen, Screen::Dashboard);
        let mut done = summary_of("done");
        done.status = FeatureStatus::Finished;
        let mut a = App::new("acme", vec![done]);
        a.show_finished = true;
        a.rebuild_rows(None);
        a.update(key(KeyCode::Char('j')));
        assert!(a.update(key(KeyCode::Enter)).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text == "feature 'done' is finished")
        );
    }

    #[test]
    fn role_selection_moves_over_change_rows_and_clamps() {
        let mut a = on_feature();
        assert_eq!(
            a.selected_change().map(|(c, _)| c.role.as_str()),
            Some("api")
        );
        a.update(key(KeyCode::Char('j')));
        assert_eq!(
            a.selected_change().map(|(c, _)| c.role.as_str()),
            Some("ui")
        );
        a.update(key(KeyCode::Down));
        assert_eq!(a.cursor(), Some(1), "clamped");
        a.update(key(KeyCode::Char('k')));
        a.update(key(KeyCode::Up));
        assert_eq!(
            a.screen,
            Screen::Feature {
                name: "suspense".into(),
                selected: Some(Selected::Change(0))
            }
        );
    }

    #[test]
    fn r_and_o_target_the_current_feature() {
        let mut a = on_feature();
        // gen 2 is in flight from opening the view; deliver it, then r requests gen 3.
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(suspense()),
        });
        assert_eq!(
            a.update(key(KeyCode::Char('r'))),
            vec![Effect::Request {
                row: RowId::Feature("suspense".into()),
                r#gen: 3
            }]
        );
        assert_eq!(
            a.update(key(KeyCode::Char('o'))),
            vec![Effect::OpenWorkspace(RowId::Feature("suspense".into()))]
        );
    }

    #[test]
    fn a_report_with_fewer_rows_clamps_the_role_selection() {
        let mut a = on_feature();
        a.update(key(KeyCode::Char('j'))); // ui, index 1
        let Report::Feature {
            mut feature,
            mut report,
        } = suspense()
        else {
            unreachable!()
        };
        // The ui change is gone, not just its row: the cursor's stored
        // index is now past the end of the record.
        feature.changes.pop();
        report.rows.pop();
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(Report::Feature { feature, report }),
        });
        assert_eq!(a.cursor(), Some(0));
    }

    #[test]
    fn q_on_the_feature_view_goes_back_not_out() {
        let mut a = on_feature();
        a.update(key(KeyCode::Char('q')));
        assert!(!a.quit);
        assert_eq!(a.screen, Screen::Dashboard);
    }

    /// Going back is not quitting, so a running action does not block it.
    #[test]
    fn q_and_esc_still_go_back_while_an_action_runs() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut a = on_feature();
            a.running = Some(Action::Finish {
                feature: "suspense".into(),
                force: false,
            });
            assert_eq!(a.update(key(code)), vec![Effect::ReloadList]);
            assert!(!a.quit);
            assert_eq!(a.screen, Screen::Dashboard);
        }
    }

    #[test]
    fn v_opens_the_review_input_and_enter_runs_the_action() {
        let mut a = on_feature();
        assert!(a.update(key(KeyCode::Char('v'))).is_empty());
        assert!(
            matches!(a.overlay, Some(Overlay::Input { ref title, .. }) if title == "Review URL for api")
        );
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::Run(Action::Review {
                feature: "suspense".into(),
                role: "api".into(),
                url: None
            })]
        );
        assert!(a.overlay.is_none() && a.flow.is_none());
    }

    #[test]
    fn esc_in_a_flow_cancels_it_with_a_footer_note() {
        let mut a = on_feature();
        a.update(key(KeyCode::Char('b')));
        assert!(a.flow.is_some());
        a.update(key(KeyCode::Esc));
        assert!(a.overlay.is_none() && a.flow.is_none());
        assert!(a.footer.as_ref().is_some_and(|f| f.text == "cancelled"));
        assert_eq!(
            a.screen,
            Screen::Feature {
                name: "suspense".into(),
                selected: None
            },
            "still on the feature view"
        );
    }

    #[test]
    fn role_actions_are_refused_on_a_merged_role_while_busy_and_without_a_report() {
        let mut a = on_feature();
        a.update(key(KeyCode::Char('j'))); // ui, merged
        assert!(a.update(key(KeyCode::Char('v'))).is_empty());
        assert!(a.footer.as_ref().is_some_and(
            |f| f.is_error && f.text == "no open change for role 'ui' in feature 'suspense'"
        ));
        assert!(a.overlay.is_none());
        a.update(key(KeyCode::Char('k')));
        a.running = Some(Action::Finish {
            feature: "suspense".into(),
            force: false,
        });
        assert!(a.update(key(KeyCode::Char('b'))).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text == "busy: finish suspense…")
        );
        a.running = None;
        a.cache.clear();
        assert!(a.update(key(KeyCode::Char('v'))).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text == "status not loaded yet; press r")
        );
    }

    #[test]
    fn plus_is_refused_when_every_role_has_an_open_change() {
        let mut a = on_feature();
        a.repos = vec![]; // no addable roles
        assert!(a.update(key(KeyCode::Char('+'))).is_empty());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text == "every role already has an open change in suspense")
        );
    }

    #[test]
    fn f_opens_the_finish_preview_and_needs_no_role() {
        let mut a = on_feature();
        a.update(key(KeyCode::Char('j'))); // on the merged ui row; finish is feature-wide
        assert!(a.update(key(KeyCode::Char('f'))).is_empty());
        assert!(
            matches!(a.overlay, Some(Overlay::Confirm { ref title, .. }) if title == "Finish suspense")
        );
        let effects = a.update(key(KeyCode::Char('F')));
        assert_eq!(
            effects,
            vec![Effect::Run(Action::Finish {
                feature: "suspense".into(),
                force: true
            })]
        );
    }

    #[test]
    fn a_reopened_role_acts_on_the_row_under_the_cursor_not_the_last_change() {
        let mut a = on_feature();
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(reopened()),
        });
        // Force the cursor onto change 0 (merged), though the role's last
        // change (index 1) is open: selected_change resolves by change
        // index, not by role name.
        a.screen = Screen::Feature {
            name: "suspense".into(),
            selected: Some(Selected::Change(0)),
        };
        let (change, row) = a.selected_change().expect("a change under the cursor");
        assert!(!change.is_open());
        assert_eq!(
            (change.branch.as_str(), row.branch.as_str()),
            ("old-api", "old-api")
        );
        for code in [KeyCode::Char('m'), KeyCode::Char('v'), KeyCode::Char('b')] {
            assert!(a.update(key(code)).is_empty(), "{code:?} ran something");
            assert!(a.overlay.is_none(), "{code:?} opened an overlay");
            assert!(
                a.footer.as_ref().is_some_and(|f| f.is_error
                    && f.text == "no open change for role 'api' in feature 'suspense'"),
                "{code:?}: {:?}",
                a.footer.as_ref().map(|f| f.text.clone())
            );
        }
        a.screen = Screen::Feature {
            name: "suspense".into(),
            selected: Some(Selected::Change(1)),
        };
        let (change, row) = a.selected_change().expect("a change under the cursor");
        assert!(change.is_open());
        assert_eq!(
            (change.branch.as_str(), row.branch.as_str()),
            ("feature/x", "feature/x")
        );
    }

    /// Dashboard app on the `suspense` row with `report` cached, then Enter.
    pub(crate) fn on_report(report: Report) -> App {
        let mut a = App::new("acme", vec![summary_of("suspense")]);
        a.update(key(KeyCode::Char('j')));
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 1,
            result: Ok(report),
        });
        a.update(key(KeyCode::Enter));
        a
    }

    #[test]
    fn opening_the_view_selects_the_first_open_change() {
        let a = on_report(reopened());
        // Record order is merged then working; the cursor lands on working.
        let (change, row) = a.selected_change().unwrap();
        assert_eq!(change.branch, "feature/x");
        assert_eq!(row.branch, "feature/x");
        assert_eq!(a.cursor(), Some(1));
    }

    #[test]
    fn j_and_k_walk_the_display_order_and_clamp() {
        let mut a = on_report(reopened());
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.cursor(), Some(0), "down from working reaches merged");
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.cursor(), Some(0), "clamps at the last row");
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.cursor(), Some(1));
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.cursor(), Some(1), "clamps at the first row");
    }

    #[test]
    fn j_and_k_walk_open_frozen_and_merged_rows() {
        let mut a = on_report(frozen_ui());
        assert_eq!(a.cursor(), Some(0), "the open api change");
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.cursor(), Some(2), "down reaches the frozen ui row");
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.cursor(), Some(1), "then the merged ui change");
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.cursor(), Some(2), "back up to the frozen row");
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.cursor(), Some(0), "back up to the open change");
        a.update(key(KeyCode::Char('k')));
        assert_eq!(a.cursor(), Some(0), "clamps at the first row");
    }

    #[test]
    fn a_refresh_that_reorders_rows_keeps_the_cursor_on_the_same_change() {
        // Two open changes; the cursor moves to the second (ui).
        let mut f = Feature::new("suspense", "acme-suspense");
        f.changes.push(change("api", "feature/x", Stage::Working));
        f.changes.push(change("ui", "feature/y", Stage::Working));
        let rows = |api_stage: &str| StatusReport {
            feature: "suspense".into(),
            checkout: "acme-suspense".into(),
            status: FeatureStatus::Open,
            rows: vec![
                status_row("hub", "suspense", "-"),
                status_row("api", "feature/x", api_stage),
                status_row("ui", "feature/y", "working"),
            ],
            warnings: vec![],
            drift: false,
        };
        let mut a = on_report(Report::Feature {
            feature: f.clone(),
            report: rows("working"),
        });
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.selected_change().unwrap().0.branch, "feature/y");
        // api merges; ui is now displayed first, and the cursor stays on it.
        f.changes[0].stage = Stage::Merged;
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(Report::Feature {
                feature: f,
                report: rows("merged"),
            }),
        });
        assert_eq!(a.cursor(), Some(1));
        assert_eq!(a.selected_change().unwrap().0.branch, "feature/y");
        assert_eq!(App::change_order(a.feature_report().unwrap().1), vec![1, 0]);
    }

    #[test]
    fn a_cursor_past_the_end_clamps_to_the_last_change_in_order() {
        let mut a = on_report(reopened());
        a.screen = Screen::Feature {
            name: "suspense".into(),
            selected: Some(Selected::Change(7)),
        };
        assert_eq!(
            a.cursor(),
            Some(0),
            "last in display order is the merged change"
        );
    }

    #[test]
    fn display_order_lists_the_hub_row_then_open_then_merged_changes() {
        let rows = vec![
            status_row("hub", "suspense", "-"),
            status_row("api", "old-api", "merged"),
            status_row("ui", "feature/y", "working"),
            status_row("bff", "old-bff", "merged"),
            status_row("api", "feature/x", "review"),
        ];
        assert_eq!(display_order(&rows), vec![0, 2, 4, 1, 3]);
        assert_eq!(display_order(&rows[..1]), vec![0]);
        assert_eq!(display_order(&[]), Vec::<usize>::new());
    }

    #[test]
    fn change_order_drops_the_hub_row_and_reindexes() {
        let Report::Feature { report, .. } = reopened() else {
            unreachable!()
        };
        // Record order: api merged (0), api working (1).
        assert_eq!(App::change_order(&report), vec![1, 0]);
    }

    #[test]
    fn c_offers_branch_path_and_review_url_then_copies_the_pick() {
        let Report::Feature {
            mut feature,
            report,
        } = suspense()
        else {
            unreachable!()
        };
        feature.changes[0].review_url = Some("https://gitlab.example/mr/1".into());
        let mut a = on_report(Report::Feature { feature, report });
        assert!(a.update(key(KeyCode::Char('c'))).is_empty());
        let Some(Overlay::Select { title, rows, .. }) = &a.overlay else {
            panic!("{:?}", a.overlay);
        };
        assert_eq!(title, "copy");
        assert_eq!(
            rows,
            &vec![
                "branch  feature/x".to_string(),
                "path  /wt/api/feature/x".to_string(),
                "review  https://gitlab.example/mr/1".to_string(),
            ]
        );
        a.update(key(KeyCode::Down));
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::Copy {
                label: "path".into(),
                text: "/wt/api/feature/x".into(),
            }]
        );
        assert!(a.overlay.is_none());
        assert!(a.flow.is_none());
    }

    /// A background status refresh landing while the picker is open must
    /// not retarget what Enter copies: the options shown at Begin are what
    /// Picked resolves against, not whatever the cache holds by then.
    #[test]
    fn a_stale_refresh_while_the_picker_is_open_does_not_retarget_the_copy() {
        let mut a = on_report(suspense());
        assert!(a.update(key(KeyCode::Char('c'))).is_empty());
        let Some(Overlay::Select { rows, .. }) = &a.overlay else {
            panic!("{:?}", a.overlay);
        };
        assert_eq!(
            rows,
            &vec![
                "branch  feature/x".to_string(),
                "path  /wt/api/feature/x".to_string(),
            ]
        );
        // The row under the cursor gains a review URL and its worktree goes
        // missing before Enter is pressed; opening the feature view issued
        // a second request (gen 2), still in flight.
        let Report::Feature {
            mut feature,
            mut report,
        } = suspense()
        else {
            unreachable!()
        };
        feature.changes[0].review_url = Some("https://gitlab.example/mr/1".into());
        report.rows[1].exists = false;
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(Report::Feature { feature, report }),
        });
        a.update(key(KeyCode::Down));
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::Copy {
                label: "path".into(),
                text: "/wt/api/feature/x".into(),
            }],
            "Enter copies the row shown at Begin, not one recomputed from the refreshed cache"
        );
    }

    #[test]
    fn c_omits_a_missing_worktree_and_an_absent_url_and_works_on_a_merged_row() {
        let Report::Feature {
            feature,
            mut report,
        } = suspense()
        else {
            unreachable!()
        };
        report.rows[1].exists = false;
        let mut a = on_report(Report::Feature { feature, report });
        a.update(key(KeyCode::Char('c')));
        let Some(Overlay::Select { rows, .. }) = &a.overlay else {
            panic!("{:?}", a.overlay);
        };
        assert_eq!(rows, &vec!["branch  feature/x".to_string()]);
        a.update(key(KeyCode::Esc));
        assert!(a.overlay.is_none());

        a.update(key(KeyCode::Char('j'))); // ui, merged
        a.update(key(KeyCode::Char('c')));
        let Some(Overlay::Select { rows, .. }) = &a.overlay else {
            panic!("{:?}", a.overlay);
        };
        assert_eq!(rows[0], "branch  old-ui");
    }

    #[test]
    fn c_is_refused_while_busy_and_without_a_report() {
        let mut a = on_feature();
        a.running = Some(Action::Finish {
            feature: "suspense".into(),
            force: false,
        });
        assert!(a.update(key(KeyCode::Char('c'))).is_empty());
        assert!(a.overlay.is_none());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.text.starts_with("busy: "))
        );

        let mut a = on_feature();
        a.cache.clear();
        assert!(a.update(key(KeyCode::Char('c'))).is_empty());
        assert!(a.overlay.is_none());
        assert!(
            a.footer
                .as_ref()
                .is_some_and(|f| f.is_error && f.text == "status not loaded yet; press r")
        );
    }

    #[test]
    fn the_cursor_reaches_a_frozen_row_between_open_and_merged_ones() {
        let mut a = on_report(frozen_ui());
        assert_eq!(a.cursor(), Some(0), "lands on the open api change");
        a.update(key(KeyCode::Char('j')));
        assert_eq!(
            a.cursor(),
            Some(2),
            "the frozen ui row sits above the merged change"
        );
        let row = a.selected_role_row().expect("a row under the cursor");
        assert_eq!((row.role.as_str(), row.stage.as_str()), ("ui", "frozen"));
        assert!(
            a.selected_change().is_none(),
            "a frozen row is not a change"
        );
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.cursor(), Some(1), "then the merged ui change");
        a.update(key(KeyCode::Char('j')));
        assert_eq!(a.cursor(), Some(1), "clamped");
    }

    #[test]
    fn a_view_with_only_a_frozen_role_opens_on_it() {
        let mut f = Feature::new("suspense", "acme-suspense");
        f.pins.push(pin("ui", true));
        let a = on_report(Report::Feature {
            feature: f,
            report: StatusReport {
                feature: "suspense".into(),
                checkout: "acme-suspense".into(),
                status: FeatureStatus::Open,
                rows: vec![status_row("hub", "suspense", "-"), frozen_status_row("ui")],
                warnings: vec![],
                drift: false,
            },
        });
        assert_eq!(a.cursor(), Some(0));
        assert_eq!(a.selected_role_row().map(|r| r.role.as_str()), Some("ui"));
    }

    #[test]
    fn change_keys_on_a_frozen_row_say_it_has_no_open_change() {
        let mut a = on_report(frozen_ui());
        a.update(key(KeyCode::Char('j'))); // ui, frozen
        for code in [KeyCode::Char('v'), KeyCode::Char('b'), KeyCode::Char('m')] {
            assert!(a.update(key(code)).is_empty(), "{code:?}");
            assert!(a.overlay.is_none(), "{code:?}");
            assert!(
                a.footer.as_ref().is_some_and(|f| f.is_error
                    && f.text == "no open change for role 'ui' in feature 'suspense'"),
                "{code:?}: {:?}",
                a.footer.as_ref().map(|f| f.text.clone())
            );
        }
    }

    #[test]
    fn a_report_without_the_frozen_row_clamps_the_cursor() {
        let mut a = on_report(frozen_ui());
        a.update(key(KeyCode::Char('j'))); // ui frozen, index 2
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(suspense()),
        });
        assert_eq!(
            a.cursor(),
            Some(1),
            "the last row in display order: merged ui"
        );
    }

    #[test]
    fn c_on_a_frozen_row_offers_the_pinned_commit_and_the_path() {
        let mut a = on_report(frozen_ui());
        a.update(key(KeyCode::Char('j'))); // ui, frozen
        assert!(a.update(key(KeyCode::Char('c'))).is_empty());
        let Some(Overlay::Select { rows, .. }) = &a.overlay else {
            panic!("{:?}", a.overlay);
        };
        assert_eq!(
            rows,
            &vec![
                format!("commit  {PIN}"),
                "path  /wt/ui/suspense-frozen".to_string(),
            ]
        );
        let effects = a.update(key(KeyCode::Enter));
        assert_eq!(
            effects,
            vec![Effect::Copy {
                label: "commit".into(),
                text: PIN.into(),
            }]
        );
    }

    #[test]
    fn a_refresh_that_shifts_the_frozen_row_keeps_the_same_role_selected() {
        let mut a = on_report(frozen_ui());
        a.update(key(KeyCode::Char('j'))); // ui frozen, index 2
        assert_eq!(
            a.screen,
            Screen::Feature {
                name: "suspense".into(),
                selected: Some(Selected::Frozen("ui".into()))
            }
        );
        // Another terminal adds bff: its change is appended to the record,
        // so its row lands before the frozen ui row and shifts it to index 3.
        let Report::Feature {
            mut feature,
            mut report,
        } = frozen_ui()
        else {
            unreachable!()
        };
        feature
            .changes
            .push(change("bff", "feature/z", Stage::Working));
        report
            .rows
            .insert(3, status_row("bff", "feature/z", "working"));
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(Report::Feature { feature, report }),
        });
        assert_eq!(a.cursor(), Some(3), "followed the role, not the index");
        let row = a.selected_role_row().expect("a row under the cursor");
        assert_eq!((row.role.as_str(), row.stage.as_str()), ("ui", "frozen"));
    }
}
