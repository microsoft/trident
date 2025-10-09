use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
    io,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{Command as StdCommand, Output},
};

use log::trace;
use strum_macros::IntoStaticStr;

use trident_api::error::{
    ExecutionEnvironmentMisconfigurationError, ServicingError, TridentError, TridentResultExt,
};

#[derive(Debug, thiserror::Error)]
pub enum DependencyError {
    #[error("Failed to find dependency '{dependency}': {source}")]
    NotFound {
        dependency: Dependency,
        #[source]
        source: which::Error,
    },

    #[error("Failed to execute dependency '{dependency}': {inner}")]
    CouldNotExecute {
        dependency: Dependency,
        #[source]
        inner: io::Error,
    },

    #[error("Dependency '{dependency}' finished unsuccessfully: {explanation}\nCmdline: {rendered_command}\n{output}")]
    ExecutionFailed {
        dependency: Dependency,
        rendered_command: String,
        code: Option<i32>,
        signal: Option<i32>,
        stdout: String,
        stderr: String,
        explanation: String,
        output: String,
    },
}

impl From<DependencyError> for TridentError {
    #[track_caller]
    fn from(value: DependencyError) -> Self {
        match value {
            DependencyError::NotFound { dependency, source } => TridentError::with_source(
                ExecutionEnvironmentMisconfigurationError::MissingBinary {
                    binary: dependency.name(),
                },
                source.into(),
            ),
            DependencyError::CouldNotExecute { dependency, inner } => TridentError::with_source(
                ServicingError::CommandCouldNotExecute {
                    binary: dependency.name(),
                },
                inner.into(),
            ),
            DependencyError::ExecutionFailed {
                dependency,
                explanation,
                ..
            } => TridentError::new(ServicingError::CommandFailed {
                binary: dependency.name(),
                explanation,
            }),
        }
    }
}

pub trait DependencyResultExt<T> {
    /// Attach a context message to the error.
    fn message(self, context: impl Into<Cow<'static, str>>) -> Result<T, TridentError>;
}

impl<T> DependencyResultExt<T> for Result<T, Box<DependencyError>> {
    #[track_caller]
    fn message(self, context: impl Into<Cow<'static, str>>) -> Result<T, TridentError> {
        let result: Result<T, TridentError> = self.map_err(|e| (*e).into());
        result.message(context)
    }
}

/// Enum of runtime and test dependencies used in the code base.
#[derive(Debug, Clone, Copy, IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum Dependency {
    Blkid,
    Cryptsetup,
    Dd,
    Df,
    Dracut,
    E2fsck,
    Efivar,
    Efibootmgr,
    Eject,
    Findmnt,
    Iptables,
    Losetup,
    Lsblk,
    Lsof,
    Mdadm,
    Mkdir,
    Mkfs,
    Mkinitrd,
    Mkswap,
    Mount,
    Mountpoint,
    Netplan,
    Partx,
    Resize2fs,
    Setfiles,
    Sfdisk,
    Swapoff,
    Swapon,
    Systemctl,
    #[strum(serialize = "systemd-cryptenroll")]
    SystemdCryptenroll,
    #[strum(serialize = "systemd-firstboot")]
    SystemdFirstboot,
    #[strum(serialize = "systemd-pcrlock")]
    SystemdPcrlock,
    #[strum(serialize = "systemd-repart")]
    SystemdRepart,
    Touch,
    #[strum(serialize = "tpm2_clear")]
    Tpm2Clear,
    #[strum(serialize = "tpm2_pcrread")]
    Tpm2Pcrread,
    Tune2fs,
    Udevadm,
    Umount,
    Uname,
    Veritysetup,
    Wipefs,
    // Test dependencies
    #[cfg(test)]
    DoesNotExist,
    #[cfg(test)]
    Echo,
    #[cfg(test)]
    False,
}

impl std::fmt::Display for Dependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.into())
    }
}

impl Dependency {
    /// Gets the path for a dependency not in $PATH
    fn path_override(&self) -> Option<PathBuf> {
        Some(PathBuf::from(match self {
            Self::Netplan => "/usr/sbin/netplan",
            Self::SystemdPcrlock => "/usr/lib/systemd/systemd-pcrlock",
            _ => return None,
        }))
    }

    /// Gets the name of the dependency
    ///
    /// For example, Dependency::Mdadm => "mdadm"
    pub fn name(&self) -> &'static str {
        self.into()
    }

    /// Checks if the dependency is present in the system
    pub fn exists(&self) -> bool {
        self.path().is_ok()
    }

    /// Gets the path of the dependency
    pub fn path(&self) -> Result<PathBuf, Box<DependencyError>> {
        which::which(match self.path_override() {
            Some(path) => path,
            None => self.name().into(),
        })
        .map_err(|source| {
            Box::new(DependencyError::NotFound {
                dependency: *self,
                source,
            })
        })
    }

    /// Converts the dependency to a new Command instance
    /// (Note this does not create a std::process::Command instance)
    pub fn cmd(&self) -> Command {
        Command {
            dependency: *self,
            args: vec![],
            envs: vec![],
        }
    }
}

pub struct Command {
    dependency: Dependency,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
}

impl Command {
    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn with_arg<S: AsRef<OsStr>>(mut self, arg: S) -> Self {
        self.arg(arg);
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg.as_ref());
        }
        self
    }

    pub fn env<K, V>(&mut self, key: K, val: V) -> &mut Command
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.envs
            .push((key.as_ref().to_os_string(), val.as_ref().to_os_string()));
        self
    }

    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Command
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        for (ref key, ref val) in vars {
            self.envs
                .push((key.as_ref().to_os_string(), val.as_ref().to_os_string()));
        }
        self
    }

    pub fn run_and_check(&self) -> Result<(), Box<DependencyError>> {
        self.output()?.check()
    }

    pub fn output_and_check(&self) -> Result<String, Box<DependencyError>> {
        self.output()?.check_output()
    }

    pub fn raw_output_and_check(&self) -> Result<Output, Box<DependencyError>> {
        self.output()?.check_raw_output()
    }

    fn render_command(&self) -> String {
        if self.args.is_empty() {
            self.dependency.to_string()
        } else {
            format!(
                "{} {}",
                self.dependency,
                self.args
                    .iter()
                    .map(|arg| arg.to_string_lossy())
                    .map(|arg| if arg.contains(' ') {
                        format!("'{arg}'")
                    } else {
                        arg.into()
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
    }

    pub fn output(&self) -> Result<CommandOutput, Box<DependencyError>> {
        let mut cmd = StdCommand::new(self.dependency.path()?);
        cmd.args(&self.args);
        cmd.envs(self.envs.clone());
        let rendered_command = self.render_command();
        trace!("Executing '{rendered_command}'");
        let output = cmd
            .output()
            .map_err(|inner| DependencyError::CouldNotExecute {
                dependency: self.dependency,
                inner,
            })?;
        let output = CommandOutput {
            rendered_command: rendered_command.clone(),
            dependency: self.dependency,
            inner: output,
        };
        trace!(
            "Executed '{rendered_command}': {}. Report:\n{}",
            output.explain_exit(),
            output.output_report(),
        );
        Ok(output)
    }
}

#[derive(Debug)]
pub struct CommandOutput {
    rendered_command: String,
    dependency: Dependency,
    inner: Output,
}

impl CommandOutput {
    /// Checks if the process exited successfully
    pub fn success(&self) -> bool {
        self.inner.status.success()
    }

    /// Gets the exit code of the process, if it exited normally
    pub fn code(&self) -> Option<i32> {
        self.inner.status.code()
    }

    /// Gets the signal that terminated the process, if it was terminated by a signal
    fn signal(&self) -> Option<i32> {
        self.inner.status.signal()
    }

    /// Gets stderr
    pub fn error_output(&self) -> String {
        String::from_utf8_lossy(&self.inner.stderr).into()
    }

    /// Gets stdout
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.inner.stdout).into()
    }

    /// Gets all available output, useful for reporting or debugging
    pub fn output_report(&self) -> String {
        let stdout = self.output();
        let stderr = self.error_output();

        let mut res = String::with_capacity(stdout.len() + stderr.len() + 20);

        if !stdout.is_empty() {
            res += &format!("stdout:\n{stdout}\n");
        }

        if !stderr.is_empty() {
            if !res.is_empty() {
                res += "\n";
            }
            res += &format!("stderr:\n{stderr}\n");
        }

        res
    }

    /// Checks if the process exited successfully, otherwise produces an error
    pub fn check(&self) -> Result<(), Box<DependencyError>> {
        if self.success() {
            return Ok(());
        }

        Err(Box::new(DependencyError::ExecutionFailed {
            dependency: self.dependency,
            rendered_command: self.rendered_command.clone(),
            code: self.code(),
            signal: self.signal(),
            stdout: self.output(),
            stderr: self.error_output(),
            explanation: self.explain_exit(),
            output: match self.output_report() {
                s if !s.is_empty() => s,
                _ => "(no output collected)".into(),
            },
        }))
    }

    /// Checks if the process exited successfully and returns the output,
    /// otherwise produces an error with the output
    pub fn check_output(&self) -> Result<String, Box<DependencyError>> {
        self.check()?;
        Ok(self.output())
    }

    pub fn check_raw_output(self) -> Result<Output, Box<DependencyError>> {
        self.check()?;
        Ok(self.inner)
    }

    /// Produces a string explaining the exit status of the process
    fn explain_exit(&self) -> String {
        if let Some(code) = self.code() {
            format!("exited with status: {code}")
        } else if let Some(signal) = self.signal() {
            format!("terminated by signal: {signal}")
        } else {
            "exited with unknown status".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command() {
        let run_and_check_res = Dependency::Echo.cmd().arg("Hello, world").run_and_check();
        run_and_check_res.unwrap();
        let output_and_check_res = Dependency::Echo
            .cmd()
            .arg("Hello, world")
            .output_and_check();
        assert_eq!(output_and_check_res.unwrap(), "Hello, world\n");

        let raw_output_and_check_res = Dependency::Echo
            .cmd()
            .arg("Hello, world")
            .raw_output_and_check();
        assert_eq!(raw_output_and_check_res.unwrap().stdout, b"Hello, world\n");

        let render_command_res = Dependency::Echo.cmd().arg("Hello, world").render_command();
        assert_eq!(render_command_res, "echo 'Hello, world'");

        let output_res = Dependency::Echo.cmd().arg("Hello, world").output();
        assert_eq!(output_res.unwrap().output(), "Hello, world\n");
    }

    #[test]
    fn test_arg_and_args() {
        let arg = Dependency::Echo.cmd().arg("Hello, world").output();
        let args = Dependency::Echo.cmd().args(["Hello,", "world"]).output();

        let arg_output = arg.unwrap().output();
        let args_output = args.unwrap().output();
        assert_eq!(arg_output, args_output);
        assert_eq!(arg_output, "Hello, world\n");
    }

    #[test]
    fn test_nonexistent_dep() {
        let output = Dependency::DoesNotExist.cmd().output().unwrap_err();
        assert!(matches!(*output, DependencyError::NotFound { .. }));
        assert_eq!(
            output.to_string(),
            "Failed to find dependency 'doesnotexist': cannot find binary path"
        );
    }

    #[test]
    fn test_commandoutput() {
        // This command should succeed
        let output = Dependency::Echo.cmd().arg("Hello, world").output().unwrap();
        assert!(output.success());
        assert_eq!(output.code(), Some(0));
        assert_eq!(output.signal(), None);
        assert_eq!(output.error_output(), "");
        assert_eq!(output.output(), "Hello, world\n");
        assert_eq!(output.output_report(), "stdout:\nHello, world\n\n");
        assert!(matches!(output.check(), Ok(())));
        assert!(matches!(output.check_output(), Ok(s) if s == "Hello, world\n"));
        assert_eq!(output.explain_exit(), "exited with status: 0");

        // This command should fail
        let output = Dependency::False.cmd().output().unwrap();
        assert!(!output.success());
        assert_eq!(output.code(), Some(1));
        assert_eq!(output.signal(), None);
        assert_eq!(output.error_output(), "");
        assert_eq!(output.output(), "");
        assert_eq!(output.output_report(), "");
        assert!(matches!(
            *output.check().unwrap_err(),
            DependencyError::ExecutionFailed { .. }
        ));
        assert!(matches!(
            *output.check_output().unwrap_err(),
            DependencyError::ExecutionFailed { .. }
        ));
        assert_eq!(output.explain_exit(), "exited with status: 1");
    }
}
