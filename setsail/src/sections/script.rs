use std::path::PathBuf;

use anyhow::Context;
use clap::{Command, CommandFactory, Parser};
use log::{debug, error};

use osutils::scripts::ScriptRunner;

use crate::{data::ParsedData, errors::ToResultSetsailError, types::KSLine, SetsailError};

use super::SectionHandler;

#[derive(Parser, Debug, Clone)]
pub struct Script {
    #[clap(skip)]
    pub line: KSLine,

    #[clap(skip)]
    pub script_type: ScriptType,

    #[clap(skip)]
    pub body: String,

    /// If the script fails the installation will be aborted.
    ///
    ///
    #[arg(long)]
    pub erroronfail: bool,

    /// The interpreter to use for the script.
    ///
    /// lol
    #[arg(long, default_value = "/bin/sh")]
    pub interpreter: PathBuf,

    /// The path to a file to log the script's output to.
    ///
    ///
    #[arg(long, alias = "logfile")]
    pub log: Option<PathBuf>,
    // Disabled for now. Trident does not support escaping the
    // chroot when running post-install scripts.
    // /// If set, the script will be run outside of the chroot.
    // /// ONLY VALID IN %post SCRIPTS.
    // #[arg(long)]
    // pub nochroot: bool,
}

impl Script {
    pub fn name(&self) -> String {
        format!("ks-script/{}", self.line.get_id())
    }

    pub fn run(&self) -> Result<(), SetsailError> {
        ScriptRunner::new_interpreter(&self.interpreter, self.body.as_bytes())
            .with_logfile(self.log.as_ref())
            .run_check()
            .context(format!("{} script failed", self.script_type))
            .map_err(|e| {
                error!("{:?}", e);
                SetsailError::new_pre_script_error(self.line.clone(), e.to_string())
            })
    }
}

#[derive(Debug, Default, Clone)]
pub enum ScriptType {
    #[default]
    Unknown,
    Pre,
    PreInstall,
    Post,
}

impl ScriptType {
    pub fn name(&self) -> &'static str {
        match self {
            ScriptType::Unknown => "unknown",
            ScriptType::Pre => "%pre",
            ScriptType::PreInstall => "%pre-install",
            ScriptType::Post => "%post",
        }
    }
}

impl std::fmt::Display for ScriptType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[derive(Debug)]
pub struct ScriptHandler {
    script_type: ScriptType,
}

impl ScriptHandler {
    pub fn new(script_type: ScriptType) -> Self {
        Self { script_type }
    }
}

impl SectionHandler for ScriptHandler {
    fn opener(&self) -> &'static str {
        self.script_type.name()
    }

    fn handle(
        &self,
        data: &mut ParsedData,
        line: KSLine,
        tokens: Vec<String>,
        body: Vec<String>,
    ) -> Result<(), SetsailError> {
        debug!(
            "Script Handler of type {} invoked for {} ({} lines)",
            self.script_type,
            line,
            body.len()
        );
        let mut script = Script::try_parse_from(tokens).to_result_parser_error(&line)?;

        // Disabled, see note above
        // nochoroot option is only valid for %post scripts
        // if !matches!(self.script_type, ScriptType::Post) && script.nochroot {
        //     return Err(SetsailError::new_syntax(
        //         line,
        //         "nochroot option is only valid for %post scripts".into(),
        //     ));
        // }

        // Finish script object
        script.line = line;
        script.script_type = self.script_type.clone();
        script.body = body.join("\n");

        // Save the script to parsed data
        data.scripts.push(script);

        Ok(())
    }

    fn name(&self) -> String {
        format!("{} script", self.script_type.name())
    }

    fn get_clap_command(&self) -> Option<Command> {
        let cmd = Script::command();
        Some(match self.script_type {
            ScriptType::Pre => cmd.about(
                "A script to run before the installer start and before kickstart parsing begins.",
            ),
            ScriptType::PreInstall => {
                cmd.about("A script to run right before the installation begins.")
            }
            ScriptType::Post => cmd.about("A script to run after the installation is complete."),
            _ => panic!("Unknown script type"),
        })
    }
}
