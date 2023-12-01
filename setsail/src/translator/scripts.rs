use std::collections::HashMap;

use trident_api::config::{HostConfiguration, Script, ServicingType};

use crate::{data::ParsedData, sections::script::ScriptType};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration) {
    hc.scripts.post_configure = input
        .scripts
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.script_type, ScriptType::Post))
        .map(|(index, script)| Script {
            name: format!("kickstart-script-{}", index),
            servicing_type: vec![ServicingType::CleanInstall],
            interpreter: Some(script.interpreter.clone()),
            content: script.body.clone(),
            log_file_path: script.log.clone(),
            environment_variables: HashMap::new(),
        })
        .collect();
}
