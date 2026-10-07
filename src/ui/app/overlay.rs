//! The five overlay kinds and their key handling. An overlay owns its
//! cursor and text; what its answer means is the flow's business.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Answer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Warn,
    Error,
    Dim,
}

/// Live text derived from an input's value, drawn beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// `names::checkout_name(template, value, hub)`.
    Checkout { template: String, hub: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Select {
        title: String,
        rows: Vec<String>,
        filter: String,
        cursor: usize,
        /// Rows are still being built on a worker; only Esc works.
        loading: bool,
        /// A row the filter never hides; Enter on it while a filter is
        /// typed answers `Text(filter)` instead of `Picked`.
        pinned: Option<usize>,
    },
    MultiSelect {
        title: String,
        rows: Vec<String>,
        chosen: Vec<bool>,
        cursor: usize,
        help: String,
    },
    Input {
        title: String,
        value: String,
        preview: Option<Preview>,
        error: Option<String>,
        /// Waiting on a worker for this value; only Esc works.
        busy: bool,
    },
    Confirm {
        title: String,
        lines: Vec<(String, Tone)>,
        /// Offer `F` as accept-with-force.
        force: bool,
        /// Why accepting is impossible; replaces the key line, Enter and `F` are inert.
        blocked: Option<String>,
    },
    Result {
        title: String,
        lines: Vec<String>,
        is_error: bool,
    },
    /// Every key of the current screen. Read-only: `?`, Esc, and `q` close
    /// it and nothing else is handled.
    Help {
        title: String,
        rows: Vec<(String, String)>,
    },
}

/// Help rows per screen, `(key, description)`. The footer legends in
/// `view` show only the frequent keys; these list all of them.
pub const DASHBOARD_HELP: &[(&str, &str)] = &[
    ("↑↓/jk", "select a row"),
    ("⏎", "open the feature"),
    ("n", "start a new feature"),
    ("a", "show or hide finished features"),
    ("r", "reload the list and refresh the selected row"),
    ("o", "open the selected row's workspace"),
    ("p", "pull the selected row's checkouts, fast-forward only"),
    ("?", "this help"),
    ("q Esc", "quit"),
];

pub const FEATURE_HELP: &[(&str, &str)] = &[
    ("↑↓/jk", "select a role"),
    ("c", "copy the branch, path, or review URL"),
    ("v", "record the review URL"),
    ("m", "mark the role merged and remove its worktree"),
    ("b", "set the branch the role merges into"),
    ("+", "add a role"),
    ("f", "finish the feature"),
    ("o", "open the feature's workspace"),
    ("p", "pull every open role, fast-forward only"),
    ("r", "refresh"),
    ("?", "this help"),
    ("Esc q", "back to the dashboard"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Answer(Answer),
    Cancel,
}

impl Overlay {
    pub fn select(title: String, rows: Vec<String>) -> Overlay {
        Overlay::Select {
            title,
            rows,
            filter: String::new(),
            cursor: 0,
            loading: false,
            pinned: None,
        }
    }

    /// Keep select row `row` visible under any filter (see `pinned`).
    pub fn with_pinned(mut self, row: usize) -> Overlay {
        if let Overlay::Select { pinned, .. } = &mut self {
            *pinned = Some(row);
        }
        self
    }

    pub fn loading(title: String) -> Overlay {
        Overlay::Select {
            title,
            rows: Vec::new(),
            filter: String::new(),
            cursor: 0,
            loading: true,
            pinned: None,
        }
    }

    pub fn multi(title: String, rows: Vec<String>, help: String) -> Overlay {
        let chosen = vec![false; rows.len()];
        Overlay::MultiSelect {
            title,
            rows,
            chosen,
            cursor: 0,
            help,
        }
    }

    pub fn input(title: String, value: String, preview: Option<Preview>) -> Overlay {
        Overlay::Input {
            title,
            value,
            preview,
            error: None,
            busy: false,
        }
    }

    pub fn confirm(title: String, lines: Vec<(String, Tone)>, force: bool) -> Overlay {
        Overlay::Confirm {
            title,
            lines,
            force,
            blocked: None,
        }
    }

    pub fn result(title: String, lines: Vec<String>, is_error: bool) -> Overlay {
        Overlay::Result {
            title,
            lines,
            is_error,
        }
    }

    pub fn help(title: &str, rows: &[(&str, &str)]) -> Overlay {
        Overlay::Help {
            title: title.to_string(),
            rows: rows
                .iter()
                .map(|(k, d)| (k.to_string(), d.to_string()))
                .collect(),
        }
    }

    /// Attach a validation message to an input; other kinds are returned unchanged.
    pub fn with_error(mut self, message: String) -> Overlay {
        if let Overlay::Input { error, busy, .. } = &mut self {
            *error = Some(message);
            *busy = false;
        }
        self
    }

    /// Indices of a select's rows that contain the filter, case-insensitively,
    /// plus the pinned row.
    pub fn filtered(&self) -> Vec<usize> {
        match self {
            Overlay::Select {
                rows,
                filter,
                pinned,
                ..
            } => {
                let needle = filter.to_lowercase();
                rows.iter()
                    .enumerate()
                    .filter(|(i, r)| Some(*i) == *pinned || r.to_lowercase().contains(&needle))
                    .map(|(i, _)| i)
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        if key.code == KeyCode::Esc {
            return match self {
                Overlay::Result { .. } => Outcome::Answer(Answer::Dismissed),
                _ => Outcome::Cancel,
            };
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self {
            Overlay::Select { loading: true, .. } => Outcome::Stay,
            Overlay::Select { .. } => {
                // The immutable borrow for `filtered` ends before `self` is
                // re-matched mutably.
                let visible = self.filtered();
                let Overlay::Select {
                    filter,
                    cursor,
                    pinned,
                    ..
                } = self
                else {
                    unreachable!()
                };
                match key.code {
                    KeyCode::Up => step(cursor, -1, visible.len()),
                    KeyCode::Down => step(cursor, 1, visible.len()),
                    KeyCode::Enter => match visible.get(*cursor) {
                        Some(i) if Some(*i) == *pinned && !filter.is_empty() => {
                            return Outcome::Answer(Answer::Text(filter.clone()));
                        }
                        Some(i) => return Outcome::Answer(Answer::Picked(*i)),
                        None => return Outcome::Stay,
                    },
                    KeyCode::Backspace => {
                        filter.pop();
                        *cursor = 0;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        filter.push(c);
                        *cursor = 0;
                    }
                    _ => {}
                }
                Outcome::Stay
            }
            Overlay::MultiSelect {
                rows,
                chosen,
                cursor,
                ..
            } => {
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => step(cursor, -1, rows.len()),
                    KeyCode::Down | KeyCode::Char('j') => step(cursor, 1, rows.len()),
                    KeyCode::Char(' ') => {
                        if let Some(c) = chosen.get_mut(*cursor) {
                            *c = !*c;
                        }
                    }
                    KeyCode::Enter => {
                        let picked = chosen
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| **c)
                            .map(|(i, _)| i)
                            .collect();
                        return Outcome::Answer(Answer::PickedMany(picked));
                    }
                    _ => {}
                }
                Outcome::Stay
            }
            Overlay::Input {
                value, error, busy, ..
            } => {
                if *busy {
                    return Outcome::Stay;
                }
                match key.code {
                    KeyCode::Enter => return Outcome::Answer(Answer::Text(value.clone())),
                    KeyCode::Backspace => {
                        value.pop();
                        *error = None;
                    }
                    KeyCode::Char(c) if !ctrl => {
                        value.push(c);
                        *error = None;
                    }
                    _ => {}
                }
                Outcome::Stay
            }
            Overlay::Confirm { force, blocked, .. } => {
                if blocked.is_some() {
                    return Outcome::Stay;
                }
                match key.code {
                    KeyCode::Enter => Outcome::Answer(Answer::Confirmed { force: false }),
                    KeyCode::Char('F') if *force => {
                        Outcome::Answer(Answer::Confirmed { force: true })
                    }
                    _ => Outcome::Stay,
                }
            }
            Overlay::Result { .. } => match key.code {
                KeyCode::Enter => Outcome::Answer(Answer::Dismissed),
                _ => Outcome::Stay,
            },
            Overlay::Help { .. } => match key.code {
                KeyCode::Char('?') | KeyCode::Char('q') => Outcome::Cancel,
                _ => Outcome::Stay,
            },
        }
    }
}

/// Move a cursor by `delta` inside `len` rows, clamping.
fn step(cursor: &mut usize, delta: isize, len: usize) {
    if len == 0 {
        *cursor = 0;
        return;
    }
    let next = (*cursor as isize + delta).clamp(0, len as isize - 1);
    *cursor = next as usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn select_filters_by_typing_and_picks_the_original_index() {
        let mut o = Overlay::select(
            "Branch for api".into(),
            vec![
                "feature/x  new".into(),
                "main  local".into(),
                "type another name".into(),
            ],
        );
        assert_eq!(o.filtered(), vec![0, 1, 2]);
        assert_eq!(o.key(k(KeyCode::Char('m'))), Outcome::Stay);
        assert_eq!(o.filtered(), vec![1, 2], "'m' matches main and 'name'");
        assert_eq!(o.key(k(KeyCode::Down)), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Enter)), Outcome::Answer(Answer::Picked(2)));
        assert_eq!(o.key(k(KeyCode::Backspace)), Outcome::Stay);
        assert_eq!(o.filtered(), vec![0, 1, 2]);
        assert_eq!(
            o.key(k(KeyCode::Char('j'))),
            Outcome::Stay,
            "j types, it does not move"
        );
        assert!(o.filtered().is_empty());
        assert_eq!(o.key(k(KeyCode::Enter)), Outcome::Stay, "nothing to pick");
        assert_eq!(o.key(k(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn pinned_row_survives_the_filter_and_answers_with_the_filter_text() {
        let mut o = Overlay::select(
            "Branch for api".into(),
            vec![
                "feature/x  new".into(),
                "main  local".into(),
                "type another name".into(),
            ],
        )
        .with_pinned(2);
        for c in "fix/zz".chars() {
            o.key(k(KeyCode::Char(c)));
        }
        assert_eq!(o.filtered(), vec![2], "only the pinned row is left");
        assert_eq!(
            o.key(k(KeyCode::Enter)),
            Outcome::Answer(Answer::Text("fix/zz".into()))
        );
        for _ in 0..6 {
            o.key(k(KeyCode::Backspace));
        }
        assert_eq!(o.key(k(KeyCode::Down)), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Down)), Outcome::Stay);
        assert_eq!(
            o.key(k(KeyCode::Enter)),
            Outcome::Answer(Answer::Picked(2)),
            "with no filter it is an ordinary row"
        );
    }

    #[test]
    fn loading_select_only_cancels() {
        let mut o = Overlay::loading("Branch for api".into());
        assert_eq!(o.key(k(KeyCode::Enter)), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn multi_select_toggles_with_space_and_confirms_the_chosen_indices() {
        let mut o = Overlay::multi(
            "Roles".into(),
            vec!["api".into(), "ui".into(), "bff".into()],
            "space toggles".into(),
        );
        assert_eq!(o.key(k(KeyCode::Char(' '))), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Char('j'))), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Down)), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Char(' '))), Outcome::Stay);
        assert_eq!(
            o.key(k(KeyCode::Enter)),
            Outcome::Answer(Answer::PickedMany(vec![0, 2]))
        );
        let mut o = Overlay::multi("Roles".into(), vec!["api".into()], String::new());
        assert_eq!(
            o.key(k(KeyCode::Enter)),
            Outcome::Answer(Answer::PickedMany(vec![])),
            "nothing chosen is allowed"
        );
    }

    #[test]
    fn input_edits_and_returns_its_text() {
        let mut o = Overlay::input("New feature".into(), String::new(), None);
        for c in "abc".chars() {
            o.key(k(KeyCode::Char(c)));
        }
        o.key(k(KeyCode::Backspace));
        assert_eq!(
            o.key(k(KeyCode::Enter)),
            Outcome::Answer(Answer::Text("ab".into()))
        );
        let mut busy = Overlay::input("x".into(), "ab".into(), None);
        if let Overlay::Input { busy: b, .. } = &mut busy {
            *b = true;
        }
        assert_eq!(busy.key(k(KeyCode::Char('z'))), Outcome::Stay);
        assert_eq!(busy.key(k(KeyCode::Enter)), Outcome::Stay);
        assert_eq!(busy.key(k(KeyCode::Esc)), Outcome::Cancel);
        let with_err = Overlay::input("x".into(), "ab".into(), None).with_error("bad".into());
        assert!(matches!(with_err, Overlay::Input { error: Some(e), .. } if e == "bad"));
    }

    #[test]
    fn confirm_offers_force_only_when_asked_and_is_inert_when_blocked() {
        let mut o = Overlay::confirm(
            "Finish x".into(),
            vec![("Kill tmux session x".into(), Tone::Plain)],
            true,
        );
        assert_eq!(
            o.key(k(KeyCode::Char('F'))),
            Outcome::Answer(Answer::Confirmed { force: true })
        );
        assert_eq!(
            o.key(k(KeyCode::Enter)),
            Outcome::Answer(Answer::Confirmed { force: false })
        );
        let mut no_force = Overlay::confirm("Start x".into(), vec![], false);
        assert_eq!(no_force.key(k(KeyCode::Char('F'))), Outcome::Stay);
        let mut blocked = Overlay::confirm("Finish x".into(), vec![], true);
        if let Overlay::Confirm { blocked: b, .. } = &mut blocked {
            *b = Some("running inside tmux session x".into());
        }
        assert_eq!(blocked.key(k(KeyCode::Enter)), Outcome::Stay);
        assert_eq!(blocked.key(k(KeyCode::Char('F'))), Outcome::Stay);
        assert_eq!(blocked.key(k(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn result_dismisses_on_enter_or_esc() {
        let mut o = Overlay::result("merged api".into(), vec!["a".into(), "b".into()], false);
        assert_eq!(o.key(k(KeyCode::Char('x'))), Outcome::Stay);
        assert_eq!(o.key(k(KeyCode::Enter)), Outcome::Answer(Answer::Dismissed));
        assert_eq!(o.key(k(KeyCode::Esc)), Outcome::Answer(Answer::Dismissed));
    }
}
