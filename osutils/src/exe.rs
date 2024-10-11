use std::{
    os::unix::process::ExitStatusExt,
    process::{Command, ExitStatus, Output},
};

use anyhow::{anyhow, bail, Context, Error};
use log::trace;

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

    /// Get stdout
    fn output(&self) -> String {
        "".into()
    }

    /// Get stderr
    fn error_output(&self) -> String {
        "".into()
    }

    /// Get all available output, useful for reporting or debugging
    fn output_report(&self) -> String {
        let stdout = self.output();
        let stderr = self.error_output();

        let mut res = String::with_capacity(stdout.len() + stderr.len() + 20);

        if !stdout.is_empty() {
            res += &format!("stdout:\n{}\n", stdout);
        }

        if !stderr.is_empty() {
            if !res.is_empty() {
                res += "\n";
            }
            res += &format!("stderr:\n{}\n", stderr);
        }

        res
    }

    /// Check if the process exited successfully, otherwise produce an error
    fn check(&self) -> Result<(), Error> {
        if self.is_success() {
            return Ok(());
        }

        Err(match self.output_report() {
            s if !s.is_empty() => anyhow!("Process output:\n{}", s).context(self.explain_exit()),
            _ => anyhow!("(No output was captured)").context(self.explain_exit()),
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

    /// Get stderr
    fn error_output(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into()
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

impl Sealed for Result<Output, std::io::Error> {}

impl OutputChecker for Result<Output, std::io::Error> {
    /// Check if the process exited successfully
    fn is_success(&self) -> bool {
        self.as_ref()
            .map(|output| output.is_success())
            .unwrap_or(false)
    }

    /// Get the exit code of the process, if it exited normally
    fn exit_code(&self) -> Option<i32> {
        self.as_ref().ok().and_then(|output| output.exit_code())
    }

    /// Get the signal that terminated the process, if it was terminated by a signal
    fn end_signal(&self) -> Option<i32> {
        self.as_ref().ok().and_then(|output| output.end_signal())
    }

    /// When available, get stderr, otherwise get stdout
    fn error_output(&self) -> String {
        self.as_ref()
            .map(|output| output.error_output())
            .unwrap_or("".into())
    }

    /// Get stdout
    fn output(&self) -> String {
        self.as_ref()
            .map(|output| output.output())
            .unwrap_or("".into())
    }

    /// Check if the process exited successfully, otherwise produce an error
    fn check(&self) -> Result<(), Error> {
        match self {
            Ok(output) => output.check(),
            Err(e) => bail!("Failed to execute {}: {}", self.process_type(), e),
        }
    }

    /// Check if the process exited successfully and return the output, otherwise produce an error with the output
    fn check_output(&self) -> Result<String, Error> {
        match self {
            Ok(output) => output.check_output(),
            Err(e) => bail!("Failed to execute {}: {}", self.process_type(), e),
        }
    }

    /// Produce a string explaining the exit status of the process
    fn explain_exit(&self) -> String {
        match self {
            Ok(output) => output.explain_exit(),
            Err(e) => format!("Failed to execute {}: {}", self.process_type(), e),
        }
    }
}

pub trait RunAndCheck: Sealed {
    fn run_and_check(&mut self) -> Result<(), Error>;
    fn output_and_check(&mut self) -> Result<String, Error>;
    fn raw_output_and_check(&mut self) -> Result<Output, Error>;
    fn render_command(&self) -> String;
}

impl Sealed for Command {}

impl RunAndCheck for Command {
    fn run_and_check(&mut self) -> Result<(), Error> {
        let rendered_command = self.render_command();
        trace!("Executing '{rendered_command}'");
        let result = self.output();
        trace!(
            "Executed '{rendered_command}': {}. Report:\n{}",
            result.explain_exit(),
            result.output_report(),
        );
        result
            .check()
            .with_context(|| format!("Error when running: {}", self.render_command()))
    }

    fn output_and_check(&mut self) -> Result<String, Error> {
        let rendered_command = self.render_command();
        trace!("Executing '{rendered_command}'");
        let result = self.output();
        trace!(
            "Executed '{rendered_command}': {}. Report:\n{}",
            result.explain_exit(),
            result.output_report(),
        );
        result
            .check_output()
            .with_context(|| format!("Error when running: {}", self.render_command()))
    }

    fn raw_output_and_check(&mut self) -> Result<Output, Error> {
        let rendered_command = self.render_command();
        trace!("Executing '{rendered_command}'");
        // Run the process and store the result.
        let result = self.output();
        trace!(
            "Executed '{rendered_command}': {}. Report:\n{}",
            result.explain_exit(),
            result.output_report(),
        );

        // Check the result to be sure it's Ok(output) and that the subprocess
        // exited successfully.
        result
            .check()
            .with_context(|| format!("Error when running: {}", self.render_command()))?;

        // We already checked the result, so we know it's an Ok(output) and
        // output.is_success() == true. We need to return the output, so we
        // unwrap() it out of the result.
        Ok(result.unwrap())
    }

    fn render_command(&self) -> String {
        if self.get_args().count() == 0 {
            self.get_program().to_string_lossy().into()
        } else {
            format!(
                "{} {}",
                self.get_program().to_string_lossy(),
                self.get_args()
                    .map(|arg| arg.to_string_lossy())
                    .map(|arg| if arg.contains(' ') {
                        format!("'{}'", arg)
                    } else {
                        arg.into()
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use std::process::Command;

    #[test]
    fn test_output_checker() {
        let output = Command::new("echo").arg("something").output().unwrap();

        assert!(output.is_success());
        assert_eq!(output.exit_code(), Some(0));
        assert_eq!(output.end_signal(), None);
        assert_eq!(output.error_output(), "");
        assert_eq!(output.output(), "something\n");
        assert_eq!(output.explain_exit(), "process exited with status: 0");
        assert!(matches!(output.check(), Ok(())));
        assert!(matches!(output.check_output(), Ok(s) if s == "something\n"));

        let output = Command::new("false").arg("something").output().unwrap();

        assert!(!output.is_success());
        assert_eq!(output.exit_code(), Some(1));
        assert_eq!(output.end_signal(), None);
        assert_eq!(output.error_output(), "");
        assert_eq!(output.output(), "");
        assert_eq!(output.explain_exit(), "process exited with status: 1");

        output.check().unwrap_err();

        // Check trait on io::Result<Output>
        let result = Command::new("/doesnotexist_1234").arg("something").output();

        assert!(result.is_err(), "Expected error, got {:?}", result);

        assert!(!result.is_success(), "Expected failure, got {:?}", result);

        assert_eq!(
            result.exit_code(),
            None,
            "Expected exit code None, got {:?}",
            result
        );

        assert_eq!(
            result.end_signal(),
            None,
            "Expected end signal None, got {:?}",
            result
        );

        assert!(result.check().is_err(), "Expected error, got {:?}", result);
        assert!(
            result.check_output().is_err(),
            "Expected error, got {:?}",
            result
        );
        assert!(result.explain_exit().contains("Failed to execute process:"));

        // Check exit codes
        let result = Command::new("bash")
            .arg("-c")
            .arg("exit 123")
            .output()
            .expect("Failed to start bash");

        assert!(!result.is_success(), "Expected failure, got {:?}", result);

        assert_eq!(
            result.exit_code(),
            Some(123),
            "Expected exit code 123, got {:?}",
            result
        );

        assert_eq!(
            result.end_signal(),
            None,
            "Expected end signal None, got {:?}",
            result
        );
    }

    #[test]
    fn test_run_and_check() {
        let mut cmd = Command::new("echo");
        cmd.arg("something");
        assert_eq!(cmd.output_and_check().unwrap(), "something\n");

        // This command doesnt exist
        let mut cmd = Command::new("nonexistent_command_1234");
        cmd.arg("/nonexistent");
        cmd.run_and_check().unwrap_err();

        // This command should fail
        let mut cmd = Command::new("false");
        cmd.arg("something");
        cmd.run_and_check().unwrap_err();

        // This command should fail
        let mut cmd = Command::new("cat");
        cmd.arg("/nonexistent_file_1234");
        cmd.run_and_check().unwrap_err();
    }

    #[test]
    fn test_render_command() {
        let mut cmd = Command::new("echo");
        cmd.arg("something");
        assert_eq!(cmd.render_command(), "echo something");

        let mut cmd = Command::new("echo");
        cmd.arg("something with spaces");
        assert_eq!(cmd.render_command(), "echo 'something with spaces'");

        let mut cmd = Command::new("echo");
        cmd.arg("something");
        cmd.arg("with");
        cmd.arg("multiple");
        cmd.arg("arguments");
        assert_eq!(
            cmd.render_command(),
            "echo something with multiple arguments"
        );
    }

    #[test]
    fn test_raw_output_and_check() {
        let mut cmd = Command::new("echo");
        cmd.arg("something");
        let output = cmd.raw_output_and_check().unwrap();
        assert_eq!(
            output.stdout, b"something\n",
            "Output does not match expected",
        );
        assert!(output.is_success(), "Expected success, got {:?}", output);

        // This command doesnt exist
        let mut cmd = Command::new("nonexistent_command_1234");
        cmd.arg("/nonexistent");
        cmd.raw_output_and_check().unwrap_err();

        // This command should fail
        let mut cmd = Command::new("false");
        cmd.arg("something");
        cmd.raw_output_and_check().unwrap_err();

        // This command should fail
        let mut cmd = Command::new("cat");
        cmd.arg("/nonexistent_file_1234");
        cmd.raw_output_and_check().unwrap_err();
    }
}
