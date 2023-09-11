use log::debug;

use trident_api::config::{HostConfiguration, Script};

use super::{errors::SetsailError, parser::ParsedData};

pub fn translate(input: ParsedData) -> Result<HostConfiguration, Vec<SetsailError>> {
    debug!("Translating: {:#?}", input);
    let mut hc = HostConfiguration::default();
    translate_scripts(&input, &mut hc);

    Ok(hc)
}

fn translate_scripts(input: &ParsedData, hc: &mut HostConfiguration) {
    hc.post_install_scripts = input
        .scripts
        .iter()
        .filter(|s| matches!(s.script_type, super::sections::ScriptType::Post))
        .map(|script| Script {
            interpreter: Some(script.interpreter.clone()),
            log_file_path: script.log.clone(),
            content: script.body.clone(),
        })
        .collect();
}
