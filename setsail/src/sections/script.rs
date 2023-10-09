use log::debug;
use std::{error::Error, path::PathBuf};

use clap::Parser;

use crate::{
    data::ParsedData, errors::ToResultSetsailError, handlers::SectionHandler, types::KSLine,
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
        format!("ks-script/{}", self.line.get_id())
    }

    pub fn run(&self) -> Result<(), SetsailError> {
        self.run_internal()
            .map_err(|e| SetsailError::new_pre_script_error(self.line.clone(), e.to_string()))
    }

    fn run_internal(&self) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        debug!("Running {} script from {}", self.script_type, self.line);
        osutils::scripts::ScriptRunner::new_interpreter(&self.interpreter, &self.body)?
            .with_logfile(self.log.as_ref())?
            .run_check()?;
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
