use std::{fs, path::Path};

use log::{debug, info};

use trident_api::{
    config::HostConfiguration,
    error::{InternalError, InvalidInputError, ReportError, TridentError, TridentResultExt},
};

pub(crate) fn parse_host_config(
    contents: &str,
    path: impl AsRef<Path>,
) -> Result<HostConfiguration, TridentError> {
    let parsed =
        serde_yaml::from_str(contents).structured(InvalidInputError::ParseHostConfigurationFile {
            path: path.as_ref().display().to_string(),
        });

    if parsed.is_err() {
        match serde_yaml::from_str::<serde_yaml::Value>(contents) {
            Ok(value) => {
                // Detect a few common issues with the host configuration
                if value.get("hostConfiguration").is_some() {
                    return Err(TridentError::new(InvalidInputError::OldStyleConfiguration));
                } else if value.get("allowedOperations").is_some() {
                    return Err(TridentError::new(
                        InvalidInputError::AllowedOperationsInHostConfiguration,
                    ));
                }
            }
            Err(_) => return parsed.message("Host Configuration is not valid YAML"),
        }
    }
    parsed
}

pub fn validate_host_config_file(path: impl AsRef<Path>) -> Result<(), TridentError> {
    info!(
        "Validating Host Configuration file: {}",
        path.as_ref().display()
    );

    let contents =
        fs::read_to_string(path.as_ref()).structured(InvalidInputError::ReadInputFile {
            path: path.as_ref().display().to_string(),
        })?;

    let parsed = parse_host_config(&contents, path.as_ref())
        .message("Failed to parse Host Configuration")?;

    validate_host_config(parsed)
}

fn validate_host_config(hc: HostConfiguration) -> Result<(), TridentError> {
    hc.validate()
        .map_err(|e| TridentError::new(InvalidInputError::from(e)))
        .message("Host Configuration is invalid")?;

    info!("Host Configuration is valid");
    debug!(
        "Parsed contents:\n{}",
        serde_yaml::to_string(&hc).structured(InternalError::SerializeHostStatus)?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    #[test]
    fn test_validate_embedded_host_configuration() {
        let func_test_trident_config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/functional_tests/trident-setup.yaml");
        validate_host_config_file(func_test_trident_config)
            .expect("Failed to validate functional test Host Configuration");
    }
}
