//! Rendering of the dashboard. Pure over `App`; no terminal handling.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ui::app::{App, Cached, Report, RowId, Screen};

mod dashboard;
mod feature;
mod overlay;
use dashboard::{SPINNER, render_list, render_status};

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 12;

/// Key legends as `(key, description)` pairs; `legend_line` styles the key
/// bold and the description dim (EW-51). The rendered text is exactly the
/// pairs joined by two spaces, with one leading space. These show only the
/// frequent keys so both fit the minimum width; `?` lists them all.
const LEGEND: &[(&str, &str)] = &[
    ("↑↓/jk", "select"),
    ("⏎", "open"),
    ("n", "new"),
    ("?", "help"),
    ("q", "quit"),
];
const FEATURE_LEGEND: &[(&str, &str)] = &[
    ("↑↓/jk", "role"),
    ("c", "copy"),
    ("f", "finish"),
    ("?", "help"),
    ("Esc", "back"),
];

/// ` k desc  k desc …`: keys bold, descriptions dim. Shared with the
/// overlay key lines.
pub(super) fn legend_line<K: AsRef<str>, D: AsRef<str>>(entries: &[(K, D)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, desc)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            key.as_ref().to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", desc.as_ref()),
            Style::new().add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(app, frame, header);
    match &app.screen {
        Screen::Dashboard => {
            let list_width = (area.width / 3).clamp(20, 36);
            let [list, status] =
                Layout::horizontal([Constraint::Length(list_width), Constraint::Min(0)])
                    .areas(body);
            render_list(app, frame, list);
            render_status(app, frame, status);
        }
        Screen::Feature { .. } => feature::render_feature(app, frame, body),
    }
    render_footer(app, frame, footer);
    overlay::render_overlay(app, frame, area);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let (left, right) = match &app.screen {
        Screen::Feature { name, .. } => {
            let checkout = app
                .feature_report()
                .map(|(_, report)| report.checkout.clone())
                .or_else(|| {
                    app.rows
                        .iter()
                        .find(|r| r.id == RowId::Feature(name.clone()))
                        .and_then(|r| r.summary.as_ref())
                        .map(|s| s.checkout.clone())
                })
                .unwrap_or_default();
            (format!(" hub: {}  ▸ {name}", app.hub_name), checkout)
        }
        Screen::Dashboard => {
            let branch = match app.cache.get(&RowId::Base) {
                Some(Cached {
                    report: Report::Base(base),
                    ..
                }) => base
                    .rows
                    .first()
                    .map(|hub| hub.branch.clone())
                    .unwrap_or_default(),
                _ => String::new(),
            };
            (format!(" hub: {}", app.hub_name), branch)
        }
    };
    let [left_area, right_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(right.chars().count() as u16 + 1),
    ])
    .areas(area);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(left).style(bold), left_area);
    frame.render_widget(Paragraph::new(right).style(bold), right_area);
}

fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let now = Instant::now();
    let line = match &app.footer {
        Some(f) if f.visible(now) => {
            let style = if f.is_error {
                Style::new().fg(Color::Red)
            } else {
                Style::new().fg(Color::Green)
            };
            Line::styled(format!(" {}", f.text.replace('\n', "; ")), style)
        }
        _ => match &app.running {
            Some(action) => Line::styled(
                format!(
                    " running: {action}… {}",
                    SPINNER[app.spinner % SPINNER.len()]
                ),
                Style::new().add_modifier(Modifier::DIM),
            ),
            None => match &app.screen {
                Screen::Dashboard => legend_line(LEGEND),
                Screen::Feature { .. } => legend_line(FEATURE_LEGEND),
            },
        },
    };
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use crate::ui::app::{Action, Event, Footer, Overlay, Preview, Tone};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use hub::error::HubError;
    use hub::feature::{Feature, FeatureStatus, Stage};
    use hub::ops::list::{FeatureSummary, RoleSummary};
    use hub::ops::status::{BaseStatusReport, StatusReport, StatusRow};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn lines(width: u16, height: u16, app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(app, f)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn joined(lines: &[String]) -> String {
        lines.join("\n")
    }

    /// The rendered buffer as one whitespace-collapsed string, with the box
    /// glyphs dropped, so a sentence that wrapped over several rows can be
    /// matched whole.
    fn flatten(lines: &[String]) -> String {
        lines
            .iter()
            .map(|l| l.replace(['│', '┌', '┐', '└', '┘', '─'], " "))
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The style of the first cell of the first occurrence of `needle` in
    /// the rendered buffer, scanning row by row.
    fn find_style(width: u16, height: u16, app: &App, needle: &str) -> Style {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(app, f)).unwrap();
        let buffer = terminal.backend().buffer();
        for row in buffer.content.chunks(width as usize) {
            let text: String = row.iter().map(|c| c.symbol()).collect();
            if let Some(byte_idx) = text.find(needle) {
                let char_idx = text[..byte_idx].chars().count();
                return row[char_idx].style();
            }
        }
        panic!("substring {needle:?} not found in the rendered buffer");
    }

    fn row(role: &str, branch: &str, stage: &str, dirty: bool, hint: Option<&str>) -> StatusRow {
        StatusRow {
            role: role.into(),
            branch: branch.into(),
            stage: stage.into(),
            path: PathBuf::from(format!("/wt/{role}/{branch}")),
            exists: true,
            registered: true,
            branch_matches: true,
            dirty,
            ahead: Some(0),
            behind: Some(0),
            base: None,
            custom_base: false,
            base_behind: None,
            hint: hint.map(str::to_string),
            review_url: None,
            drift: dirty,
        }
    }

    fn summary(name: &str, status: FeatureStatus) -> FeatureSummary {
        FeatureSummary {
            name: name.into(),
            checkout: format!("acme-{name}"),
            status,
            roles: vec![
                RoleSummary {
                    role: "api".into(),
                    branch: "x".into(),
                    stage: Stage::Review,
                },
                RoleSummary {
                    role: "ui".into(),
                    branch: "y".into(),
                    stage: Stage::Working,
                },
            ],
        }
    }

    fn app() -> App {
        let mut a = App::new(
            "acme",
            vec![
                summary("suspense", FeatureStatus::Open),
                summary("old", FeatureStatus::Finished),
            ],
        );
        a.show_finished = true;
        a.rebuild_rows(None);
        a
    }

    #[test]
    fn features_pane_omits_roles_whose_last_change_is_merged() {
        let mut s = summary("suspense", FeatureStatus::Open);
        s.roles.push(RoleSummary {
            role: "bff".into(),
            branch: "z".into(),
            stage: Stage::Merged,
        });
        let a = App::new("acme", vec![s]);
        let rows = lines(100, 30, &a);
        let text = joined(&rows);
        // List pane is 33 wide at 100 columns: the stage ends at its right border.
        assert!(
            rows.iter()
                .any(|l| l.contains("  api ") && l.contains("review│")),
            "{text}"
        );
        assert!(
            rows.iter()
                .any(|l| l.contains("  ui ") && l.contains("working│")),
            "{text}"
        );
        assert!(!text.contains("bff"), "merged role listed: {text}");
    }

    fn deliver(app: &mut App, row: RowId, report: Report) {
        let effects = app.request_selected();
        let r#gen = match &effects[..] {
            [crate::ui::app::Effect::Request { r#gen, .. }] => *r#gen,
            other => panic!("unexpected effects {other:?}"),
        };
        app.update(Event::Report {
            row,
            r#gen,
            result: Ok(report),
        });
    }

    #[test]
    fn base_selected_with_a_report_shows_header_list_and_table() {
        let mut a = app();
        deliver(
            &mut a,
            RowId::Base,
            Report::Base(BaseStatusReport {
                hub: "acme".into(),
                rows: vec![
                    row("hub", "main", "-", false, None),
                    row("api", "master", "-", true, None),
                ],
                warnings: vec!["fetch failed for ui".into()],
                drift: true,
            }),
        );
        let out = lines(100, 30, &a);
        let text = joined(&out);
        assert!(out[0].contains("hub: acme"), "{}", out[0]);
        assert!(
            out[0].contains("main"),
            "header shows the hub branch: {}",
            out[0]
        );
        assert!(text.contains("features"));
        assert!(text.contains("▸ base"));
        assert!(text.contains("suspense"));
        assert!(
            out.iter()
                .any(|l| l.contains("  api ") && l.contains("review│"))
        );
        assert!(text.contains("old  finished"));
        assert!(text.contains("status: base"));
        assert!(text.contains("ROLE"));
        assert!(text.contains("dirty"));
        assert!(text.contains("warning: fetch failed for ui"));
        assert!(text.contains("refreshed 0s ago"));
        assert!(out.last().unwrap().contains("q quit"));
    }

    #[test]
    fn feature_selected_shows_hints_attach_line_and_spinner_while_refreshing() {
        let mut a = app();
        a.update(Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        )));
        // The move already requested gen 1; deliver it directly.
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 1,
            result: Ok(Report::Feature {
                feature: Feature::new("suspense", "acme-suspense"),
                report: StatusReport {
                    feature: "suspense".into(),
                    checkout: "acme-suspense".into(),
                    status: FeatureStatus::Open,
                    rows: vec![row(
                        "api",
                        "feature/x",
                        "review",
                        false,
                        Some("looks merged"),
                    )],
                    warnings: vec![],
                    drift: false,
                },
            }),
        });
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("status: suspense"));
        assert!(text.contains("! api: looks merged"));
        assert!(text.contains("Attach with: tmux attach -t acme-suspense"));
        assert!(!text.contains("refreshing"));
        a.request_selected();
        let text = joined(&lines(100, 30, &a));
        assert!(
            text.contains("refreshing…"),
            "spinner while in flight: {text}"
        );
        assert!(text.contains("ROLE"), "cached table still shown");
    }

    #[test]
    fn tmux_off_hides_the_attach_line() {
        let mut a = app();
        a.tmux_on = false;
        a.update(Event::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        )));
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 1,
            result: Ok(Report::Feature {
                feature: Feature::new("suspense", "acme-suspense"),
                report: StatusReport {
                    feature: "suspense".into(),
                    checkout: "acme-suspense".into(),
                    status: FeatureStatus::Open,
                    rows: vec![row(
                        "api",
                        "feature/x",
                        "review",
                        false,
                        Some("looks merged"),
                    )],
                    warnings: vec![],
                    drift: false,
                },
            }),
        });
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("status: suspense"), "{text}");
        assert!(!text.contains("Attach with"), "{text}");
    }

    #[test]
    fn no_cache_shows_only_the_spinner_and_errors_replace_the_table() {
        let mut a = app();
        a.request_selected();
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("refreshing…"));
        assert!(!text.contains("ROLE"));
        a.update(Event::Report {
            row: RowId::Base,
            r#gen: 1,
            result: Err(HubError::Precondition(
                "clone for role 'api' is missing".into(),
            )),
        });
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("clone for role 'api' is missing"));
        assert!(!text.contains("ROLE"));
    }

    #[test]
    fn footer_shows_a_message_then_the_legend() {
        let mut a = app();
        a.footer = Some(Footer {
            text: "Opened /w/x.code-workspace".into(),
            is_error: false,
            at: Instant::now(),
        });
        let out = lines(100, 30, &a);
        assert!(out.last().unwrap().contains("Opened /w/x.code-workspace"));
        a.footer.as_mut().unwrap().at = Instant::now() - std::time::Duration::from_secs(5);
        let out = lines(100, 30, &a);
        assert!(out.last().unwrap().contains("q quit"));
    }

    #[test]
    fn footer_shows_the_running_action() {
        let mut a = app();
        a.running = Some(Action::Finish {
            feature: "suspense".into(),
            force: false,
        });
        let out = lines(100, 30, &a);
        assert!(
            out.last().unwrap().contains("running: finish suspense…"),
            "{}",
            out.last().unwrap()
        );
    }

    #[test]
    fn feature_view_shows_roles_review_url_and_its_legend() {
        let mut a = crate::ui::app::feature::tests::on_feature();
        let Report::Feature {
            mut feature,
            report,
        } = crate::ui::app::feature::tests::suspense()
        else {
            unreachable!()
        };
        feature.changes[0].review_url = Some("https://gitlab.example/mr/1".into());
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(Report::Feature { feature, report }),
        });
        let out = lines(100, 30, &a);
        let text = joined(&out);
        assert!(out[0].contains("hub: acme  ▸ suspense"), "{}", out[0]);
        assert!(out[0].contains("acme-suspense"), "{}", out[0]);
        assert!(text.contains(" roles "), "{text}");
        assert!(
            text.contains("▸ api"),
            "cursor on the first change row: {text}"
        );
        assert!(text.contains("hub"), "hub row still listed: {text}");
        assert!(
            text.contains("review: https://gitlab.example/mr/1"),
            "{text}"
        );
        assert!(
            text.contains("Attach with: tmux attach -t acme-suspense"),
            "{text}"
        );
        assert!(
            out.last().unwrap().contains("Esc back"),
            "{}",
            out.last().unwrap()
        );
        assert!(
            !text.contains("features"),
            "no list pane on the feature view: {text}"
        );
    }

    #[test]
    fn feature_view_lists_open_changes_above_merged_with_the_cursor_on_the_open_one() {
        let a =
            crate::ui::app::feature::tests::on_report(crate::ui::app::feature::tests::reopened());
        let out = lines(100, 30, &a);
        let open = out.iter().position(|l| l.contains("feature/x")).unwrap();
        let merged = out.iter().position(|l| l.contains("old-api")).unwrap();
        assert!(open < merged, "open row above merged: {}", joined(&out));
        assert!(
            out[open].contains("▸"),
            "cursor on the open row: {}",
            out[open]
        );
        assert!(!out[merged].contains("▸"), "{}", out[merged]);
    }

    #[test]
    fn dashboard_status_pane_lists_open_changes_above_merged() {
        let mut a = App::new(
            "acme",
            vec![crate::ui::app::feature::tests::summary_of("suspense")],
        );
        a.update(Event::Key(KeyEvent::new(
            KeyCode::Char('j'),
            KeyModifiers::NONE,
        )));
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 1,
            result: Ok(crate::ui::app::feature::tests::reopened()),
        });
        let out = lines(100, 30, &a);
        let open = out.iter().position(|l| l.contains("feature/x")).unwrap();
        let merged = out.iter().position(|l| l.contains("old-api")).unwrap();
        assert!(open < merged, "{}", joined(&out));
    }

    #[test]
    fn feature_view_renders_a_long_branch_whole() {
        let mut a = crate::ui::app::feature::tests::on_feature();
        let Report::Feature {
            mut feature,
            mut report,
        } = crate::ui::app::feature::tests::suspense()
        else {
            unreachable!()
        };
        let branch = "acmeOrders/field-feedback-9_13-10-fix"; // 37 chars, over the old cap
        feature.changes[0].branch = branch.into();
        report.rows[1].branch = branch.into();
        a.update(Event::Report {
            row: RowId::Feature("suspense".into()),
            r#gen: 2,
            result: Ok(Report::Feature { feature, report }),
        });
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains(branch), "branch truncated: {text}");
    }

    #[test]
    fn legend_keys_are_bold_and_descriptions_dim() {
        let a = app();
        let out = lines(100, 30, &a);
        assert_eq!(
            out.last().unwrap(),
            " ↑↓/jk select  ⏎ open  n new  ? help  q quit"
        );
        assert!(
            find_style(100, 30, &a, "q quit")
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            !find_style(100, 30, &a, "q quit")
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            find_style(100, 30, &a, "quit")
                .add_modifier
                .contains(Modifier::DIM)
        );
        assert!(
            !find_style(100, 30, &a, "quit")
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn footers_fit_the_minimum_width_verbatim() {
        let a = app();
        let out = lines(MIN_WIDTH, 30, &a);
        assert_eq!(
            out.last().unwrap(),
            " ↑↓/jk select  ⏎ open  n new  ? help  q quit"
        );
        let a = crate::ui::app::feature::tests::on_feature();
        let out = lines(MIN_WIDTH, 30, &a);
        assert_eq!(
            out.last().unwrap(),
            " ↑↓/jk role  c copy  f finish  ? help  Esc back"
        );
    }

    #[test]
    fn help_overlay_lists_every_key_for_the_screen() {
        let mut a = app();
        a.update(Event::Key(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        let text = flatten(&lines(100, 30, &a));
        assert!(text.contains("keys"), "title: {text}");
        assert!(text.contains("a show or hide finished features"), "{text}");
        assert!(text.contains("? this help"), "{text}");
        assert!(text.contains("? close Esc close"), "{text}");
        let mut a = crate::ui::app::feature::tests::on_feature();
        a.update(Event::Key(KeyEvent::new(
            KeyCode::Char('?'),
            KeyModifiers::NONE,
        )));
        let text = flatten(&lines(100, 30, &a));
        assert!(
            text.contains("c copy the branch, path, or review URL"),
            "{text}"
        );
        assert!(text.contains("+ add a role"), "{text}");
    }

    #[test]
    fn too_small_terminal_shows_one_line() {
        let a = app();
        let out = lines(40, 8, &a);
        assert!(out.iter().any(|l| l.contains("terminal too small")));
        assert!(!joined(&out).contains("features"));
    }

    #[test]
    fn too_small_terminal_full_buffer_snapshot() {
        let a = app();
        let out = lines(40, 8, &a);
        assert_eq!(
            out,
            vec![
                "terminal too small".to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ]
        );
    }

    #[test]
    fn multi_line_errors_render_on_separate_lines() {
        let mut a = app();
        a.request_selected();
        a.update(Event::Report {
            row: RowId::Base,
            r#gen: 1,
            result: Err(HubError::Command {
                command: "git fetch origin".into(),
                stderr: "fatal: could not read".into(),
            }),
        });
        let out = lines(100, 30, &a);
        let first = out
            .iter()
            .position(|l| l.contains("command failed: git fetch origin"));
        let second = out.iter().position(|l| l.contains("fatal: could not read"));
        assert!(first.is_some(), "command line missing: {out:?}");
        assert!(second.is_some(), "stderr line missing: {out:?}");
        assert_ne!(
            first, second,
            "each line of a multi-line error renders separately: {out:?}"
        );
    }

    #[test]
    fn state_colours_and_dim_rows_are_styled() {
        let mut a = app();
        deliver(
            &mut a,
            RowId::Base,
            Report::Base(BaseStatusReport {
                hub: "acme".into(),
                rows: vec![
                    row("hub", "main", "-", false, None),
                    row("api", "feature/x", "working", true, None),
                    row("ui", "feature/y", "merged", false, None),
                ],
                warnings: vec![],
                drift: false,
            }),
        );
        let dirty_style = find_style(100, 30, &a, "dirty");
        assert_eq!(dirty_style.fg, Some(Color::Yellow));
        let merged_style = find_style(100, 30, &a, "merged");
        assert!(
            merged_style.add_modifier.contains(Modifier::DIM),
            "merged row is dim: {merged_style:?}"
        );
    }

    #[test]
    fn status_table_shortens_paths_under_home() {
        let rows = [row("api", "feature/x", "working", false, None)];
        let rows: Vec<&StatusRow> = rows.iter().collect();
        let mut terminal = Terminal::new(TestBackend::new(80, 3)).unwrap();
        terminal
            .draw(|f| {
                f.render_widget(
                    dashboard::status_table(&rows, Some(Path::new("/wt"))),
                    f.area(),
                )
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("~/api/feature/x"), "{text}");
        assert!(!text.contains("/wt/api"), "{text}");
    }

    #[test]
    fn base_lag_shows_in_the_state_cell_in_yellow() {
        let mut a = app();
        let mut lagging = row("api", "feature/x", "working", false, None);
        lagging.base = Some("feature/canonical".into());
        lagging.base_behind = Some(1);
        deliver(
            &mut a,
            RowId::Base,
            Report::Base(BaseStatusReport {
                hub: "acme".into(),
                rows: vec![lagging],
                warnings: vec![],
                drift: false,
            }),
        );
        let text = joined(&lines(120, 30, &a));
        assert!(
            text.contains("behind base feature/canonical by 1"),
            "{text}"
        );
        assert_eq!(
            find_style(120, 30, &a, "behind base").fg,
            Some(Color::Yellow)
        );
    }

    #[test]
    fn overlays_render_centred_over_a_dimmed_screen() {
        let mut a = app();
        a.overlay = Some(Overlay::select(
            "Branch for api".into(),
            vec!["feature/x  new".into(), "main  local".into()],
        ));
        let out = lines(100, 30, &a);
        let text = joined(&out);
        assert!(text.contains(" Branch for api "), "{text}");
        assert!(text.contains("▸ feature/x  new"), "{text}");
        assert!(text.contains("main  local"), "{text}");
        assert!(text.contains("hub: acme"), "screen still visible: {text}");
        assert!(
            find_style(100, 30, &a, "hub: acme")
                .add_modifier
                .contains(Modifier::DIM)
        );

        a.overlay = Some(
            Overlay::input(
                "New feature".into(),
                "suspense".into(),
                Some(Preview::Checkout {
                    template: "acme-{feature}".into(),
                    hub: "acme".into(),
                }),
            )
            .with_error("feature 'suspense' already exists".into()),
        );
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("> suspense"), "{text}");
        assert!(text.contains("checkout: acme-suspense"), "{text}");
        assert!(text.contains("feature 'suspense' already exists"), "{text}");
        assert_eq!(
            find_style(100, 30, &a, "feature 'suspense' already").fg,
            Some(Color::Red)
        );

        a.overlay = Some(Overlay::confirm(
            "Finish suspense".into(),
            vec![
                ("Kill tmux session acme-suspense".into(), Tone::Plain),
                ("! not merged: api (working)".into(), Tone::Warn),
            ],
            true,
        ));
        let text = joined(&lines(100, 30, &a));
        assert!(
            text.contains("⏎ finish  F finish with force  Esc cancel"),
            "{text}"
        );
        assert!(
            find_style(100, 30, &a, "F finish with force")
                .add_modifier
                .contains(Modifier::BOLD),
            "key bold (EW-51)"
        );
        assert_eq!(
            find_style(100, 30, &a, "! not merged").fg,
            Some(Color::Yellow)
        );
        // The real blocker, built the way `flow::finish` builds it: one
        // 159-character sentence that only reads at 80 columns if it wraps.
        if let Some(Overlay::Confirm { blocked, .. }) = &mut a.overlay {
            *blocked = Some(format!(
                "running inside tmux session {}, which finish would kill along with this dashboard; \
                 run it from another terminal: hub feature finish --feature {}",
                "acme-suspense", "suspense"
            ));
        }
        let out = lines(80, 30, &a);
        let text = joined(&out);
        assert!(
            text.contains("running inside tmux session acme-suspense"),
            "{text}"
        );
        assert!(
            flatten(&out)
                .contains("run it from another terminal: hub feature finish --feature suspense"),
            "blocker truncated: {text}"
        );
        assert!(text.contains("Esc cancel"), "no way out offered: {text}");
        assert!(!text.contains("⏎ finish"), "{text}");

        a.overlay = Some(Overlay::multi(
            "Roles for x".into(),
            vec!["api  clone-a".into(), "ui  clone-b".into()],
            "space toggles".into(),
        ));
        if let Some(Overlay::MultiSelect { chosen, .. }) = &mut a.overlay {
            chosen[1] = true;
        }
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("[ ] api  clone-a"), "{text}");
        assert!(text.contains("[x] ui  clone-b"), "{text}");

        a.overlay = Some(Overlay::loading("Branch for ui".into()));
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("loading…"), "{text}");
        assert!(text.contains("Esc cancel"), "no way out offered: {text}");
        assert!(!text.contains("filter:"), "nothing to filter yet: {text}");

        a.overlay = Some(Overlay::result(
            "finish suspense".into(),
            vec![
                "Killed tmux session acme-suspense".into(),
                "Feature 'suspense' finished".into(),
            ],
            false,
        ));
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("Killed tmux session acme-suspense"), "{text}");
        assert!(text.contains("⏎ close"), "{text}");
    }

    #[test]
    fn too_small_terminal_keeps_the_overlay_state() {
        let mut a = app();
        a.overlay = Some(Overlay::result("x".into(), vec!["a".into()], false));
        let out = lines(40, 8, &a);
        assert_eq!(out[0].trim(), "terminal too small");
        assert!(a.overlay.is_some());
    }

    #[test]
    fn zero_row_report_renders_header_only() {
        let mut a = app();
        deliver(
            &mut a,
            RowId::Base,
            Report::Base(BaseStatusReport {
                hub: "acme".into(),
                rows: vec![],
                warnings: vec![],
                drift: false,
            }),
        );
        let text = joined(&lines(100, 30, &a));
        assert!(text.contains("ROLE"));
    }
    #[test]
    fn overlay_height_grows_for_wrapped_lines_and_keeps_the_key_line() {
        let mut a = app();
        // 25 + 100 columns: a per-line estimate says 2 rows at width 76, but
        // word wrapping breaks before the path and needs 3. Two such lines
        // undercount by two rows, more than the result's blank spacer absorbs.
        let long = format!("Remove worktrees    api  /{}", "x".repeat(99));
        a.overlay = Some(Overlay::result(
            "finish suspense".into(),
            vec![
                long.clone(),
                long,
                "Feature 'suspense' finished".into(),
                "no branches were deleted".into(),
            ],
            false,
        ));
        let text = joined(&lines(80, 24, &a));
        assert!(text.contains("Feature 'suspense' finished"), "{text}");
        assert!(
            text.contains("no branches were deleted"),
            "last body line clipped: {text}"
        );
        assert!(text.contains("⏎ close"), "key line clipped: {text}");
    }
}
