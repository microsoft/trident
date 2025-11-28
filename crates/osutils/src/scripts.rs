use std::{
    collections::HashMap,
    ffi::OsStr,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Error};
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
    pub output: String,
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

    /// Get the output of the script
    ///
    /// Since output is merged, return both stderr and stdout output
    fn output(&self) -> String {
        self.output.clone()
    }
}

/// A flexible helper for running scripts
///
/// Use this for more complex scripts that require a specific interpreter and/or collecting output.
/// For simple scripts, use `run_bash_script`.
pub struct ScriptRunner<'a> {
    /// Interpreter to use
    pub interpreter: PathBuf,
    /// The script file. We need to keep this around so that it doesn't get deleted.
    pub script: &'a [u8],
    /// Environment variables to set for the script
    pub env_vars: HashMap<&'a OsStr, &'a OsStr>,
    /// Arguments to pass to the script
    pub args: Vec<&'a OsStr>,
}

impl<'a> ScriptRunner<'a> {
    /// Internal builder
    fn new<I: AsRef<Path>>(interpreter: I, script: &'a [u8]) -> Self {
        Self {
            interpreter: interpreter.as_ref().to_path_buf(),
            script,
            env_vars: HashMap::new(),
            args: Vec::new(),
        }
    }

    /// Build a new bash script runner
    pub fn new_bash(script: &'a [u8]) -> Self {
        Self::new("/bin/bash", script)
    }

    /// Build a new python script runner
    pub fn new_python3(script: &'a [u8]) -> Self {
        Self::new("/usr/bin/python3", script)
    }

    /// Build a new script runner with the given interpreter
    pub fn new_interpreter<I: AsRef<Path>>(interpreter: I, script: &'a [u8]) -> Self {
        Self::new(interpreter, script)
    }

    pub fn with_args(mut self, args: Vec<&'a OsStr>) -> Self {
        self.args = args;
        self
    }

    pub fn with_env_vars(mut self, env_vars: HashMap<&'a OsStr, &'a OsStr>) -> Self {
        self.env_vars = env_vars;
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
            write_script_to_named_file(self.script).context("Failed to write script to file")?;

        // Create command and set up the script file
        let mut cmd = Command::new(&self.interpreter);
        cmd.arg(script_file.path());

        // Set arguments
        cmd.args(&self.args);

        // Set environment variables
        cmd.envs(&self.env_vars);

        // Create logfile
        let mut file = tempfile::tempfile().context("Failed to create log file")?;

        // Set the command to output to the logfile
        cmd.stdout(file.try_clone().context("Failed to redirect stdout")?);
        cmd.stderr(file.try_clone().context("Failed to redirect stderr")?);

        let cmd_output = cmd.output().context("Failed to start script")?;

        file.flush().context("Failed to flush logfile")?;
        file.seek(SeekFrom::Start(0))
            .context("Failed to seek to start of logfile")?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .context("Failed to read logfile")?;
        let output = String::from_utf8_lossy(&buf).into();

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

    /// Runs the script and checks the exit status.
    pub fn run_check(&mut self) -> Result<(), Error> {
        self.run()
            .context("Failed to run script")?
            .check()
            .context("Script exited with an error")
    }

    /// Runs the script, checks the exit status, and returns the output.
    pub fn output_check(&mut self) -> Result<ScriptResult, Error> {
        let result = self.run().context("Failed to run script")?;
        result.check().context("Script exited with an error")?;
        Ok(result)
    }
}

/// Writes a script to a temporary UNNAMED file and returns the File.
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
fn write_script_to_named_file(script_body: &[u8]) -> Result<NamedTempFile, Error> {
    let mut script_file =
        tempfile::NamedTempFile::new().context("Failed to create temporary file for script")?;
    script_file
        .write_all(script_body)
        .context("Failed to write script to temporary file")?;

    Ok(script_file)
}
