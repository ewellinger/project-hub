//! Dashboard panes: the feature list and the status of the selected row.

use std::path::Path;
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, List, ListItem, ListState, Paragraph, Row as TableRow, Table, Wrap,
};

use crate::ui::app::feature::display_order;
use crate::ui::app::{App, Report, RowId};
use hub::env::{home_dir, shorten_home};
use hub::feature::FeatureStatus;
use hub::ops::format_age;
use hub::ops::review_state::{
    REVIEW_HEADERS, ReviewWord, now_secs, review_age_line, review_cells, viewed_quiet,
};
use hub::ops::status::{StateColour, StatusRow, closed, state_colour, state_text};

pub(super) const SPINNER: [&str; 4] = ["⠋", "⠙", "⠸", "⠴"];

/// Frozen reference roles: read-only checkouts, not members. Dim alone
/// already means merged or finished; italic marks "reference".
pub(super) fn read_only() -> Style {
    Style::new().add_modifier(Modifier::DIM | Modifier::ITALIC)
}

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
                text.extend(s.roles.iter().filter(|r| r.stage.is_open()).map(|r| {
                    let line = stage_line(&r.role, r.review.as_ref(), r.stage.as_str(), inner);
                    if r.viewed_quiet { dim_role(line) } else { line }
                }));
                text.extend(
                    s.frozen
                        .iter()
                        .map(|f| stage_line(&f.role, None, "frozen", inner).style(read_only())),
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

/// `word_line` with the stage coloured by value: working green, review
/// yellow, frozen grey, and the word by `ReviewWord::colour`. On the
/// selected row `REVERSED` turns the colour into the background, which
/// still reads.
fn stage_line(role: &str, word: Option<&ReviewWord>, stage: &str, width: usize) -> Line<'static> {
    let word_text = word.map(ToString::to_string);
    let mut text = word_line(role, word_text.as_deref(), stage, width);
    let kept = word.filter(|_| text != role_line(role, stage, width));
    let stage_text = text.split_off(text.len() - stage.len());
    let colour = match stage {
        "working" => Some(Color::Green),
        "review" => Some(Color::Yellow),
        "frozen" => Some(Color::DarkGray),
        _ => None,
    };
    let style = colour.map_or(Style::new(), |c| Style::new().fg(c));
    let Some(word) = kept else {
        return Line::from(vec![Span::raw(text), Span::styled(stage_text, style)]);
    };
    // `text` now ends in the word and its two-space gap.
    let word_text = text.split_off(text.len() - word.to_string().len() - 2);
    let word_style = word
        .colour()
        .map_or(Style::new(), |c| Style::new().fg(colour_of(c)));
    Line::from(vec![
        Span::raw(text),
        Span::styled(word_text.trim_end().to_string(), word_style),
        Span::raw("  "),
        Span::styled(stage_text, style),
    ])
}

/// Dims the role name alone in a `stage_line`: a review marked viewed that
/// has not moved. The indent and padding stay plain so the selected row's
/// `REVERSED` bar is not dimmed beside the name.
fn dim_role(mut line: Line<'static>) -> Line<'static> {
    let mut indent = line.spans.remove(0).content.into_owned();
    let pad = indent.split_off(indent.trim_end().len());
    let name = indent.split_off(indent.len().min(2));
    let dim = Style::new().add_modifier(Modifier::DIM);
    line.spans.splice(
        0..0,
        [Span::raw(indent), Span::styled(name, dim), Span::raw(pad)],
    );
    line
}

/// `role_line` with a word placed left of the stage; the word is dropped when
/// role, word and stage do not fit in `width`.
pub(super) fn word_line(role: &str, word: Option<&str>, stage: &str, width: usize) -> String {
    let len = |s: &str| s.chars().count();
    match word {
        Some(w) if 2 + len(role) + 1 + len(w) + 2 + len(stage) <= width => {
            role_line(role, &format!("{w}  {stage}"), width)
        }
        _ => role_line(role, stage, width),
    }
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
                (f.status == FeatureStatus::Open && app.tmux_on).then(|| {
                    format!(
                        "Attach with: tmux attach -t {}",
                        hub::tmux::session_name(&f.checkout)
                    )
                }),
            ),
        };
    // A finished feature keeps its review stages but is never read again.
    let review = match &cached.report {
        Report::Feature { report: f, .. } if f.status == FeatureStatus::Open => {
            review_table(&display_rows, inner.width)
        }
        _ => None,
    };
    let table_height = display_rows.len() as u16 + 1;
    let [table_area, review_area, tail_area] = review_layout(inner, table_height, &review);
    frame.render_widget(
        status_table(&display_rows, home_dir().as_deref()),
        table_area,
    );
    let extra = review_extra(&display_rows, review, frame, review_area);

    let tail = report_tail(app, &row.id, record_rows, warnings, attach, extra);
    frame.render_widget(Paragraph::new(tail).wrap(Wrap { trim: false }), tail_area);
}

/// The role table, the review table one blank row below it, and the tail.
pub(super) fn review_layout(
    inner: Rect,
    table_height: u16,
    review: &Option<(Table<'static>, u16)>,
) -> [Rect; 3] {
    let (gap, height) = review.as_ref().map_or((0, 0), |(_, h)| (1, *h));
    let [table, _, review, tail] = Layout::vertical([
        Constraint::Length(table_height),
        Constraint::Length(gap),
        Constraint::Length(height),
        Constraint::Min(0),
    ])
    .areas(inner);
    [table, review, tail]
}

/// Draws the review table, if any, and returns the dim age line for the tail.
pub(super) fn review_extra(
    rows: &[&StatusRow],
    review: Option<(Table<'static>, u16)>,
    frame: &mut Frame,
    area: Rect,
) -> Vec<Line<'static>> {
    let Some((table, _)) = review else {
        return Vec::new();
    };
    frame.render_widget(table, area);
    let dim = Style::new().add_modifier(Modifier::DIM);
    review_age_line(rows, now_secs())
        .map(|age| Line::styled(age, dim))
        .into_iter()
        .collect()
}

/// The review section for these rows, and its height; `None` when no row has cells.
pub(super) fn review_table(rows: &[&StatusRow], width: u16) -> Option<(Table<'static>, u16)> {
    let cells: Vec<_> = rows.iter().filter_map(|r| review_cells(r)).collect();
    if cells.is_empty() {
        return None;
    }
    let widest_cell = |i: usize| {
        cells
            .iter()
            .map(|c| c[i].0.chars().count())
            .max()
            .unwrap_or(0)
    };
    let full = |i: usize| widest_cell(i).max(REVIEW_HEADERS[i].len());
    let fits = (0..REVIEW_HEADERS.len()).map(full).sum::<usize>() + 12 <= width as usize;
    let widths: Vec<Constraint> = (0..REVIEW_HEADERS.len())
        .map(|i| {
            // Too narrow: headers clip so the values stay whole.
            let widest = if fits { full(i) } else { widest_cell(i) } as u16;
            // The other columns give first: a clipped role or request
            // number would read as another, valid one.
            if i < 2 {
                Constraint::Length(widest)
            } else {
                Constraint::Max(widest)
            }
        })
        .collect();
    let header = TableRow::new(REVIEW_HEADERS).style(Style::new().add_modifier(Modifier::BOLD));
    let height = cells.len() as u16 + 1;
    let body = cells.into_iter().map(|row| {
        TableRow::new(row.map(|(text, colour)| match colour {
            Some(c) => Cell::from(text).style(Style::new().fg(colour_of(c))),
            None => Cell::from(text),
        }))
    });
    let table = Table::new(body, widths).header(header).column_spacing(2);
    Some((table, height))
}

fn colour_of(c: StateColour) -> Color {
    match c {
        StateColour::Red => Color::Red,
        StateColour::Yellow => Color::Yellow,
        StateColour::Green => Color::Green,
        StateColour::Cyan => Color::Cyan,
    }
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
        // The branch is the distinguishing name; never truncate it.
        Constraint::Length(width_of(&|r| r.branch.chars().count(), 6, usize::MAX)),
        Constraint::Length(width_of(&|r| r.stage.chars().count(), 5, 8)),
        // Wide enough for a pinned change's whole state, such as
        // `behind pin <sha> by <n>, <base> +N since pin`. `Max` gives way
        // before the `Length` columns when the row is too narrow, so a long
        // state is cut instead of the branch.
        Constraint::Max(width_of(&|r| state_text(r).chars().count(), 5, 56)),
        Constraint::Min(4),
    ];
    let header = TableRow::new(["ROLE", "BRANCH", "STAGE", "STATE", "PATH"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let body = rows.iter().map(|r| {
        let colour = colour_of(state_colour(r));
        // Viewed and quiet: the name alone is dim, unlike a closed row.
        let role_style = if r.review.as_ref().is_some_and(viewed_quiet) {
            Style::new().add_modifier(Modifier::DIM)
        } else {
            Style::new()
        };
        let row = TableRow::new(vec![
            Cell::from(r.role.as_str()).style(role_style),
            Cell::from(r.branch.as_str()),
            Cell::from(r.stage.as_str()),
            Cell::from(state_text(r)).style(Style::new().fg(colour)),
            Cell::from(shorten_home(&r.path, home)),
        ]);
        if closed(&r.stage) {
            row.style(Style::new().add_modifier(Modifier::DIM))
        } else if r.stage == "frozen" {
            row.style(read_only())
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn stage_line_colours_only_the_stage() {
        for (stage, colour) in [
            ("working", Some(Color::Green)),
            ("review", Some(Color::Yellow)),
            ("frozen", Some(Color::DarkGray)),
            ("other", None),
        ] {
            let line = stage_line("api", None, stage, 20);
            assert_eq!(line.spans.len(), 2, "{stage}");
            assert_eq!(line.spans[0].style.fg, None, "{stage}");
            assert_eq!(line.spans[1].content, stage);
            assert_eq!(line.spans[1].style.fg, colour, "{stage}");
            assert_eq!(line.to_string(), role_line("api", stage, 20));
        }
    }

    #[test]
    fn dim_role_dims_the_name_and_nothing_else() {
        let plain = stage_line("api", Some(&ReviewWord::Approved), "review", 30);
        let line = dim_role(plain.clone());
        let dim = |s: &Span| s.style.add_modifier.contains(Modifier::DIM);
        assert_eq!(line.spans[0].content, "  ");
        assert_eq!(line.spans[1].content, "api");
        assert!(dim(&line.spans[1]));
        assert!(
            line.spans
                .iter()
                .enumerate()
                .all(|(i, s)| i == 1 || !dim(s)),
            "{line:?}"
        );
        assert_eq!(line.to_string(), plain.to_string());
        assert_eq!(line.spans.last().unwrap().style.fg, Some(Color::Yellow));
    }

    #[test]
    fn dim_role_covers_a_truncated_name_and_survives_no_name() {
        let line = dim_role(stage_line("a-very-long-role-name", None, "working", 20));
        assert_eq!(line.spans[1].content, "a-very-lo…");
        let tiny = stage_line("api", None, "working", 6);
        assert_eq!(dim_role(tiny.clone()).to_string(), tiny.to_string());
    }

    #[test]
    fn role_line_never_drops_the_stage_in_a_tiny_pane() {
        let line = role_line("api", "working", 6);
        assert!(line.ends_with("working"), "{line:?}");
    }
}
