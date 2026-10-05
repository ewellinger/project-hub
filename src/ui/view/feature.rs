//! Feature view body: the role table with a cursor, then warnings, hints,
//! the selected role's review URL, the attach line, and the refresh line.
//! The block/placeholder prologue and the warnings/hints/attach/refresh
//! tail are shared with the dashboard status pane via `view::dashboard`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, TableState, Wrap};

use crate::ui::app::feature::display_order;
use crate::ui::app::{App, RowId, Screen};
use crate::ui::view::dashboard::{report_tail, status_prologue, status_table};
use hub::env::home_dir;
use hub::ops::status::StatusRow;

pub(super) fn render_feature(app: &App, frame: &mut Frame, area: Rect) {
    let Screen::Feature { name, .. } = &app.screen else {
        return;
    };
    let id = RowId::Feature(name.clone());
    let Some(inner) = status_prologue(app, frame, &id, " roles ", area) else {
        return;
    };
    let (feature, report) = app
        .feature_report()
        .expect("status_prologue returned an inner area only when the row is cached");

    let order = display_order(&report.rows);
    let rows: Vec<&StatusRow> = order.iter().map(|&i| &report.rows[i]).collect();
    let table_height = rows.len() as u16 + 1;
    let [table_area, tail_area] =
        Layout::vertical([Constraint::Length(table_height), Constraint::Min(0)]).areas(inner);
    let table = status_table(&rows, home_dir().as_deref())
        .highlight_symbol("▸ ")
        .row_highlight_style(Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED));
    // The cursor is a change index; its table row is that change's
    // position in the display order (row = change + 1).
    let highlighted = app
        .cursor()
        .and_then(|change| order.iter().position(|&row| row == change + 1));
    let mut state = TableState::default().with_selected(highlighted);
    frame.render_stateful_widget(table, table_area, &mut state);

    let review: Vec<Line> = app
        .selected_change()
        .and_then(|(c, _)| c.review_url.as_deref())
        .map(|url| Line::from(format!("review: {url}")))
        .into_iter()
        .collect();
    let attach = (feature.is_open() && app.tmux_on)
        .then(|| format!("Attach with: tmux attach -t {}", report.checkout));
    let tail = report_tail(app, &id, &report.rows, &report.warnings, attach, review);
    frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: false }), tail_area);
}
