use std::collections::HashMap;

use trident_api::config::{HostConfiguration, Script, ServicingTypeSelection};

use crate::{data::ParsedData, sections::script::ScriptType};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration) {
    hc.scripts.post_configure = input
        .scripts
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.script_type, ScriptType::Post))
        .map(|(index, script)| Script {
            name: format!("kickstart-script-{}", index),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some(script.interpreter.clone()),
            content: Some(script.body.clone()),
            log_file_path: script.log.clone(),
            environment_variables: HashMap::new(),
            path: None,
        })
        .collect();
}
