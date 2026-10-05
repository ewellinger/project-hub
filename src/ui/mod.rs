//! The dashboard: bare `hub` in a terminal. Only this module tree imports
//! `ratatui` and `crossterm`. Nothing here changes hub state.

pub mod app;
pub mod view;
pub mod worker;

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use crossterm::event as term;
use hub::error::{HubError, Result};
use hub::hub::Hub;
use hub::ops;
use ratatui::DefaultTerminal;

use app::{App, Effect, Event};

const POLL: Duration = Duration::from_millis(100);

/// Load the list, take over the terminal, run until quit, restore.
/// Errors before `ratatui::init` print normally; `init` also installs a
/// panic hook that restores the terminal.
pub fn run(hub: Hub) -> Result<()> {
    let features = ops::list::list(&hub)?;
    let mut app = App::new(&hub.manifest.name, features);
    app.repos = hub.manifest.repos.clone();
    app.checkout_template = hub.manifest.checkout_template().to_string();
    app.tmux_on = hub.tmux.is_some();
    app.current_session = hub
        .tmux
        .as_ref()
        .and_then(|t| t.current_session().unwrap_or(None));
    let effects = app.start(hub.cwd_feature.as_deref()).into();
    let (tx, rx) = mpsc::channel();
    let mut terminal = ratatui::try_init().map_err(|e| HubError::io("opening the terminal", e))?;
    let result = event_loop(&hub, &mut app, &mut terminal, &tx, &rx, effects);
    ratatui::restore();
    result
}

fn event_loop(
    hub: &Hub,
    app: &mut App,
    terminal: &mut DefaultTerminal,
    tx: &Sender<Event>,
    rx: &Receiver<Event>,
    mut effects: VecDeque<Effect>,
) -> Result<()> {
    loop {
        run_effects(hub, app, &mut effects, tx);
        terminal
            .draw(|frame| view::render(app, frame))
            .map_err(|e| HubError::io("drawing the dashboard", e))?;
        if app.quit {
            return Ok(());
        }
        let mut events = Vec::new();
        if term::poll(POLL).map_err(|e| HubError::io("polling the terminal", e))? {
            if let term::Event::Key(key) =
                term::read().map_err(|e| HubError::io("reading the terminal", e))?
            {
                events.push(Event::Key(key));
            }
        } else {
            events.push(Event::Tick);
        }
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        for event in events {
            effects.extend(app.update(event));
        }
    }
}

/// Execute effects; an effect whose result feeds back into the app may
/// queue further effects, hence the deque.
fn run_effects(hub: &Hub, app: &mut App, effects: &mut VecDeque<Effect>, tx: &Sender<Event>) {
    while let Some(effect) = effects.pop_front() {
        match effect {
            Effect::Request { row, r#gen } => worker::request(hub, row, r#gen, tx.clone()),
            Effect::OpenWorkspace(row) => worker::open_workspace(hub, row, tx.clone()),
            Effect::Copy { label, text } => worker::copy(label, text, tx.clone()),
            Effect::ReloadList => {
                effects.extend(app.update(Event::List(ops::list::list(hub))));
            }
            Effect::Run(action) => worker::run(hub, action, tx.clone()),
            Effect::LoadBranches { feature, role } => {
                worker::load_branches(hub, feature, role, tx.clone())
            }
            Effect::Preflight { name } => worker::preflight(hub, name, tx.clone()),
        }
    }
}
