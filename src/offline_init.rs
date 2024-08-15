use std::{fs, path::PathBuf};

use log::info;
use trident_api::{
    error::{InitializationError, ReportError, TridentError, TridentResultExt},
    status::HostStatus,
};

use crate::datastore::DataStore;

/// Given a path to a Host Status file, initializes the datastore with the Host Status.
/// This command can be executed offline in a chroot environment as part of MIC image customization.
pub fn execute(hs_path: &PathBuf) -> Result<(), TridentError> {
    let host_status: HostStatus = {
        info!("Reading Host Status from {:?}", hs_path);
        let host_status_yaml = fs::read_to_string(hs_path)
            .structured(InitializationError::LoadHostStatus)
            .message(format!("Failed to read Host Status from {:?}", hs_path))?;
        serde_yaml::from_str(&host_status_yaml)
            .structured(InitializationError::ParseHostStatus)
            .message("Failed to parse Host Status from YAML")?
    };

    let datastore_path = host_status.spec.trident.datastore_path.clone();

    let mut datastore =
        DataStore::open_temporary().message("Failed to open temporary datastore")?;
    datastore
        .with_host_status(|hs| *hs = host_status)
        .message("Failed to set new Host Status")?;

    info!("Persisting Host Status to {:?}", datastore_path);
    datastore.persist(&datastore_path).message(format!(
        "Failed to persist Host Status to datastore at {:?}",
        datastore_path
    ))?;

    Ok(())
}
