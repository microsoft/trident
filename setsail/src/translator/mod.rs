use log::trace;

use trident_api::config::HostConfiguration;

use crate::data::ParsedData;

use super::errors::SetsailError;

mod network;
mod partitions;
mod scripts;
mod users;

pub fn translate(input: ParsedData) -> Result<HostConfiguration, Vec<SetsailError>> {
    trace!("Translating: {:#?}", input);
    let mut hc = HostConfiguration::default();
    let mut errors: Vec<SetsailError> = Vec::new();

    // TODO(6007): remove this dev option
    hc.trident.self_upgrade = true;

    // Translation functions
    scripts::translate(&input, &mut hc);
    network::translate(&input, &mut hc, &mut errors);
    partitions::translate(&input, &mut hc, &mut errors);
    users::translate(&input, &mut hc, &mut errors);

    if errors.is_empty() {
        Ok(hc)
    } else {
        Err(errors)
    }
}
