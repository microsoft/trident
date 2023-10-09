use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output},
};

use anyhow::{bail, Context, Error};
use tempfile::NamedTempFile;

/// Runs a bash script and checks the exit status
/// Preferred for simple one-off configuration scripts
pub fn run_bash_script(script: &str) -> Result<(), Error> {
    let file = write_script_to_file(std::env::temp_dir(), script)
        .context("Failed to write script to file")?;
    let status = Command::new("/bin/bash")
        .stdin(file)
        .status()
        .context("Failed to execute script")?;

    if !status.success() {
        bail!(
            "{}",
            match status.code() {
                Some(code) => format!("Script exited with status: {code}"),
                None => "Script was terminated by signal".into(),
            }
        )
    }

    Ok(())
}

// These paths are purposefully set to /root instead of /tmp for 2 reasons:
// 1. /root guarantees that we only execute scripts when running as root
//    because we first need to write the script file to /root. This can
//    prevent accidents when running in a development environment.
// 2. For logs, we want to persist them.
const SCRIPT_DIR: &str = "/root/trident-script";
const SCRIPT_LOG_DIR: &str = "/root/trident-script-logs";

/// Output of a script
#[derive(Debug)]
pub struct ScriptResult {
    /// Exist status of the script
    pub status: ExitStatus,

    /// Output of the script
    pub output: ScriptOutput,
}

impl ScriptResult {
    /// Get success
    pub fn success(&self) -> bool {
        self.status.success()
    }

    /// Get exit code
    pub fn code(&self) -> Option<i32> {
        self.status.code()
    }

    /// Check exit code
    pub fn check(&self) -> Result<(), String> {
        if self.success() {
            Ok(())
        } else {
            Err(self.explain_exit())
        }
    }

    /// Exit code explanation
    pub fn explain_exit(&self) -> String {
        match self.code() {
            Some(code) => format!("script exited with status: {code}"),
            None => "script was terminated by signal".into(),
        }
    }

    /// Get stderr
    pub fn stderr(&self) -> &str {
        match self.output {
            ScriptOutput::Combined(ref s) => s,
            ScriptOutput::Separate { ref stderr, .. } => stderr,
        }
    }

    /// Get stdout
    pub fn stdout(&self) -> &str {
        match self.output {
            ScriptOutput::Combined(ref s) => s,
            ScriptOutput::Separate { ref stdout, .. } => stdout,
        }
    }
}

/// Output of a script
#[derive(Debug)]
pub enum ScriptOutput {
    /// Combined stdout and stderr
    Combined(String),
    /// Separate stdout and stderr
    Separate { stdout: String, stderr: String },
}

/// A flexible helper for running scripts
///
/// Use this for more complex scripts that require a specific interpreter and collecting output.
/// For simple scripts, use `run_bash_script`.
pub struct ScriptRunner {
    /// The command to run the script
    command: Command,
    /// The script file. We need to keep this around so that it doesn't get deleted.
    _script: NamedTempFile,
    /// The path to the logfile
    logfile: Option<(File, PathBuf)>,
}

impl ScriptRunner {
    /// Clean script directory
    pub fn clear_script_dir() -> Result<(), Error> {
        let path = PathBuf::from(SCRIPT_DIR);
        // Only clear the directory if it exists as removing a non-existent directory will fail
        if path.exists() && path.is_dir() {
            std::fs::remove_dir_all(SCRIPT_DIR).context("Failed to clear script directory")?;
        }

        Ok(())
    }

    /// Internal builder
    fn new<I: AsRef<Path>>(interpreter: I, script: &str) -> Result<Self, Error> {
        let script = write_script_to_named_file(SCRIPT_DIR, script)?;
        let mut command = Command::new(interpreter.as_ref());
        command.arg(script.path());
        Ok(Self {
            command,
            _script: script,
            logfile: None,
        })
    }

    /// Build a new bash script runner
    pub fn new_bash(script: &str) -> Result<Self, Error> {
        Self::new("/bin/bash", script)
    }

    /// Build a new python script runner
    pub fn new_python3(script: &str) -> Result<Self, Error> {
        Self::new("/usr/bin/python3", script)
    }

    /// Build a new script runner with the given interpreter
    pub fn new_interpreter<I: AsRef<Path>>(interpreter: I, script: &str) -> Result<Self, Error> {
        Self::new(
            which::which(interpreter.as_ref()).context("Failed to find interpreter")?,
            script,
        )
    }

    /// Set the logfile to use for the script
    /// If `logfile_path` is `None`, a random logfile will be created in the script directory.
    /// If `logfile_path` is `Some`, the logfile will be created at the given path.
    /// The logfile will contain both stdout and stderr.
    pub fn with_logfile<S: AsRef<Path>>(mut self, logfile_path: Option<S>) -> Result<Self, Error> {
        // Create a logfile
        let (logfile, logfile_path) = match logfile_path {
            Some(v) => (
                crate::files::create_file(v.as_ref())?,
                v.as_ref().to_path_buf(),
            ),
            None => crate::files::create_random_file(SCRIPT_LOG_DIR)?,
        };

        // Set the command to output to the logfile
        self.command
            .stdout(logfile.try_clone().context("Failed to redirect stdout")?);
        self.command
            .stderr(logfile.try_clone().context("Failed to redirect stderr")?);

        self.logfile = Some((logfile, logfile_path));

        Ok(self)
    }

    /// Run the script
    /// Returns the output of the script. This function will block until the script exits.
    /// This function does NOT check if the script exited successfully. Use `run_check` for that.
    pub fn run(&mut self) -> Result<ScriptResult, Error> {
        let process_output: Output = self.command.output().context("Failed to run script")?;

        let output = match self.logfile {
            Some((ref mut logfile, _)) => {
                logfile
                    .seek(SeekFrom::Start(0))
                    .context("Failed to seek to start of logfile")?;
                let mut output = String::new();
                logfile
                    .read_to_string(&mut output)
                    .context("Failed to read logfile")?;
                ScriptOutput::Combined(output)
            }
            None => ScriptOutput::Separate {
                stdout: String::from_utf8_lossy(&process_output.stdout).into(),
                stderr: String::from_utf8_lossy(&process_output.stderr).into(),
            },
        };

        Ok(ScriptResult {
            status: process_output.status,
            output,
        })
    }

    /// Run the script and check the exit status
    pub fn run_check(&mut self) -> Result<(), Error> {
        let result = self.run()?;

        if let Err(e) = result.check() {
            bail!(
                "{}{}",
                e,
                match self.logfile {
                    Some((_, ref logfile)) => format!(" - See logfile: {}", logfile.display()),
                    None => "".into(),
                }
            )
        }

        Ok(())
    }

    pub fn get_logfile(&self) -> Option<&Path> {
        self.logfile.as_ref().map(|(_, p)| p.as_ref())
    }
}

/// Writes a script to a temporary UNAMED file and returns the File.
fn write_script_to_file<P: AsRef<Path>>(dir: P, script_body: &str) -> Result<File, Error> {
    crate::files::create_dirs(SCRIPT_DIR)?;
    let mut script_file =
        tempfile::tempfile_in(dir).context("Failed to create temporary file for script")?;
    script_file
        .write_all(script_body.as_bytes())
        .context("Failed to write script to temporary file")?;
    script_file
        .seek(SeekFrom::Start(0))
        .context("Failed to seek to start of script file")?;

    Ok(script_file)
}

/// Writes a script to a temporary NAMED file and returns the NamedTempFile.
fn write_script_to_named_file<P: AsRef<Path>>(
    dir: P,
    script_body: &str,
) -> Result<NamedTempFile, Error> {
    crate::files::create_dirs(SCRIPT_DIR)?;
    let mut script_file = tempfile::NamedTempFile::new_in(dir)
        .context("Failed to create temporary file for script")?;
    script_file
        .write_all(script_body.as_bytes())
        .context("Failed to write script to temporary file")?;

    Ok(script_file)
}
