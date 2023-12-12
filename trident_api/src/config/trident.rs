use std::path::PathBuf;

use anyhow::{bail, Error};
use netplan_types::NetworkConfig;
use serde::{Deserialize, Serialize};

use crate::is_default;

use super::host::HostConfiguration;

/// Definition of Trident's full configuration.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LocalConfigFile {
    /// Optional URL to reach out to when networking is up, so Trident
    /// can report its status. This is useful for debugging and monitoring purposes,
    /// say by an orchestrator. Note that separately the updates to the Host Status
    /// can be monitored, once gRPC support is implemented. TODO: document the
    /// interface, for reference in the meantime.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phonehome: Option<String>,

    /// Optional URL to stream logs to. TODO: document the interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logstream: Option<String>,

    /// If present, indicates the path to an existing datastore Trident
    /// should load its state from. This field should not be included when Trident is
    /// running from the provisioning OS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datastore: Option<PathBuf>,

    /// Optional netplan network configuration for the bootstrap OS. If
    /// not specified, the network configuration from Host Configuration
    /// will be used otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_override: Option<NetworkConfig>,

    /// Wait for the provisioning OS network to be up before starting the
    /// clean install process.
    #[serde(default, skip_serializing_if = "is_default")]
    pub wait_for_provisioning_network: bool,

    /// A combination of flags representing allowed operations. This is a
    /// list of operations that Trident is allowed to perform on the host.
    ///
    /// You can pass multiple flags, separated by `|`. Example: `Update | Transition`.
    /// You can pass `''` to disable all operations, which would result in getting
    /// refreshed Host Status, but no operations performed on the host.
    #[serde(default)]
    pub allowed_operations: Operations,

    /// Grpc configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc: Option<GrpcConfiguration>,

    // * * * * * * * * * * * * * * * *
    //   Host configuration sources.
    // * * * * * * * * * * * * * * * *
    // ONLY ONE OF THE FOLLOWING CAN BE PROVIDED
    //
    // This looks like an enum because it was originally an enum, but to achieve
    // the desired schema we had to use serde(flatten) which breaks deny_unknown_fields
    // and causes silent errors some times. So we switched to a struct with all the fields
    // manually "flattened".
    //
    /// Describes the host configuration. This is the configuration that Trident
    /// will apply to the host (same payload as `host-configuration-file`, but
    /// directly embedded in the Trident configuration)
    #[serde(skip_serializing_if = "Option::is_none")]
    host_configuration: Option<Box<HostConfiguration>>,

    /// Path to the host configuration file. This is a YAML file that describes the
    /// host configuration in the Host Configuration format.
    #[serde(skip_serializing_if = "Option::is_none")]
    host_configuration_file: Option<PathBuf>,

    /// Describes the host configuration in the kickstart format. This is the
    /// configuration that Trident will apply to the host (same payload as
    /// `kickstart-file`, but directly embedded in the Trident configuration). WIP,
    /// early preview only.
    #[serde(skip_serializing_if = "Option::is_none")]
    kickstart: Option<String>,

    /// Path to the kickstart file. This is a kickstart file that describes the host
    /// configuration in the kickstart format. WIP, early preview only. TODO:
    /// document what is supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    kickstart_file: Option<PathBuf>,
}

impl LocalConfigFile {
    /// Returns the host configuration source, if any.
    pub fn get_host_configuration_source(&self) -> Result<Option<HostConfigurationSource>, Error> {
        let config_sources = [
            self.host_configuration.is_some(),
            self.host_configuration_file.is_some(),
            self.kickstart.is_some(),
            self.kickstart_file.is_some(),
        ]
        .into_iter()
        .filter(|x| *x)
        .count();

        if config_sources > 1 {
            bail!("Failed to parse Trident configuration: at MOST one of host-configuration, host-configuration-file, kickstart, kickstart-file can be specified");
        }

        Ok(Some(
            if let Some(host_configuration) = &self.host_configuration {
                HostConfigurationSource::Embedded(host_configuration.clone())
            } else if let Some(host_configuration_file) = &self.host_configuration_file {
                HostConfigurationSource::File(host_configuration_file.clone())
            } else if let Some(kickstart) = &self.kickstart {
                HostConfigurationSource::KickstartEmbedded(kickstart.clone())
            } else if let Some(kickstart_file) = &self.kickstart_file {
                HostConfigurationSource::KickstartFile(kickstart_file.clone())
            } else {
                return Ok(None);
            },
        ))
    }

    pub fn with_host_configuration(mut self, host_configuration: HostConfiguration) -> Self {
        self.host_configuration = Some(Box::new(host_configuration));
        self
    }

    pub fn with_host_configuration_source(mut self, src: HostConfigurationSource) -> Self {
        match src {
            HostConfigurationSource::Embedded(host_configuration) => {
                self.host_configuration = Some(host_configuration);
            }
            HostConfigurationSource::File(host_configuration_file) => {
                self.host_configuration_file = Some(host_configuration_file);
            }
            HostConfigurationSource::KickstartEmbedded(kickstart) => {
                self.kickstart = Some(kickstart);
            }
            HostConfigurationSource::KickstartFile(kickstart_file) => {
                self.kickstart_file = Some(kickstart_file);
            }
        }
        self
    }

    pub fn with_datastore(mut self, datastore: PathBuf) -> Self {
        self.datastore = Some(datastore);
        self
    }

    pub fn with_network_override(mut self, network_override: NetworkConfig) -> Self {
        self.network_override = Some(network_override);
        self
    }

    pub fn with_allowed_operations(mut self, allowed_operations: Operations) -> Self {
        self.allowed_operations = allowed_operations;
        self
    }

    pub fn with_grpc(mut self, grpc: Option<GrpcConfiguration>) -> Self {
        self.grpc = grpc;
        self
    }

    pub fn with_phonehome(mut self, phonehome: Option<String>) -> Self {
        self.phonehome = phonehome;
        self
    }

    pub fn with_logstream(mut self, logstream: Option<String>) -> Self {
        self.logstream = logstream;
        self
    }
}

/// HostConfigurationSource is the source of the host configuration.
/// Used internally by Trident.
#[derive(Debug)]
pub enum HostConfigurationSource {
    File(PathBuf),
    Embedded(Box<HostConfiguration>),
    KickstartFile(PathBuf),
    KickstartEmbedded(String),
}

/// GrpcConfiguration is the configuration for the gRPC server.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GrpcConfiguration {
    /// Port for the gRPC server (defaults to 50051 if not set).
    pub listen_port: Option<u16>,
}

bitflags::bitflags! {
    #[derive(Serialize, Deserialize, Debug, Copy, Clone)]
    #[serde(rename_all = "kebab-case", deny_unknown_fields)]
    pub struct Operations: u32 {
        /// Trident will update the host based on the host configuration,
        /// but it will not transition the host to the new configuration. This is useful
        /// if you want to drive additional operations on the host outside of Trident.
        const Update = 0b1;
        /// Trident will transition the host to the new configuration,
        /// which can include rebooting the host. This will only happen if `Update` is
        /// also specified.
        const Transition = 0b10;
    }
}
impl Default for Operations {
    fn default() -> Self {
        Operations::all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    #[test]
    fn test_grpc_and_embedded_host_config() {
        let local_config_yaml = indoc! {r#"
            grpc:
              listen-port: null
            host-configuration:
              management:
                disable: true
        "#};

        let local_config: LocalConfigFile = serde_yaml::from_str(local_config_yaml).unwrap();

        assert!(local_config.grpc.is_some());
        assert!(local_config.host_configuration.is_some());
        assert!(local_config.host_configuration_file.is_none());
        assert!(local_config.kickstart.is_none());
        assert!(local_config.kickstart_file.is_none());
    }

    #[test]
    fn test_host_config_source() {
        let cfg: LocalConfigFile = serde_yaml::from_str(indoc! {r#"
            host-configuration:
              management:
                disable: true
        "#})
        .unwrap();

        assert!(cfg.host_configuration.is_some());
        assert!(cfg.host_configuration_file.is_none());
        assert!(cfg.kickstart.is_none());
        assert!(cfg.kickstart_file.is_none());

        assert!(matches!(
            cfg.get_host_configuration_source().unwrap(),
            Some(HostConfigurationSource::Embedded(_))
        ));

        let cfg: LocalConfigFile = serde_yaml::from_str(indoc! {r#"
            host-configuration-file: /tmp/foo.yaml
        "#})
        .unwrap();

        assert!(cfg.host_configuration.is_none());
        assert!(cfg.host_configuration_file.is_some());
        assert!(cfg.kickstart.is_none());
        assert!(cfg.kickstart_file.is_none());

        assert!(matches!(
            cfg.get_host_configuration_source().unwrap(),
            Some(HostConfigurationSource::File(_))
        ));

        let cfg: LocalConfigFile = serde_yaml::from_str(indoc! {r#"
            kickstart: |
              part / --option --option
        "#})
        .unwrap();

        assert!(cfg.host_configuration.is_none());
        assert!(cfg.host_configuration_file.is_none());
        assert!(cfg.kickstart.is_some());
        assert!(cfg.kickstart_file.is_none());

        assert!(matches!(
            cfg.get_host_configuration_source().unwrap(),
            Some(HostConfigurationSource::KickstartEmbedded(_))
        ));

        let cfg: LocalConfigFile = serde_yaml::from_str(indoc! {r#"
            kickstart-file: /tmp/foo.yaml
        "#})
        .unwrap();

        assert!(cfg.host_configuration.is_none());
        assert!(cfg.host_configuration_file.is_none());
        assert!(cfg.kickstart.is_none());
        assert!(cfg.kickstart_file.is_some());

        assert!(matches!(
            cfg.get_host_configuration_source().unwrap(),
            Some(HostConfigurationSource::KickstartFile(_))
        ));
    }

    #[test]
    fn test_single_host_config_source() {
        let cfg: LocalConfigFile = serde_yaml::from_str(indoc! {r#"
            host-configuration:
              management:
                disable: true
            host-configuration-file: /tmp/foo.yaml
        "#})
        .unwrap();

        // We expect to parse both
        assert!(cfg.host_configuration.is_some());
        assert!(cfg.host_configuration_file.is_some());

        // But it should err when we try to get it
        cfg.get_host_configuration_source().unwrap_err();
    }
}
