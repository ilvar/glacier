//! The terminal browser: `legacy tui`.
//!
//! Three layers, kept apart on purpose. `app` holds the state and every
//! transition, with no terminal in sight. `render` turns that state into a
//! frame. This file owns the parts that genuinely need a TTY — raw mode,
//! the alternate screen, and the key loop — and is small enough to read in
//! one go, because it is the part that cannot be unit-tested.

pub mod app;
pub mod render;

use std::io::Stdout;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

use crate::core::vault::Vault;
use crate::tui::app::{App, Level, Mode};

/// How long to wait for a key before redrawing. Short enough that the
/// recording indicator feels live, long enough to stay idle at rest.
const TICK: Duration = Duration::from_millis(250);

/// Run the browser against `vault` until the operator quits.
pub fn run(vault: Vault) -> Result<(), String> {
    let mut app = App::new(vault)?;

    let mut terminal = setup()?;
    let outcome = event_loop(&mut terminal, &mut app);
    // Restore the terminal even if the loop failed: leaving someone in raw
    // mode with no echo is a much worse failure than whatever went wrong.
    let restored = restore(&mut terminal);

    outcome.and(restored)
}

type Backend = CrosstermBackend<Stdout>;

fn setup() -> Result<Terminal<Backend>, String> {
    enable_raw_mode().map_err(|error| format!("failed to enter raw mode: {error}"))?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .map_err(|error| format!("failed to enter the alternate screen: {error}"))?;
    Terminal::new(CrosstermBackend::new(stdout))
        .map_err(|error| format!("failed to start the terminal: {error}"))
}

fn restore(terminal: &mut Terminal<Backend>) -> Result<(), String> {
    disable_raw_mode().map_err(|error| format!("failed to leave raw mode: {error}"))?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .map_err(|error| format!("failed to leave the alternate screen: {error}"))?;
    terminal
        .show_cursor()
        .map_err(|error| format!("failed to restore the cursor: {error}"))
}

fn event_loop(terminal: &mut Terminal<Backend>, app: &mut App) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| render::draw(frame, app))
            .map_err(|error| format!("failed to draw: {error}"))?;

        if !event::poll(TICK).map_err(|error| format!("failed to poll for input: {error}"))? {
            continue;
        }
        let event = event::read().map_err(|error| format!("failed to read input: {error}"))?;

        // Windows reports both press and release; acting on both would
        // double every keystroke.
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Press {
                handle_key(terminal, app, key);
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key(terminal: &mut Terminal<Backend>, app: &mut App, key: KeyEvent) {
    match app.mode {
        Mode::Browse => handle_browse(app, key),
        Mode::Filter => handle_filter(app, key),
        Mode::Add => handle_add(terminal, app, key),
        Mode::Help => app.toggle_help(),
    }
}

fn handle_browse(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => app.quit(),
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        KeyCode::Char('g') | KeyCode::Home => app.select_first(),
        KeyCode::Char('G') | KeyCode::End => app.select_last(),
        KeyCode::PageDown => app.select_page(10, true),
        KeyCode::PageUp => app.select_page(10, false),
        KeyCode::Tab | KeyCode::Right => app.next_tab(),
        KeyCode::BackTab | KeyCode::Left => app.previous_tab(),
        KeyCode::Char('/') => app.begin_filter(),
        KeyCode::Esc => {
            app.clear_filter();
            app.set_status("filter cleared", Level::Info);
        }
        KeyCode::Char('a') => app.begin_add(),
        KeyCode::Char('r') => match app.reload() {
            Ok(()) => app.set_status("reloaded from disk", Level::Success),
            Err(error) => app.set_status(&error, Level::Error),
        },
        KeyCode::Char('?') => app.toggle_help(),
        _other => {}
    }
}

fn handle_filter(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.clear_filter();
            app.end_filter();
        }
        KeyCode::Enter => app.end_filter(),
        KeyCode::Backspace => app.pop_filter(),
        KeyCode::Char(character) => app.push_filter(character),
        _other => {}
    }
}

fn handle_add(terminal: &mut Terminal<Backend>, app: &mut App, key: KeyEvent) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);

    if control && matches!(key.code, KeyCode::Char('s')) {
        match app.save_draft() {
            Ok(path) => app.set_status(&format!("saved {path}"), Level::Success),
            Err(error) => app.set_status(&error, Level::Error),
        }
        return;
    }

    // Recording is bound to ctrl-r and F2 rather than a bare letter: every
    // printable key has to remain typeable while filling in a story.
    if control && matches!(key.code, KeyCode::Char('r')) {
        toggle_voice(terminal, app);
        return;
    }

    match key.code {
        KeyCode::F(2) => toggle_voice(terminal, app),
        KeyCode::Esc => {
            if app.is_recording() {
                app.set_status("stop the recording first (ctrl-r)", Level::Warning);
                return;
            }
            let lost = app.cancel_add();
            let message = if lost {
                "draft discarded"
            } else {
                "nothing to save"
            };
            app.set_status(message, Level::Info);
        }
        KeyCode::Tab => app.next_field(),
        KeyCode::BackTab => app.previous_field(),
        KeyCode::Enter => app.newline(),
        KeyCode::Backspace => app.backspace(),
        KeyCode::Left => app.cycle_visibility(false),
        KeyCode::Right => app.cycle_visibility(true),
        KeyCode::Char(character) => app.type_char(character),
        _other => {}
    }
}

/// Start or stop a voice take, redrawing in between because transcription
/// blocks on the network and the operator should see why.
fn toggle_voice(terminal: &mut Terminal<Backend>, app: &mut App) {
    if app.is_recording() {
        app.set_status("transcribing…", Level::Info);
        let _ignored = terminal.draw(|frame| render::draw(frame, app));

        match app.stop_voice() {
            Ok(message) => app.set_status(&message, Level::Success),
            Err(error) => app.set_status(&error, Level::Warning),
        }
        return;
    }

    match app.start_voice() {
        Ok(()) => app.set_status("recording — press ctrl-r to stop", Level::Warning),
        Err(error) => app.set_status(&error, Level::Error),
    }
}
