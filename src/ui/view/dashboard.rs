//! Dashboard panes: the feature list and the status of the selected row.

use std::path::Path;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{
    Block, Cell, List, ListItem, ListState, Paragraph, Row as TableRow, Table, Wrap,
};

use crate::ui::app::feature::display_order;
use crate::ui::app::{App, Report, RowId};
use hub::env::{home_dir, shorten_home};
use hub::feature::{FeatureStatus, Stage};
use hub::ops::status::{StateColour, StatusRow, state_colour, state_text};

pub(super) const SPINNER: [&str; 4] = ["⠋", "⠙", "⠸", "⠴"];

pub(super) fn render_list(app: &App, frame: &mut Frame, area: Rect) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    // Borders (2) and the highlight symbol `▸ ` (2), which ratatui reserves
    // on every row while one is selected.
    let inner = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|row| match &row.summary {
            None => ListItem::new("base"),
            Some(s) if s.status == FeatureStatus::Finished => {
                ListItem::new(Line::styled(format!("{}  finished", s.name), dim))
            }
            Some(s) => {
                let mut text = vec![Line::from(s.name.clone())];
                text.extend(
                    s.roles
                        .iter()
                        .filter(|r| r.stage != Stage::Merged)
                        .map(|r| Line::from(role_line(&r.role, r.stage.as_str(), inner))),
                );
                ListItem::new(text)
            }
        })
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(" features "))
        .highlight_symbol("▸ ")
        .highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// `"  <role>  …  <stage>"` exactly `width` chars wide when it fits: the
/// stage sits at the right edge and the role gives way, ending in `…`.
/// The stage is never cut; a pane too narrow for it overflows instead.
pub(super) fn role_line(role: &str, stage: &str, width: usize) -> String {
    let stage_len = stage.chars().count();
    // Indent, the role, at least one space, the stage.
    let room = width.saturating_sub(2 + 1 + stage_len);
    let role: String = if role.chars().count() <= room {
        role.to_string()
    } else if room == 0 {
        String::new()
    } else {
        role.chars().take(room - 1).chain(['…']).collect()
    };
    let pad = width
        .saturating_sub(2 + role.chars().count() + stage_len)
        .max(1);
    format!("  {role}{}{stage}", " ".repeat(pad))
}

pub(super) fn render_status(app: &App, frame: &mut Frame, area: Rect) {
    let row = app.selected_row();
    let title = match &row.id {
        RowId::Base => " status: base ".to_string(),
        RowId::Feature(name) => format!(" status: {name} "),
    };
    let Some(inner) = status_prologue(app, frame, &row.id, &title, area) else {
        return;
    };

    let cached = app
        .cache
        .get(&row.id)
        .expect("status_prologue returned an inner area only when the row is cached");
    let (record_rows, display_rows, warnings, attach): (&[StatusRow], Vec<&StatusRow>, _, _) =
        match &cached.report {
            Report::Base(b) => (&b.rows, b.rows.iter().collect(), &b.warnings, None),
            Report::Feature { report: f, .. } => (
                &f.rows,
                display_order(&f.rows)
                    .into_iter()
                    .map(|i| &f.rows[i])
                    .collect(),
                &f.warnings,
                (f.status == FeatureStatus::Open && app.tmux_on)
                    .then(|| format!("Attach with: tmux attach -t {}", f.checkout)),
            ),
        };
    let table_height = display_rows.len() as u16 + 1;
    let [table_area, tail_area] =
        Layout::vertical([Constraint::Length(table_height), Constraint::Min(0)]).areas(inner);
    frame.render_widget(
        status_table(&display_rows, home_dir().as_deref()),
        table_area,
    );

    let tail = report_tail(app, &row.id, record_rows, warnings, attach, Vec::new());
    frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: false }), tail_area);
}

/// Renders the bordered block and, when the row has no cached report yet
/// (an error, or nothing fetched/in flight), the placeholder text — shared
/// by the dashboard status pane and the feature view. Returns the block's
/// inner area only when there is a cached report left for the caller to
/// draw.
pub(super) fn status_prologue(
    app: &App,
    frame: &mut Frame,
    id: &RowId,
    title: &str,
    area: Rect,
) -> Option<Rect> {
    let block = Block::bordered().title(title.to_string());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let in_flight = app.in_flight.contains_key(id);

    if let Some(err) = app.errors.get(id) {
        let mut tail: Vec<Line> = err
            .lines()
            .map(|l| Line::styled(l.to_string(), Style::new().fg(Color::Red)))
            .collect();
        if in_flight {
            tail.push(refresh_line(app, None, in_flight));
        }
        frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: false }), inner);
        return None;
    }

    if !app.cache.contains_key(id) {
        let text = if in_flight {
            refresh_line(app, None, in_flight)
        } else {
            Line::from("press r to refresh")
        };
        frame.render_widget(Paragraph::new(text), inner);
        return None;
    }

    Some(inner)
}

/// Warning lines, hint lines, any `extra` lines the caller supplies, the
/// attach line, then the refresh line — the tail shared by the dashboard
/// status pane and the feature view.
pub(super) fn report_tail(
    app: &App,
    id: &RowId,
    rows: &[StatusRow],
    warnings: &[String],
    attach: Option<String>,
    extra: Vec<Line<'static>>,
) -> Vec<Line<'static>> {
    let mut tail: Vec<Line> = Vec::new();
    for w in warnings {
        for line in format!("warning: {w}").lines() {
            tail.push(Line::styled(
                line.to_string(),
                Style::new().fg(Color::Yellow),
            ));
        }
    }
    for r in rows.iter().filter(|r| r.hint.is_some()) {
        tail.push(Line::styled(
            format!("! {}: {}", r.role, r.hint.as_deref().unwrap_or_default()),
            Style::new().fg(Color::Red),
        ));
    }
    tail.extend(extra);
    if let Some(attach) = attach {
        tail.push(Line::from(attach));
    }
    let in_flight = app.in_flight.contains_key(id);
    let cached_at = app.cache.get(id).map(|c| c.at);
    tail.push(refresh_line(app, cached_at, in_flight));
    tail
}

pub(super) fn status_table<'a>(rows: &[&'a StatusRow], home: Option<&Path>) -> Table<'a> {
    let width_of = |f: &dyn Fn(&StatusRow) -> usize, floor: usize, cap: usize| {
        rows.iter()
            .map(|r| f(r))
            .max()
            .unwrap_or(0)
            .max(floor)
            .min(cap) as u16
    };
    let widths = [
        Constraint::Length(width_of(&|r| r.role.chars().count(), 4, 16)),
        // The branch is the distinguishing name; never truncate it (EW-56).
        Constraint::Length(width_of(&|r| r.branch.chars().count(), 6, usize::MAX)),
        Constraint::Length(width_of(&|r| r.stage.chars().count(), 5, 8)),
        Constraint::Length(width_of(&|r| state_text(r).chars().count(), 5, 40)),
        Constraint::Min(4),
    ];
    let header = TableRow::new(["ROLE", "BRANCH", "STAGE", "STATE", "PATH"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let body = rows.iter().map(|r| {
        let colour = match state_colour(r) {
            StateColour::Red => Color::Red,
            StateColour::Yellow => Color::Yellow,
            StateColour::Green => Color::Green,
        };
        let row = TableRow::new(vec![
            Cell::from(r.role.as_str()),
            Cell::from(r.branch.as_str()),
            Cell::from(r.stage.as_str()),
            Cell::from(state_text(r)).style(Style::new().fg(colour)),
            Cell::from(shorten_home(&r.path, home)),
        ]);
        if r.stage == "merged" {
            row.style(Style::new().add_modifier(Modifier::DIM))
        } else {
            row
        }
    });
    Table::new(body, widths).header(header).column_spacing(2)
}

pub(super) fn refresh_line(app: &App, at: Option<Instant>, in_flight: bool) -> Line<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    if in_flight {
        let frame = SPINNER[app.spinner % SPINNER.len()];
        return Line::styled(format!("refreshing… {frame}"), dim);
    }
    match at {
        Some(at) => Line::styled(format!("refreshed {} ago", format_age(at.elapsed())), dim),
        None => Line::from(""),
    }
}

/// An elapsed time at the precision a glance needs: seconds under a minute,
/// whole minutes under an hour, hours and minutes beyond that.
fn format_age(age: Duration) -> String {
    let secs = age.as_secs();
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_age_scales_seconds_minutes_and_hours() {
        assert_eq!(format_age(Duration::from_secs(0)), "0s");
        assert_eq!(format_age(Duration::from_secs(12)), "12s");
        assert_eq!(format_age(Duration::from_secs(59)), "59s");
        assert_eq!(format_age(Duration::from_secs(60)), "1m");
        assert_eq!(format_age(Duration::from_secs(37 * 60 + 5)), "37m");
        assert_eq!(format_age(Duration::from_secs(3600)), "1h 0m");
        assert_eq!(
            format_age(Duration::from_secs(3600 + 12 * 60 + 9)),
            "1h 12m"
        );
        assert_eq!(
            format_age(Duration::from_secs(26 * 3600 + 3 * 60)),
            "26h 3m"
        );
    }

    #[test]
    fn role_line_pushes_the_stage_to_the_right_edge() {
        let line = role_line("api", "review", 20);
        assert_eq!(line, "  api         review");
        assert_eq!(line.chars().count(), 20);
    }

    #[test]
    fn role_line_truncates_a_long_role_and_keeps_the_stage() {
        let line = role_line("a-very-long-role-name", "working", 20);
        assert_eq!(line, "  a-very-lo… working");
        assert_eq!(line.chars().count(), 20);
    }

    #[test]
    fn role_line_never_drops_the_stage_in_a_tiny_pane() {
        let line = role_line("api", "working", 6);
        assert!(line.ends_with("working"), "{line:?}");
    }
}
