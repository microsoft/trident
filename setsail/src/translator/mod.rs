use log::debug;

use trident_api::config::HostConfiguration;

use crate::data::ParsedData;

use super::errors::SetsailError;

mod network;
mod scripts;

mod misc;

pub fn translate(input: ParsedData) -> Result<HostConfiguration, Vec<SetsailError>> {
    debug!("Translating: {:#?}", input);
    let mut hc = HostConfiguration::default();
    let mut errors: Vec<SetsailError> = Vec::new();

    // Translation functions
    scripts::translate(&input, &mut hc);
    network::translate(&input, &mut hc, &mut errors);

    if errors.is_empty() {
        Ok(hc)
    } else {
        Err(errors)
    }
}
