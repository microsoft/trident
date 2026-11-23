//! Console ownership, serial mirrors and recovery-shell setup for installer boots.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use nix::{libc, unistd};

use osutils::dependencies::{self, Dependency};

/// Kernel list of consoles active for this boot.
const ACTIVE_CONSOLES_PATH: &str = "/sys/class/tty/console/active";
/// The installer service owns tty1 and stops its getty before attaching.
const GRAPHICAL_CONSOLE: &str = "tty1";
/// Linux virtual terminals expose 16 programmable ANSI color slots.
const VT_PALETTE_SIZE: u8 = 16;

/// Sets RGB values for the Linux virtual console's 16 ANSI colour slots.
pub(super) fn set_vt_palette(
    output: &mut impl Write,
    colours: &[(u8, u8, u8, u8)],
) -> io::Result<()> {
    for &(slot, red, green, blue) in colours {
        if slot >= VT_PALETTE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid Linux VT palette slot {slot}"),
            ));
        }
        write!(output, "\x1b]P{slot:x}{red:02x}{green:02x}{blue:02x}")?;
    }
    output.flush()
}

/// Restores the Linux virtual console's built-in ANSI colour palette.
pub(super) fn reset_vt_palette(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\x1b]R")?;
    output.flush()
}

/// Reads the active kernel consoles, resolving `tty0` to the graphical `tty1`.
pub(super) fn active_consoles() -> Result<Vec<PathBuf>, Error> {
    let active = fs::read_to_string(ACTIVE_CONSOLES_PATH)
        .context("Failed to read active kernel consoles")?;
    console_paths(&active)
}

/// Validates console names before producing unique `/dev` paths.
fn console_paths(active: &str) -> Result<Vec<PathBuf>, Error> {
    let mut consoles = Vec::new();
    for name in active.split_whitespace() {
        ensure!(
            name.chars().all(|c| c.is_ascii_alphanumeric()),
            "Invalid console name '{name}'"
        );
        let name = if name == "tty0" {
            GRAPHICAL_CONSOLE
        } else {
            name
        };
        let path = Path::new("/dev").join(name);
        if !consoles.contains(&path) {
            consoles.push(path);
        }
    }
    ensure!(!consoles.is_empty(), "No active kernel consoles found");
    Ok(consoles)
}

/// Opens a serial console without allowing flow control to block a caller's event loop.
pub(super) fn open_nonblocking_console(path: impl AsRef<Path>) -> Result<File, Error> {
    let path = path.as_ref();
    OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .with_context(|| format!("Failed to open console '{}'", path.display()))
}

/// Claims a character terminal as the controlling TTY and redirects stdin/stdout to it.
///
/// The caller must stop competing getty units before attaching.
pub(super) fn attach(path: impl AsRef<Path>) -> Result<File, Error> {
    let path = path.as_ref();
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("Failed to open terminal '{}'", path.display()))?;
    ensure!(
        terminal.metadata()?.file_type().is_char_device() && terminal.is_terminal(),
        "Not a terminal: {}",
        path.display()
    );
    if unistd::getsid(None)? != unistd::getpid() {
        // A session leader can claim tty1 as its controlling terminal.
        unistd::setsid()?;
    }
    // The caller has stopped competing getty units before claiming the terminal.
    if unsafe { libc::ioctl(terminal.as_raw_fd(), libc::TIOCSCTTY, 1) } < 0 {
        return Err(io::Error::last_os_error()).context("Failed to claim controlling terminal");
    }
    // The recovery shell later takes this foreground group and returns it on exit.
    dependencies::set_foreground_group(&terminal, unistd::getpgrp())
        .context("Failed to make installer the terminal's foreground process group")?;
    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO] {
        // Leave stderr on its inherited sink rather than drawing it over tty1.
        if unsafe { libc::dup2(terminal.as_raw_fd(), target) } < 0 {
            return Err(io::Error::last_os_error()).context("Failed to attach terminal");
        }
    }
    Ok(terminal)
}

/// Runs an interactive login Bash on the supplied terminal, loading the root profile.
pub(super) fn shell(terminal: &File) -> Result<(), Error> {
    Dependency::Bash
        .cmd()
        .args(["--login", "-i"])
        .run_interactive(terminal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Ensures graphical console aliases are deduplicated and invalid names fail.
    fn graphical_console_wins_and_aliases_are_deduplicated() {
        let consoles = console_paths("ttyS0 tty0 tty1").unwrap();
        assert_eq!(
            consoles,
            [PathBuf::from("/dev/ttyS0"), PathBuf::from("/dev/tty1")]
        );
        console_paths("../tty1").unwrap_err();
        console_paths("").unwrap_err();
    }

    #[test]
    /// Serial mirrors must not block the installer when a console stops draining.
    fn serial_console_is_nonblocking() {
        let terminal = nix::pty::openpty(None, None).unwrap();
        let path = unistd::ttyname(&terminal.slave).unwrap();
        let mirror = open_nonblocking_console(path).unwrap();
        let flags = unsafe { libc::fcntl(mirror.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::O_NONBLOCK, 0);
    }

    #[test]
    /// Ensures VT palette slots are encoded correctly and out-of-range slots fail.
    fn linux_vt_palette_sequences_set_rgb_slots_and_reset() {
        let mut output = Vec::new();
        set_vt_palette(&mut output, &[(0, 11, 17, 24), (15, 229, 234, 236)]).unwrap();
        reset_vt_palette(&mut output).unwrap();
        assert_eq!(output, b"\x1b]P00b1118\x1b]Pfe5eaec\x1b]R");
        set_vt_palette(&mut output, &[(16, 0, 0, 0)]).unwrap_err();
    }
}
