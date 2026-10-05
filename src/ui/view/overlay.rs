//! Overlay boxes: centred over the dimmed screen.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::ui::app::{App, Overlay, Preview, Tone};
use crate::ui::view::dashboard::SPINNER;
use crate::ui::view::legend_line;
use hub::names;

/// Dim everything drawn so far, then draw the overlay on top.
pub(super) fn render_overlay(app: &App, frame: &mut Frame, screen: Rect) {
    let Some(overlay) = &app.overlay else {
        return;
    };
    for cell in frame.buffer_mut().content.iter_mut() {
        cell.set_style(Style::new().add_modifier(Modifier::DIM));
    }
    let (title, body) = match overlay {
        Overlay::Select {
            title,
            rows,
            filter,
            cursor,
            loading,
        } => (
            title,
            select_lines(overlay, rows, filter, *cursor, *loading, app.spinner),
        ),
        Overlay::MultiSelect {
            title,
            rows,
            chosen,
            cursor,
            help,
        } => (title, multi_lines(rows, chosen, *cursor, help)),
        Overlay::Input {
            title,
            value,
            preview,
            error,
            busy,
        } => (
            title,
            input_lines(
                value,
                preview.as_ref(),
                error.as_deref(),
                *busy,
                &app.hub_name,
                app.spinner,
            ),
        ),
        Overlay::Confirm {
            title,
            lines,
            force,
            blocked,
        } => (
            title,
            confirm_lines(title, lines, *force, blocked.as_deref()),
        ),
        Overlay::Result {
            title,
            lines,
            is_error,
        } => (title, result_lines(lines, *is_error)),
        Overlay::Help { title, rows } => (title, help_lines(rows)),
    };
    let width = body
        .iter()
        .map(|l| l.width())
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 2)
        .max(40) as u16
        + 4;
    let width = width.min(screen.width.saturating_sub(2));
    // Long lines (paths) wrap inside the box, so size the height by the rows
    // ratatui will render, not by the line count, or the tail lines fall off
    // the bottom. The last line is the key legend and gets its own row.
    let inner_width = width.saturating_sub(2).max(1);
    let (head, last) = body.split_at(body.len().saturating_sub(1));
    let head = Paragraph::new(head.to_vec()).wrap(Wrap { trim: false });
    let rows = head.line_count(inner_width) + last.len();
    let height = (u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(2))
        .min(screen.height.saturating_sub(2));
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(screen);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .title(format!(" {title} "))
        .style(Style::new());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [head_area, last_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);
    frame.render_widget(head, head_area);
    frame.render_widget(Paragraph::new(last.to_vec()), last_area);
}

fn toned(text: String, tone: Tone) -> Line<'static> {
    let style = match tone {
        Tone::Plain => Style::new(),
        Tone::Warn => Style::new().fg(Color::Yellow),
        Tone::Error => Style::new().fg(Color::Red),
        Tone::Dim => Style::new().add_modifier(Modifier::DIM),
    };
    Line::styled(text, style)
}

fn select_lines(
    overlay: &Overlay,
    rows: &[String],
    filter: &str,
    cursor: usize,
    loading: bool,
    spinner: usize,
) -> Vec<Line<'static>> {
    if loading {
        // Nothing to filter yet, and the only key that works is Esc.
        return vec![
            toned(
                format!("loading… {}", SPINNER[spinner % SPINNER.len()]),
                Tone::Dim,
            ),
            legend_line(&[("Esc", "cancel")]),
        ];
    }
    let mut out = vec![Line::from(format!("filter: {filter}"))];
    let visible = overlay.filtered();
    if visible.is_empty() {
        out.push(toned("no match".into(), Tone::Dim));
    }
    for (pos, i) in visible.iter().enumerate() {
        let marker = if pos == cursor { "▸ " } else { "  " };
        let line = Line::from(format!("{marker}{}", rows[*i]));
        out.push(if pos == cursor {
            line.style(Style::new().add_modifier(Modifier::BOLD))
        } else {
            line
        });
    }
    out.push(legend_line(&[
        ("type", "to filter"),
        ("⏎", "choose"),
        ("Esc", "cancel"),
    ]));
    out
}

fn multi_lines(rows: &[String], chosen: &[bool], cursor: usize, help: &str) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let marker = if i == cursor { "▸ " } else { "  " };
            let tick = if chosen[i] { "[x]" } else { "[ ]" };
            let line = Line::from(format!("{marker}{tick} {r}"));
            if i == cursor {
                line.style(Style::new().add_modifier(Modifier::BOLD))
            } else {
                line
            }
        })
        .collect();
    out.push(toned(help.to_string(), Tone::Dim));
    out.push(legend_line(&[
        ("space", "toggles"),
        ("⏎", "confirm"),
        ("Esc", "cancel"),
    ]));
    out
}

fn input_lines(
    value: &str,
    preview: Option<&Preview>,
    error: Option<&str>,
    busy: bool,
    hub: &str,
    spinner: usize,
) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(format!("> {value}"))];
    match preview {
        Some(Preview::Checkout {
            template,
            hub: hub_name,
        }) => {
            let hub = if hub_name.is_empty() { hub } else { hub_name };
            let checkout = if value.is_empty() {
                String::new()
            } else {
                names::checkout_name(template, value, hub)
            };
            out.push(toned(format!("checkout: {checkout}"), Tone::Dim));
        }
        None => {}
    }
    if let Some(e) = error {
        for l in e.lines() {
            out.push(toned(l.to_string(), Tone::Error));
        }
    }
    if busy {
        out.push(toned(
            format!("checking… {}", SPINNER[spinner % SPINNER.len()]),
            Tone::Dim,
        ));
    }
    out.push(legend_line(&[("⏎", "accept"), ("Esc", "cancel")]));
    out
}

fn confirm_lines(
    title: &str,
    lines: &[(String, Tone)],
    force: bool,
    blocked: Option<&str>,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line> = lines
        .iter()
        .map(|(t, tone)| toned(t.clone(), *tone))
        .collect();
    out.push(Line::from(""));
    match blocked {
        Some(why) => {
            // The blocker goes in the wrapped body, never on the fixed last
            // row: it is a sentence, and the last row does not wrap. The
            // last row is the one key that still works.
            for l in why.lines() {
                out.push(toned(l.to_string(), Tone::Error));
            }
            out.push(legend_line(&[("Esc", "cancel")]));
        }
        None => {
            let verb = title
                .split_whitespace()
                .next()
                .unwrap_or("confirm")
                .to_lowercase();
            let mut keys: Vec<(String, String)> = vec![("⏎".into(), verb.clone())];
            if force {
                keys.push(("F".into(), format!("{verb} with force")));
            }
            keys.push(("Esc".into(), "cancel".into()));
            out.push(legend_line(&keys));
        }
    }
    out
}

fn result_lines(lines: &[String], is_error: bool) -> Vec<Line<'static>> {
    let tone = if is_error { Tone::Error } else { Tone::Plain };
    let mut out: Vec<Line> = lines.iter().map(|l| toned(l.clone(), tone)).collect();
    out.push(Line::from(""));
    out.push(legend_line(&[("⏎", "close")]));
    out
}

fn help_lines(rows: &[(String, String)]) -> Vec<Line<'static>> {
    let width = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let mut out: Vec<Line> = rows
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("{key:<width$}"),
                    Style::new().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {desc}"),
                    Style::new().add_modifier(Modifier::DIM),
                ),
            ])
        })
        .collect();
    out.push(Line::from(""));
    out.push(legend_line(&[("?", "close"), ("Esc", "close")]));
    out
}
