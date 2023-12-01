use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Error};
use log::debug;
use tempfile::NamedTempFile;

use crate::{crate_private::Sealed, exe::OutputChecker};

/// Runs a bash script and checks the exit status
/// Preferred for simple one-off configuration scripts
pub fn run_bash_script(script: &str) -> Result<(), Error> {
    let file = write_script_to_file(script).context("Failed to write script to file")?;
    Command::new("/bin/bash")
        .stdin(file)
        .status()
        .context("Failed to execute script")?
        .check()
}

/// Output of a script
#[derive(Debug)]
pub struct ScriptResult {
    /// Exist status of the script
    pub status: ExitStatus,

    /// Output of the script
    pub output: ScriptOutput,
}

impl Sealed for ScriptResult {}

impl OutputChecker for ScriptResult {
    fn is_success(&self) -> bool {
        self.status.success()
    }

    fn exit_code(&self) -> Option<i32> {
        self.status.code()
    }

    fn end_signal(&self) -> Option<i32> {
        self.status.end_signal()
    }

    fn process_type(&self) -> &'static str {
        // Report this as a "script" in errors
        "script"
    }

    /// Get the error output of the script
    ///
    /// - When output is merged, return empty string, since there is no separate stderr.
    /// - When output is separate, return stderr.
    /// - When output is none, return an empty string.
    fn error_output(&self) -> String {
        match self.output {
            ScriptOutput::Combined(_) => "".into(),
            ScriptOutput::Separate { ref stderr, .. } => stderr.clone(),
            ScriptOutput::None => "".into(),
        }
    }

    /// Get the output of the script
    ///
    /// - When output is merged, return the merged output
    /// - When output is separate, return stdout
    /// - When output is none, return an empty string
    fn output(&self) -> String {
        match self.output {
            ScriptOutput::Combined(ref s) => s.clone(),
            ScriptOutput::Separate { ref stdout, .. } => stdout.clone(),
            ScriptOutput::None => "".into(),
        }
    }
}

/// Output of a script
#[derive(Debug)]
pub enum ScriptOutput {
    /// No output was captured
    None,
    /// Combined stdout and stderr
    Combined(String),
    /// Separate stdout and stderr
    Separate { stdout: String, stderr: String },
}

/// A flexible helper for running scripts
///
/// Use this for more complex scripts that require a specific interpreter and/or collecting output.
/// For simple scripts, use `run_bash_script`.
pub struct ScriptRunner {
    /// Interpreter to use
    interpreter: PathBuf,
    /// The script file. We need to keep this around so that it doesn't get deleted.
    script: String,
    /// The path to the logfile
    logfile: Option<PathBuf>,
    /// Merge stderr into stdout
    merge_stderr: bool,
}

impl ScriptRunner {
    /// Internal builder
    fn new<I: AsRef<Path>>(interpreter: I, script: &str) -> Self {
        Self {
            interpreter: interpreter.as_ref().to_path_buf(),
            script: script.to_string(),
            logfile: None,
            merge_stderr: false,
        }
    }

    /// Build a new bash script runner
    pub fn new_bash(script: &str) -> Self {
        Self::new("/bin/bash", script)
    }

    /// Build a new python script runner
    pub fn new_python3(script: &str) -> Self {
        Self::new("/usr/bin/python3", script)
    }

    /// Build a new script runner with the given interpreter
    pub fn new_interpreter<I: AsRef<Path>>(interpreter: I, script: &str) -> Self {
        Self::new(interpreter, script)
    }

    /// Set the logfile to use for the script
    /// If `logfile_path` is `None`, it's the same as not setting a logfile.
    /// If `logfile_path` is `Some`, the logfile will be created at the given path.
    /// The logfile will contain both stdout and stderr.
    pub fn with_logfile<S: AsRef<Path>>(mut self, logfile_path: Option<S>) -> Self {
        self.logfile = logfile_path.map(|p| p.as_ref().to_path_buf());
        self
    }

    /// Merge stderr into stdout
    pub fn merge_stderr(mut self) -> Self {
        self.merge_stderr = true;
        self
    }

    fn run_internal(&mut self) -> Result<ScriptResult, Error> {
        // TODO: consider changing internal implementation to use duct crate

        // Find interpreter's full path
        self.interpreter = which::which(&self.interpreter).context(format!(
            "Failed to find interpreter: {}",
            self.interpreter.display()
        ))?;

        // Write the script to a file
        let script_file =
            write_script_to_named_file(&self.script).context("Failed to write script to file")?;

        // Create command and set up the script file
        let mut cmd = Command::new(&self.interpreter);
        cmd.arg(script_file.path());

        let to_file = self.merge_stderr || self.logfile.is_some();

        // Create logfile
        let mut file = tempfile::tempfile().context("Failed to create log file")?;

        if to_file {
            // Set the command to output to the logfile
            debug!("Writing logfile to temp file");
            cmd.stdout(file.try_clone().context("Failed to redirect stdout")?);
            cmd.stderr(file.try_clone().context("Failed to redirect stderr")?);
        }

        let cmd_output = cmd.output().context("Failed to start script")?;

        let output = if to_file {
            file.flush().context("Failed to flush logfile")?;
            file.seek(SeekFrom::Start(0))
                .context("Failed to seek to start of logfile")?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)
                .context("Failed to read logfile")?;

            // Write the logfile to the given path when requested
            if let Some(ref logfile) = self.logfile {
                log::debug!("Writing logfile to {}", logfile.display());
                crate::files::create_file(logfile)
                    .context("Failed to create persistent logfile")?
                    .write_all(&buf)
                    .context("Failed to write persistent logfile")?;
            }

            ScriptOutput::Combined(String::from_utf8_lossy(&buf).into())
        } else {
            ScriptOutput::Separate {
                stdout: String::from_utf8_lossy(&cmd_output.stdout).into(),
                stderr: String::from_utf8_lossy(&cmd_output.stderr).into(),
            }
        };

        Ok(ScriptResult {
            status: cmd_output.status,
            output,
        })
    }

    /// Run the script and get the output
    /// Returns the output of the script. This function will block until the script exits.
    /// This function does NOT check if the script exited successfully. Use `run_check` for that.
    pub fn run(&mut self) -> Result<ScriptResult, Error> {
        self.run_internal()
    }

    /// Run the script and check the exit status
    pub fn run_check(&mut self) -> Result<(), Error> {
        self.run()
            .context("Failed to run script")?
            .check()
            .context("Script exited with an error")
    }
}

/// Writes a script to a temporary UNAMED file and returns the File.
fn write_script_to_file(script_body: &str) -> Result<File, Error> {
    let mut script_file =
        tempfile::tempfile().context("Failed to create temporary file for script")?;
    script_file
        .write_all(script_body.as_bytes())
        .context("Failed to write script to temporary file")?;
    script_file
        .seek(SeekFrom::Start(0))
        .context("Failed to seek to start of script file")?;

    Ok(script_file)
}

/// Writes a script to a temporary NAMED file and returns the NamedTempFile.
fn write_script_to_named_file(script_body: &str) -> Result<NamedTempFile, Error> {
    let mut script_file =
        tempfile::NamedTempFile::new().context("Failed to create temporary file for script")?;
    script_file
        .write_all(script_body.as_bytes())
        .context("Failed to write script to temporary file")?;

    Ok(script_file)
}
