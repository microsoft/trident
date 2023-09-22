use log::debug;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use clap::Parser;

use crate::{
    data::ParsedData,
    errors::{ToResultSetsailError, ToSetsailPreScriptError},
    handlers::SectionHandler,
    types::KSLine,
    SetsailError,
};

#[derive(Parser, Debug, Clone)]
pub struct Script {
    #[clap(skip)]
    pub line: KSLine,

    #[clap(skip)]
    pub script_type: ScriptType,

    #[clap(skip)]
    pub body: String,

    #[arg(long)]
    pub erroronfail: bool,

    #[arg(long, default_value = "/bin/sh")]
    pub interpreter: PathBuf,

    #[arg(long)]
    pub log: Option<PathBuf>,

    #[arg(long)]
    pub nochroot: bool,
}

impl Script {
    pub fn name(&self) -> String {
        format!("ks-script-{}", self.line.get_id())
    }

    pub fn run(&self) -> Result<(), SetsailError> {
        debug!("Running {} script from {}", self.script_type, self.line);
        let path = format!("/tmp/{}.sh", self.name());
        let log = self
            .log
            .to_owned()
            .unwrap_or(PathBuf::from(format!("/tmp/{}.log", self.name())));
        let logf = std::fs::File::create(&log).to_pre_script_error(
            &self.line,
            format!("Failed to create log file: {}", log.display()),
        )?;

        std::fs::write(&path, &self.body)
            .to_pre_script_error(&self.line, format!("Failed to write script to {}", &path))?;

        let mut cmd = Command::new(&self.interpreter)
            .arg(path)
            .stdout(Stdio::from(logf.try_clone().to_pre_script_error(
                &self.line,
                "Failed to merge stderr into stdout".into(),
            )?))
            .stderr(Stdio::from(logf.try_clone().to_pre_script_error(
                &self.line,
                "Failed to merge stderr into stdout".into(),
            )?))
            .spawn()
            .to_pre_script_error(&self.line, "Failed to start script".to_string())?;

        debug!("Saved output to {}", log.display());

        let status = cmd
            .wait()
            .to_pre_script_error(&self.line, "Failed to wait for script".into())?;
        if !status.success() {
            return Err(SetsailError::new_pre_script_error(
                self.line.clone(),
                format!("Script exited with status {}", status),
                "".into(),
            ));
        }

        Ok(())
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

impl std::fmt::Display for ScriptType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptType::Unknown => write!(f, "Unknown"),
            ScriptType::Pre => write!(f, "%pre"),
            ScriptType::PreInstall => write!(f, "%pre-install"),
            ScriptType::Post => write!(f, "%post"),
        }
    }
}

pub struct ScriptHandler {
    script_type: ScriptType,
}

impl ScriptHandler {
    pub fn new_boxed(script_type: ScriptType) -> Box<dyn SectionHandler> {
        Box::new(Self { script_type })
    }
}

impl SectionHandler for ScriptHandler {
    fn opener(&self) -> String {
        self.script_type.to_string()
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

        // nochoroot option is only valid for %post scripts
        if !matches!(self.script_type, ScriptType::Post) && script.nochroot {
            return Err(SetsailError::new_syntax(
                line,
                "nochroot option is only valid for %post scripts".into(),
            ));
        }

        // Finish script object
        script.line = line;
        script.script_type = self.script_type.clone();
        script.body = body.join("\n");

        // Save the script to parsed data
        data.scripts.push(script);

        Ok(())
    }
}
