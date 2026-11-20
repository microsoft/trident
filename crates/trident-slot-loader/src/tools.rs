//! Shell-free initrd helpers. Mounting and reboot authority have separate test seams.
//!
//! Keeping these adapters local avoids pulling the servicing engine's OS-wrapper dependency graph
//! into a boot-critical binary. Helpers inherit stderr so diagnostics are streamed, not buffered.

use std::{
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};

use anyhow::{ensure, Context, Error};
use log::debug;

pub(super) trait ImageMounter {
    /// Failure may leave a partial mount; the caller must attempt cleanup before releasing its path.
    fn mount(&self, image: &Path, target: &Path) -> Result<(), Error>;
    fn unmount(&self, target: &Path) -> Result<(), Error>;
}

pub(super) trait Rebooter {
    /// Requests a reset only; never modifies boot selection.
    fn reboot(&self) -> Result<(), Error>;
}

pub(super) struct System;

impl ImageMounter for System {
    fn mount(&self, image: &Path, target: &Path) -> Result<(), Error> {
        checked(
            Command::new("/usr/bin/systemd-dissect")
                .args([
                    "--no-pager",
                    "--read-only",
                    "--fsck=no",
                    "--growfs=no",
                    "--mount",
                ])
                .arg(image)
                .arg(target),
        )
    }

    fn unmount(&self, target: &Path) -> Result<(), Error> {
        checked(
            Command::new("/usr/bin/systemd-dissect")
                .args(["--no-pager", "--umount"])
                .arg(target),
        )
    }
}

impl Rebooter for System {
    fn reboot(&self) -> Result<(), Error> {
        checked(Command::new("/usr/bin/systemctl").args(["--no-block", "reboot"]))
    }
}

fn checked(command: &mut Command) -> Result<(), Error> {
    let start = Instant::now();
    debug!("event=helper_start command={command:?}");
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("executing {command:?}"))?;
    debug!(
        "event=helper_exit command={command:?} status={status} elapsed_ms={}",
        start.elapsed().as_millis()
    );
    ensure!(
        status.success(),
        "{command:?} returned {status}; helper diagnostics were written to stderr"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_start_and_exit_failures_have_command_context() {
        let error = checked(&mut Command::new("/usr/bin/false")).unwrap_err();
        assert!(format!("{error:#}").contains("/usr/bin/false"));
        let root = tempfile::tempdir().unwrap();
        let error = checked(&mut Command::new(root.path().join("missing-helper"))).unwrap_err();
        assert!(format!("{error:#}").contains("missing-helper"));
    }
}
