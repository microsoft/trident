use std::fs;

use anyhow::{Context, Error};
use log::debug;

use netplan_types::NetworkConfig;

use crate::dependencies::Dependency;

/// Path to Trident's netplan config file.
pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/99-trident.yaml";

/// Writes the given network configuration to Trident's netplan config file.
pub fn write(value: &NetworkConfig) -> Result<(), Error> {
    debug!("Writing netplan config to {}", TRIDENT_NETPLAN_FILE);
    fs::write(TRIDENT_NETPLAN_FILE, render_netplan_yaml(value)?)
        .with_context(|| format!("Failed to write netplan config to {TRIDENT_NETPLAN_FILE}"))
}

/// Executes `netplan generate`.
pub fn generate() -> Result<(), Error> {
    debug!("Generating netplan config");
    Dependency::Netplan.cmd().arg("generate").run_and_check()?;
    Ok(())
}

/// Executes `netplan apply`.
pub fn apply() -> Result<(), Error> {
    debug!("Applying netplan config");
    Dependency::Netplan.cmd().arg("apply").run_and_check()?;
    Ok(())
}

/// Renders the given network configuration as a netplan yaml string.
fn render_netplan_yaml(value: &NetworkConfig) -> Result<String, Error> {
    #[derive(serde::Serialize)]
    struct NetplanConfig<'a> {
        network: &'a NetworkConfig,
    }

    serde_yaml::to_string(&NetplanConfig { network: value })
        .context("Failed to render netplan yaml")
}

/// Remove Trident's netplan config file.
pub fn remove() -> Result<(), Error> {
    fs::remove_file(TRIDENT_NETPLAN_FILE)
        .with_context(|| format!("Failed to remove netplan config at {TRIDENT_NETPLAN_FILE}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use netplan_types::{CommonPropertiesAllDevices, EthernetConfig};

    #[test]
    fn test_render_netplan_yaml_basic() {
        let config = NetworkConfig {
            version: 2,
            ethernets: Some(
                [(
                    "eth0".to_string(),
                    EthernetConfig {
                        common_all: Some(CommonPropertiesAllDevices {
                            dhcp4: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )]
                .into(),
            ),
            ..Default::default()
        };

        let expected = indoc::indoc! {r#"
            network:
              version: 2
              ethernets:
                eth0:
                  dhcp4: true
        "#};
        let yaml = render_netplan_yaml(&config).expect("Failed to render yaml");
        assert_eq!(yaml.trim(), expected.trim());
    }

    #[test]
    fn test_render_netplan_yaml_empty() {
        let config = NetworkConfig {
            version: 2,
            ..Default::default()
        };

        let expected = indoc::indoc! {r#"
            network:
              version: 2
        "#};
        let yaml = render_netplan_yaml(&config).expect("Failed to render yaml");
        assert_eq!(yaml.trim(), expected.trim());
    }
}
