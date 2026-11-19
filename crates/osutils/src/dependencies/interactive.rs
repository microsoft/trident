use std::{
    fs::File,
    io,
    os::unix::process::CommandExt,
    process::{Command as StdCommand, Output},
};

use nix::{
    sys::signal::{self, SigSet, SigmaskHow, Signal},
    unistd::{self, Pid},
};

use super::{Command, CommandOutput, DependencyError};

impl Command {
    /// Runs a command on its controlling TTY in a separate foreground process group.
    ///
    /// Unlike `output()`, a shell needs the real TTY for job control, and keyboard
    /// signals must reach the shell rather than its caller. The former foreground
    /// group is restored after exit; nonzero exits become [`DependencyError`].
    pub fn run_interactive(&self, terminal: &File) -> Result<(), Box<DependencyError>> {
        let binary = self.dependency.path()?;
        let execute = || -> Result<Output, io::Error> {
            if self.stdin_data.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Interactive commands cannot pipe stdin",
                ));
            }
            let parent_group = unistd::tcgetpgrp(terminal)?;
            let mut command = StdCommand::new(&binary);
            // A separate group makes keyboard signals target the shell, not the installer.
            command
                .args(&self.args)
                .envs(self.envs.clone())
                .stdin(terminal.try_clone()?)
                .stdout(terminal.try_clone()?)
                .stderr(terminal.try_clone()?)
                .process_group(0);
            let mut child = command.spawn()?;
            let child_group = Pid::from_raw(i32::try_from(child.id()).map_err(io::Error::other)?);
            let foreground = set_foreground_group(terminal, child_group);
            if let Err(error) = foreground {
                child.kill()?;
                child.wait()?;
                return Err(error);
            }
            // The child can stop on SIGTTIN if it reads before the parent hands over the TTY.
            let continued = signal::killpg(child_group, Signal::SIGCONT);
            let status = child.wait();
            let restored = set_foreground_group(terminal, parent_group);
            restored?;
            continued?;
            // Output went directly to the TTY, not to pipes we could collect here.
            Ok(Output {
                status: status?,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        };
        CommandOutput {
            rendered_command: self.render_command(),
            dependency: self.dependency,
            inner: execute().map_err(|inner| DependencyError::CouldNotExecute {
                dependency: self.dependency,
                inner,
            })?,
        }
        .check()
    }
}

/// A background process must block SIGTTOU to reclaim the TTY from its child.
pub fn set_foreground_group(terminal: &File, group: Pid) -> io::Result<()> {
    let mut blocked = SigSet::empty();
    blocked.add(Signal::SIGTTOU);
    let mut previous = SigSet::empty();
    signal::pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous))?;
    let result = unistd::tcsetpgrp(terminal, group);
    signal::pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None)?;
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dependencies::Dependency;

    #[test]
    fn interactive_command_rejects_piped_stdin() {
        let terminal = File::open("/dev/null").unwrap();
        let error = Dependency::Echo
            .cmd()
            .with_stdin(b"not a terminal".to_vec())
            .run_interactive(&terminal)
            .unwrap_err();
        assert!(matches!(
            *error,
            DependencyError::CouldNotExecute { inner, .. }
                if inner.kind() == io::ErrorKind::InvalidInput
        ));
    }
}
