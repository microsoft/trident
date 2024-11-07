use std::path::Path;

use anyhow::{Context, Error};
use log::{debug, info};

use trident_api::config::{HostConfiguration, HostConfigurationSource, LocalConfigFile};

#[cfg(feature = "setsail")]
pub fn validate_setsail(conents: impl AsRef<str>) -> Result<(), Error> {
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
pub fn validate_setsail_file(path: impl AsRef<Path>) -> Result<(), Error> {
    use anyhow::bail;
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

pub fn validate_trident_config_file(path: impl AsRef<Path>) -> Result<(), Error> {
    info!(
        "Validating Trident configuration file: {}",
        path.as_ref().display()
    );
    let contents = std::fs::read_to_string(path.as_ref())
        .with_context(|| format!("Failed to read file: {}", path.as_ref().display()))?;

    match serde_yaml::from_str::<LocalConfigFile>(&contents)
        .with_context(|| {
            format!(
                "Failed to parse Trident Configuration YAML file: {}",
                path.as_ref().display()
            )
        })?
        .get_host_configuration_source()
        .context("Invalid Host Configuration source.")?
        .context("Trident Config does not contain Host Configuration.")?
    {
        HostConfigurationSource::Embedded(hc) => {
            info!("Loading embedded Host Configuration.");
            validate_host_config(*hc)?
        }
        HostConfigurationSource::File(path) => {
            info!("Loading Host Configuration from file: {}", path.display());
            validate_host_config_file(path)?
        }
        #[cfg(feature = "setsail")]
        HostConfigurationSource::KickstartEmbedded(contents) => {
            info!("Loading embedded kickstart.");
            validate_setsail(contents)?
        }
        #[cfg(feature = "setsail")]
        HostConfigurationSource::KickstartFile(path) => {
            info!("Loading kickstart from file: {}", path.display());
            validate_setsail_file(path)?
        }
    }

    Ok(())
}

pub fn validate_host_config_file(path: impl AsRef<Path>) -> Result<(), Error> {
    info!(
        "Validating Host Configuration file: {}",
        path.as_ref().display()
    );

    let contents = std::fs::read_to_string(path.as_ref())
        .with_context(|| format!("Failed to read file: {}", path.as_ref().display()))?;

    validate_host_config(
        serde_yaml::from_str::<HostConfiguration>(&contents).with_context(|| {
            format!(
                "Failed to parse Host Configuration YAML file: {}",
                path.as_ref().display()
            )
        })?,
    )
}

fn validate_host_config(hc: HostConfiguration) -> Result<(), Error> {
    hc.validate().context("Host config is invalid")?;

    info!("Host Configuration is valid");
    debug!(
        "Parsed contents:\n{}",
        serde_yaml::to_string(&hc).context("Failed to serialize host configuration file.")?
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
        validate_trident_config_file(func_test_trident_config)
            .expect("Failed to validate functional test Trident Config");
    }
}
