//! Terminal adapter: rendering, navigation, input and display ownership only.

use std::{
    io::{self, IsTerminal},
    time::Duration,
};

use anyhow::Error;
use crossterm::{
    cursor::MoveTo,
    event::{self, Event as TerminalEvent},
    execute,
    terminal::{
        self, Clear as ClearTerminal, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use log::error;
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::{
    diagnostics,
    engine::{Command, Frontend, State, Update},
    terminal as installer_terminal,
};

mod model;
mod render;
mod theme;

use model::{Model, Screen};
use theme::VERBOSITY_LEVELS;

/// Bound each input batch so pasted text cannot starve the engine's next tick.
const INPUT_EVENTS_PER_FRAME: usize = 256;

/// Owns the alternate screen and restores terminal state even on early exit.
struct Display {
    /// Ratatui draws to stdout, which may be an attached graphical console.
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    /// Only restore raw mode and the alternate screen when the UI owns them.
    active: bool,
    /// Linux VTs need a programmed ANSI palette; other terminals use RGB directly.
    linux_vt: bool,
}

impl Display {
    /// Creates a terminal display and enters its alternate screen.
    fn new(linux_vt: bool) -> Result<Self, Error> {
        let mut display = Self {
            terminal: Terminal::new(CrosstermBackend::new(io::stdout()))?,
            active: false,
            linux_vt,
        };
        display.resume()?;
        Ok(display)
    }

    /// Restores raw-mode drawing and the VT palette after a shell exits.
    fn resume(&mut self) -> Result<(), Error> {
        terminal::enable_raw_mode()?;
        self.active = true;
        if self.linux_vt {
            installer_terminal::set_vt_palette(&mut io::stdout(), &theme::vt_rgb_palette()?)?;
        }
        execute!(self.terminal.backend_mut(), EnterAlternateScreen)?;
        self.terminal.clear()?;
        Ok(())
    }

    /// Leaves raw mode and the alternate screen so an interactive shell can use the terminal.
    /// On Linux VTs, also clears the old UI and restores the original palette.
    fn suspend(&mut self) -> Result<(), Error> {
        let restore = self.leave_screen();
        let palette = if self.linux_vt {
            installer_terminal::reset_vt_palette(&mut io::stdout())
        } else {
            Ok(())
        };
        self.active = false;
        restore?;
        palette?;
        Ok(())
    }

    /// Releases the screen before palette cleanup, which must run even if this fails.
    fn leave_screen(&mut self) -> Result<(), Error> {
        terminal::disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        if self.linux_vt {
            // Linux VT leaves the old TUI visible when exiting its alternate screen.
            execute!(
                self.terminal.backend_mut(),
                ClearTerminal(ClearType::All),
                MoveTo(0, 0)
            )?;
        }
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for Display {
    /// Attempts terminal restoration if the display was not explicitly suspended.
    fn drop(&mut self) {
        if self.active {
            if let Err(error) = self.suspend() {
                error!("Failed to restore terminal: {error:#}");
            }
        }
    }
}

/// Presentation adapter for both the full-screen console and line-oriented/headless output.
pub(crate) struct Console {
    /// Navigation, filters and display-only history.
    model: Model,
    /// Absent in plain and headless operation.
    display: Option<Display>,
    /// Captured before input handling; headless runs never offer recovery menus.
    interactive: bool,
    /// Prevent plain records from interrupting the recovery shell.
    suspended: bool,
    /// Avoid repeating unchanged menus in the line-oriented frontend.
    printed_screen: Option<(Screen, bool)>,
}

impl Console {
    /// Opens a display only for an interactive terminal when `plain` is false.
    /// No source discovery, servicing task, logger, or power operation is started here.
    pub(crate) fn new(plain: bool, linux_vt: bool) -> Result<Self, Error> {
        let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
        let display = if interactive && !plain {
            Some(Display::new(linux_vt)?)
        } else {
            None
        };
        let mut model = Model::new();
        model.linux_vt = linux_vt;
        Ok(Self {
            model,
            display,
            interactive,
            suspended: false,
            printed_screen: None,
        })
    }
}

impl Frontend for Console {
    /// Headless sessions cannot supply recovery commands.
    fn interactive(&self) -> bool {
        self.interactive
    }

    /// Observe engine output without changing its state; filters affect only this display.
    fn update(&mut self, state: &State, update: Update) {
        self.model.state = state.clone();
        match update {
            Update::Record(record) => {
                if self.display.is_none()
                    && !self.suspended
                    && diagnostics::log_visible(record.level, self.model.verbosity)
                {
                    println!("{}", record.plain());
                }
                self.model.record(state, record);
            }
            Update::AttemptStarted => self.model.begin_attempt(),
            Update::Interrupted { active } => self.model.interrupt(active),
        }
    }

    /// Paint the snapshot or print a changed line-oriented menu.
    fn draw(&mut self, state: &State) -> Result<(), Error> {
        self.model.state = state.clone();
        if let Some(display) = &mut self.display {
            display
                .terminal
                .draw(|frame| render::render(frame, &self.model))?;
        } else if self.printed_screen != Some((self.model.screen, self.model.verbosity_open)) {
            if self.model.verbosity_open {
                println!("\nLOG VERBOSITY (display only)");
                for (index, level) in VERBOSITY_LEVELS.iter().enumerate() {
                    println!("{}: {level}", index + 1);
                }
                println!("Number + Enter: apply; Esc: cancel");
            } else {
                println!(
                    "\n{}\n{}",
                    render::title(self.model.screen),
                    diagnostics::display_text(&state.details)
                );
                for (index, choice) in self.model.choices().iter().enumerate() {
                    println!("{}: {}", index + 1, choice.label());
                }
                if matches!(self.model.screen, Screen::StreamUrl | Screen::HostConfigUrl) {
                    println!("Enter an http:// or https:// URL, then press Enter");
                }
                println!("Number + Enter: select; S + Enter: shell; Esc: menu; V: verbosity");
            }
            self.printed_screen = Some((self.model.screen, self.model.verbosity_open));
        }
        Ok(())
    }

    /// Consume ready keys until one command is requested; navigation has no side effects.
    fn poll(&mut self, state: &State, active: bool) -> Result<Option<Command>, Error> {
        self.model.state = state.clone();
        for _ in 0..INPUT_EVENTS_PER_FRAME {
            if !event::poll(Duration::ZERO)? {
                break;
            }
            if let TerminalEvent::Key(key) = event::read()? {
                if let Some(command) = self.model.key(key, active) {
                    return Ok(Some(command));
                }
            }
        }
        Ok(None)
    }

    /// Release drawing while the engine runs the recovery shell.
    fn suspend(&mut self) -> Result<(), Error> {
        if let Some(display) = &mut self.display {
            display.suspend()?;
        }
        self.suspended = true;
        println!("\nShell: type exit to return. Installation continues; automatic reboot waits.");
        Ok(())
    }

    /// Resume the latest presentation; servicing may have finished while the shell was open.
    fn resume(&mut self) -> Result<(), Error> {
        if let Some(display) = &mut self.display {
            display.resume()?;
        }
        self.suspended = false;
        self.printed_screen = None;
        Ok(())
    }
}
