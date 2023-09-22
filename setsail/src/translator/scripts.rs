use trident_api::config::{HostConfiguration, Script};

use crate::{data::ParsedData, sections::ScriptType};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration) {
    hc.post_install_scripts = input
        .scripts
        .iter()
        .filter(|s| matches!(s.script_type, ScriptType::Post))
        .map(|script| Script {
            interpreter: Some(script.interpreter.clone()),
            log_file_path: script.log.clone(),
            content: script.body.clone(),
        })
        .collect();
}
