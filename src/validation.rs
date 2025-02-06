use std::path::Path;

use log::{debug, info};

use trident_api::{
    config::HostConfiguration,
    error::{InternalError, InvalidInputError, ReportError, TridentError, TridentResultExt},
};

#[cfg(feature = "setsail")]
#[allow(unused)]
pub fn validate_setsail(conents: impl AsRef<str>) -> Result<(), anyhow::Error> {
    use anyhow::bail;
    use log::error;
    use setsail::KsTranslator;

    info!("Validating embedded kickstart.");
    let translator = KsTranslator::new().include_fail_is_error(false);
    match translator.translate(setsail::load_kickstart_string(conents.as_ref())) {
        Ok(hc) => {
            info!("Kickstart is valid.");
            println!("{}", serde_yaml::to_string(&hc)?);
        }
        Err(e) => {
            error!(
                "Failed to translate kickstart:\n{}",
                serde_json::to_string_pretty(&e.0)?
            );
            bail!("Failed to translate kickstart");
        }
    };

    Ok(())
}

#[cfg(feature = "setsail")]
#[allow(unused)]
pub fn validate_setsail_file(path: impl AsRef<Path>) -> Result<(), anyhow::Error> {
    use anyhow::{bail, Context};
    use log::error;
    use setsail::KsTranslator;

    info!("Validating kickstart file: {}", path.as_ref().display());
    let translator = KsTranslator::new().include_fail_is_error(false);
    match translator.translate(
        setsail::load_kickstart_file(path.as_ref())
            .context(format!("Failed to read {}", path.as_ref().display()))?,
    ) {
        Ok(hc) => {
            info!("Kickstart is valid.");
            println!("{}", serde_yaml::to_string(&hc)?);
        }
        Err(e) => {
            error!(
                "Failed to translate kickstart:\n{}",
                serde_json::to_string_pretty(&e.0)?
            );
            bail!("Failed to translate kickstart");
        }
    };

    Ok(())
}

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
        std::fs::read_to_string(path.as_ref()).structured(InvalidInputError::ReadInputFile {
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
        let func_test_trident_config =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("functional_tests/trident-setup.yaml");
        validate_host_config_file(func_test_trident_config)
            .expect("Failed to validate functional test Host Configuration");
    }
}
