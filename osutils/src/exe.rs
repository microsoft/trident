use std::{
    os::unix::process::ExitStatusExt,
    process::{ExitStatus, Output},
};

use anyhow::{anyhow, Error};

use crate::crate_private::Sealed;

/// Extension for `std::process::Output` to easily check status, produce anyhow errors, and get output
/// This is a sealed trait, so it cannot be implemented outside of this crate.
pub trait OutputChecker: Sealed {
    /// Check if the process exited successfully
    fn is_success(&self) -> bool;

    /// Get the exit code of the process, if it exited normally
    fn exit_code(&self) -> Option<i32>;

    /// Get the signal that terminated the process, if it was terminated by a signal
    fn end_signal(&self) -> Option<i32>;

    /// Return the type of process that was running
    fn process_type(&self) -> &'static str {
        "process"
    }

    /// When available, get stderr, otherwise get stdout
    fn err_output(&self) -> String {
        "".into()
    }

    /// Get stdout
    fn output(&self) -> String {
        "".into()
    }

    /// Check if the process exited successfully, otherwise produce an error
    fn check(&self) -> Result<(), Error> {
        if self.is_success() {
            return Ok(());
        }

        Err(match self.err_output() {
            s if !s.is_empty() => anyhow!("Process output:\n{}", s).context(self.explain_exit()),
            _ => anyhow!("{} (No output was captured)", self.explain_exit()),
        })
    }

    /// Check if the process exited successfully and return the output, otherwise produce an error with the output
    fn check_output(&self) -> Result<String, Error> {
        self.check()?;
        Ok(self.output())
    }

    /// Produce a string explaining the exit status of the process
    fn explain_exit(&self) -> String {
        if let Some(code) = self.exit_code() {
            format!("{} exited with status: {code}", self.process_type())
        } else if let Some(signal) = self.end_signal() {
            format!("{} was terminated by signal: {signal}", self.process_type())
        } else {
            format!("{} exited with unknown status", self.process_type())
        }
    }
}

impl Sealed for Output {}

impl OutputChecker for Output {
    /// Check if the process exited successfully
    fn is_success(&self) -> bool {
        self.status.success()
    }

    /// Get the exit code of the process, if it exited normally
    fn exit_code(&self) -> Option<i32> {
        self.status.code()
    }

    /// Get the signal that terminated the process, if it was terminated by a signal
    fn end_signal(&self) -> Option<i32> {
        self.status.end_signal()
    }

    /// Get stderr if it's not empty, otherwise get stdout
    fn err_output(&self) -> String {
        if self.stderr.is_empty() {
            String::from_utf8_lossy(&self.stdout).into()
        } else {
            String::from_utf8_lossy(&self.stderr).into()
        }
    }

    /// Get stdout
    fn output(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into()
    }
}

impl Sealed for ExitStatus {}

impl OutputChecker for ExitStatus {
    fn is_success(&self) -> bool {
        self.success()
    }

    fn exit_code(&self) -> Option<i32> {
        self.code()
    }

    fn end_signal(&self) -> Option<i32> {
        self.signal()
    }
}
