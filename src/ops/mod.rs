pub mod add;
pub mod config;
pub mod finish;
pub mod init;
pub mod list;
pub mod merged;
pub mod pull;
pub mod repo;
pub mod resolve;
pub mod review;
pub mod session;
pub mod set_base;
pub mod start;
pub mod start_review;
pub mod status;
pub mod suggest;

use std::io::IsTerminal;

use comfy_table::presets::{NOTHING, UTF8_FULL_CONDENSED};
use comfy_table::{Attribute, Cell, ContentArrangement, Table};

/// Render a table that wraps to the terminal width and stays plain when piped.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> Vec<String> {
    let rows = rows
        .iter()
        .map(|row| row.iter().map(Cell::new).collect())
        .collect();
    styled_table(headers, rows)
}

/// Render a table whose callers provide styled cells.
pub fn styled_table(headers: &[&str], rows: Vec<Vec<Cell>>) -> Vec<String> {
    render_table(headers, rows, std::io::stdout().is_terminal(), None)
}

/// Columns take their content width when the table fits the terminal; when
/// it does not, comfy-table wraps the widest columns. No column has a fixed
/// cap or floor, so a wide terminal is used before anything wraps.
fn render_table(
    headers: &[&str],
    rows: Vec<Vec<Cell>>,
    terminal: bool,
    width: Option<u16>,
) -> Vec<String> {
    let header: Vec<Cell> = headers
        .iter()
        .map(|header| {
            let cell = Cell::new(header);
            if terminal {
                cell.add_attribute(Attribute::Bold)
            } else {
                cell
            }
        })
        .collect();
    let mut table = Table::new();
    table
        .load_style(if terminal {
            UTF8_FULL_CONDENSED
        } else {
            NOTHING
        })
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(header)
        .add_rows(rows);

    if terminal {
        table.enforce_styling();
        if let Some(width) = width {
            table.set_width(width);
        }
    } else {
        table.force_no_tty();
        match table.column_count() {
            0 => {}
            1 => {
                table
                    .column_mut(0)
                    .expect("column exists")
                    .set_padding((0, 0));
            }
            count => {
                table
                    .column_mut(0)
                    .expect("column exists")
                    .set_padding((0, 1));
                table
                    .column_mut(count - 1)
                    .expect("column exists")
                    .set_padding((1, 0));
            }
        }
    }

    table
        .lines()
        .map(|line| {
            if terminal {
                line
            } else {
                line.trim_end().to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use comfy_table::{Cell, Color};

    #[test]
    fn redirected_table_aligns_columns_without_decoration() {
        let lines = super::render_table(
            &["A", "Bee"],
            vec![
                vec![Cell::new("long"), Cell::new("x")],
                vec![Cell::new("y"), Cell::new("zz")],
            ],
            false,
            None,
        );
        assert_eq!(lines, vec!["A     Bee", "long  x", "y     zz"]);
        assert!(!lines.join("\n").contains('\u{1b}'));
    }

    #[test]
    fn terminal_table_wraps_to_its_width_and_uses_a_frame() {
        let lines = super::render_table(
            &["ROLE", "DESCRIPTION"],
            vec![vec![
                Cell::new("api"),
                Cell::new("A long description that needs to wrap cleanly"),
            ]],
            true,
            Some(36),
        );

        assert_eq!(lines[0].chars().count(), 36);
        assert!(lines[0].starts_with('┌'));
        assert!(lines[0].ends_with('┐'));
        assert!(
            lines
                .iter()
                .filter(|line| line.contains("description"))
                .count()
                == 1
        );
        assert!(lines.iter().any(|line| line.contains("cleanly")));
    }

    #[test]
    fn terminal_table_handles_common_terminal_widths() {
        let description = format!("{} END", "long description ".repeat(20));

        for width in [80, 120, 200] {
            let lines = super::render_table(
                &["ROLE", "CLONE", "BASE", "BRANCH TEMPLATE", "DESCRIPTION"],
                vec![vec![
                    Cell::new("api"),
                    Cell::new("acme-api"),
                    Cell::new("master"),
                    Cell::new("{feature}"),
                    Cell::new(&description),
                ]],
                true,
                Some(width),
            );

            assert!(lines[0].chars().count() <= usize::from(width));
            assert!(lines.iter().any(|line| line.contains("END")));
        }
    }

    #[test]
    fn terminal_table_keeps_fitting_content_on_one_line() {
        let branch = "jdoe/acmeOrders-audit-e2e-checkout-flow-retry2";
        let path = "~/workspace/worktrees/acme-billing-relay/jdoe-acmeOrders-audit-e2e-checkout-flow-retry2";
        let lines = super::render_table(
            &["ROLE", "BRANCH", "STAGE", "STATE", "PATH"],
            vec![vec![
                Cell::new("bff"),
                Cell::new(branch),
                Cell::new("working"),
                Cell::new("ok"),
                Cell::new(path),
            ]],
            true,
            Some(200),
        );

        // top border, header, rule, one data row, bottom border
        assert_eq!(lines.len(), 5, "{lines:#?}");
        assert!(lines[3].contains(branch), "{:?}", lines[3]);
        assert!(lines[3].contains(path), "{:?}", lines[3]);
    }

    #[test]
    fn terminal_table_emits_cell_styles_but_redirected_table_does_not() {
        let rows = || vec![vec![Cell::new("ok").fg(Color::Green)]];
        let terminal = super::render_table(&["STATE"], rows(), true, Some(20)).join("\n");
        let redirected = super::render_table(&["STATE"], rows(), false, None).join("\n");

        assert!(terminal.contains('\u{1b}'));
        assert!(!redirected.contains('\u{1b}'));
    }
}
