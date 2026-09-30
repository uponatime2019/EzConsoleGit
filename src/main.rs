//! EzConsoleGit — a terminal Git diff viewer and AI commit assistant.
//!
//! A fast, lightweight Rust TUI Git diff viewer and AI commit assistant. All
//! Git work shells out to the `git` CLI and the AI client talks to any
//! OpenAI-compatible `/v1/chat/completions` endpoint, both through the
//! shared, GUI-free `ezgit-core` crate.

mod app;
mod diff_lines;
mod grouping;
mod jobs;
mod text_input;
mod theme;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use crossterm::{cursor, execute};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> io::Result<()> {
    // Restore the terminal even if a panic unwinds mid-draw.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_terminal();
        default_hook(info);
    }));

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal);
    restore_terminal()?;
    result
}

fn setup_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        SetTitle("EzConsoleGit")
    )?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    )?;
    Ok(())
}

fn run(terminal: &mut Tui) -> io::Result<()> {
    let mut app = App::new();

    while !app.should_quit {
        if app.wants_cursor() {
            terminal.show_cursor()?;
        } else {
            terminal.hide_cursor()?;
        }

        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
        }

        app.pump();
        app.tick();
    }

    Ok(())
}
