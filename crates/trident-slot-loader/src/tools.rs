use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{ensure, Context, Error};

pub(super) trait Tools {
    fn mount(&self, image: &Path, target: &Path) -> Result<(), Error>;
    fn unmount(&self, target: &Path) -> Result<(), Error>;
    fn reboot(&self) -> Result<(), Error>;
}

pub(super) struct System;

impl Tools for System {
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

    fn reboot(&self) -> Result<(), Error> {
        checked(Command::new("/usr/bin/systemctl").args(["--no-block", "reboot"]))
    }
}

fn checked(command: &mut Command) -> Result<(), Error> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("executing {command:?}"))?;
    ensure!(
        output.status.success(),
        "{command:?} returned {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
