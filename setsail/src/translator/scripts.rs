use std::collections::HashMap;

use trident_api::config::{HostConfiguration, Script, ScriptSource, ServicingTypeSelection};

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
            source: ScriptSource::Content(script.body.clone()),
            environment_variables: HashMap::new(),
            arguments: vec![],
        })
        .collect();
}
