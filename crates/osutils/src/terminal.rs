use std::{
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Write},
    os::{fd::AsRawFd, unix::fs::FileTypeExt},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use nix::{
    libc,
    sys::signal::{self, SigSet, SigmaskHow, Signal},
    unistd::{self, Pid},
};

use crate::dependencies::Dependency;

const ACTIVE_CONSOLES_PATH: &str = "/sys/class/tty/console/active";
const GRAPHICAL_CONSOLE: &str = "tty1";
const VT_PALETTE_SIZE: u8 = 16;

/// Sets RGB values for the Linux virtual console's 16 ANSI colour slots.
pub fn set_vt_palette(output: &mut impl Write, colours: &[(u8, u8, u8, u8)]) -> io::Result<()> {
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

pub fn reset_vt_palette(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\x1b]R")?;
    output.flush()
}

pub fn active_consoles() -> Result<Vec<PathBuf>, Error> {
    let active = fs::read_to_string(ACTIVE_CONSOLES_PATH)
        .context("Failed to read active kernel consoles")?;
    console_paths(&active)
}

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

pub fn preferred_console(consoles: &[PathBuf]) -> Result<&Path, Error> {
    consoles
        .iter()
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name == GRAPHICAL_CONSOLE)
        })
        .or_else(|| consoles.first())
        .map(PathBuf::as_path)
        .context("No console available")
}

pub fn attach(path: impl AsRef<Path>) -> Result<File, Error> {
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
        unistd::setsid()?;
    }
    // The caller has stopped competing getty units before claiming the terminal.
    if unsafe { libc::ioctl(terminal.as_raw_fd(), libc::TIOCSCTTY, 1) } < 0 {
        return Err(io::Error::last_os_error()).context("Failed to claim controlling terminal");
    }
    set_foreground_group(&terminal, unistd::getpgrp())
        .context("Failed to make installer the terminal's foreground process group")?;
    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO] {
        // dup2 redirects the inherited stdio descriptors without transferring ownership.
        if unsafe { libc::dup2(terminal.as_raw_fd(), target) } < 0 {
            return Err(io::Error::last_os_error()).context("Failed to attach terminal");
        }
    }
    Ok(terminal)
}

pub(crate) fn set_foreground_group(terminal: &File, group: Pid) -> io::Result<()> {
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut previous = SigSet::empty();
    signal::pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous))?;
    let result = unistd::tcsetpgrp(terminal, group);
    signal::pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None)?;
    result?;
    Ok(())
}

pub fn shell(terminal: &File) -> Result<(), Error> {
    Dependency::Bash
        .cmd()
        .args(["--noprofile", "--norc", "-i"])
        .run_interactive(terminal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphical_console_wins_and_aliases_are_deduplicated() {
        let consoles = console_paths("ttyS0 tty0 tty1").unwrap();
        assert_eq!(
            consoles,
            [PathBuf::from("/dev/ttyS0"), PathBuf::from("/dev/tty1")]
        );
        assert_eq!(
            preferred_console(&consoles).unwrap(),
            Path::new("/dev/tty1")
        );
        console_paths("../tty1").unwrap_err();
        console_paths("").unwrap_err();
    }

    #[test]
    fn linux_vt_palette_sequences_set_rgb_slots_and_reset() {
        let mut output = Vec::new();
        set_vt_palette(&mut output, &[(0, 11, 17, 24), (15, 229, 234, 236)]).unwrap();
        reset_vt_palette(&mut output).unwrap();
        assert_eq!(output, b"\x1b]P00b1118\x1b]Pfe5eaec\x1b]R");
        set_vt_palette(&mut output, &[(16, 0, 0, 0)]).unwrap_err();
    }
}
