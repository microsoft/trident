use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use sysdefs::tpm2::Pcr;

use crate::{
    config::HostConfigurationStaticValidationError, constants::DEV_MAPPER_PATH, BlockDeviceId,
};

#[cfg(feature = "schemars")]
use crate::schema_helpers::block_device_id_schema;

/// Configure encrypted volumes of underlying disk partitions or software RAID arrays.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Encryption {
    /// A URL to read the recovery key from.
    ///
    /// This parameter allows specifying a local file path to a recovery
    /// key file via a `file://` URL scheme. The recovery key file serves
    /// as an essential fallback to recover data should TPM 2.0 automatic
    /// decryption fail. If not specified, only the TPM 2.0 device will be
    /// enrolled.
    ///
    /// The URL must be non-empty if provided. Other URL schemes are not
    /// supported at this time.
    ///
    /// # Recommended Configuration
    ///
    /// It is strongly advised to configure a recovery key file, as it
    /// plays a pivotal role in data recovery.
    ///
    /// # File Format Expectations
    ///
    /// The recovery key file must be a binary file without any encoding.
    /// This direct format ensures compatibility with cryptsetup and
    /// systemd APIs. Be mindful that all file content, including any
    /// potential whitespace or newline characters, is considered part of
    /// the recovery key.
    ///
    /// # Security Considerations
    ///
    /// Ensuring the recovery key's confidentiality and integrity is
    /// paramount. Employ secure storage and rigorous access control
    /// measures. Specifically:
    ///
    /// - The file containing the key should only be accessible by the
    ///   root user and have `0400` permissions set.
    ///
    /// - The recovery key should be a minimum of 32 bytes long and should
    ///   be generated with enough high entropy to defend against brute
    ///   force or cryptographic attacks targeting on-disk hash values.
    ///
    /// # Generating a Recovery Key
    ///
    /// One way to create a recovery key file on Linux systems is using
    /// the `dd` utility:
    ///
    /// > Note: The following example is for illustration purposes only.
    /// > Be sure to generate recovery keys with diligence and attention
    /// > to security principles. Please adjust the following example
    /// > according to your own security policies and operational
    /// > environment to fit your specific security requirements and
    /// > constraints.
    ///
    /// ```sh
    /// touch ./recovery.key
    /// chmod 0400 ./recovery.key
    /// dd if=/dev/random of=./recovery.key bs=1 count=256
    /// ```
    ///
    /// This command generates 256 bytes of random data for the recovery
    /// key, sourcing entropy from `/dev/random`. Be aware, in
    /// environments with limited entropy sources, such as certain
    /// embedded systems, `/dev/random` may not provide sufficient data
    /// promptly. Alternative entropy sources or methods may be required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_key_url: Option<Url>,

    /// The list of LUKS2-encrypted volumes to create.
    ///
    /// This parameter is required and must not be empty. Each item is an
    /// object that will contain the configuration for a given partition
    /// or RAID array.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<EncryptedVolume>,

    /// List of PCRs in TPM 2.0 device to seal to. Each PCR may be specified either as a digit or a
    /// string representation. At least one PCR must be specified, and any combination of the
    /// following PCRs may be used:
    /// - 4, or `boot-loader-code`
    /// - 7, or `secure-boot-policy`
    /// - 11, or `kernel-boot`.
    ///
    /// Other PCRs are currently not supported in the encryption logic.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pcrs: Vec<Pcr>,
}

/// A LUKS2-encrypted volume configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct EncryptedVolume {
    /// The id of the LUKS-encrypted volumes to create.
    ///
    /// This parameter is required. It must be non-empty and unique among
    /// the ids of all block devices in the host configuration. This
    /// includes the ids of all disk partitions, encrypted volumes,
    /// software RAID arrays, and A/B volume pairs.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The name of the device to create under `/dev/mapper` when opening
    /// the volume.
    ///
    /// This parameter is required. It must be a valid file name and
    /// unique among the list of encrypted volumes.
    pub device_name: String,

    /// The id of the disk partition or software RAID array to encrypt.
    ///
    /// This parameter is required. It must be unique among the list of
    /// encrypted volumes.
    ///
    /// If it refers to a disk partition, it must be of a supported type.
    /// Supported types are all but `root` and `efi`.
    ///
    /// If it refers to a software RAID array, the first disk partition of
    /// the software RAID array must be of a supported type.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub device_id: BlockDeviceId,
}

impl Encryption {
    /// Validate the encryption storage configuration.
    ///
    /// This function will validate the encryption configuration and
    /// return an error if the configuration is invalid.
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Encryption recovery key URLs must start with file://
        if let Some(recovery_key_url) = &self.recovery_key_url {
            if recovery_key_url.scheme() != "file" {
                return Err(
                    HostConfigurationStaticValidationError::InvalidEncryptionRecoveryKeyUrlScheme {
                        url: recovery_key_url.to_string(),
                        scheme: recovery_key_url.scheme().to_string(),
                    },
                );
            }
        }

        // The list of PCRs must include at least one PCR, and only currently supported PCRs.
        if self.pcrs.is_empty() {
            return Err(HostConfigurationStaticValidationError::InvalidEncryptionPcrsEmpty);
        }

        let supported_pcrs = [Pcr::Pcr4, Pcr::Pcr7, Pcr::Pcr11];
        let unsupported_pcrs: Vec<Pcr> = self
            .pcrs
            .iter()
            .cloned()
            .filter(|pcr| !supported_pcrs.contains(pcr))
            .collect();
        if !unsupported_pcrs.is_empty() {
            return Err(
                HostConfigurationStaticValidationError::InvalidEncryptionPcrsUnsupported {
                    pcrs: unsupported_pcrs,
                },
            );
        }

        Ok(())
    }
}

impl EncryptedVolume {
    pub fn device_path(&self) -> PathBuf {
        Path::new(DEV_MAPPER_PATH).join(&self.device_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_encryption() {
        let mut config = Encryption {
            pcrs: vec![Pcr::Pcr7],
            ..Default::default()
        };
        config.validate().unwrap();

        config.recovery_key_url = Some(Url::parse("file:///path/to/recovery.key").unwrap());
        config.validate().unwrap();
    }

    #[test]
    fn test_validate_encryption_fail_invalid_recovery_key_url() {
        let config = Encryption {
            pcrs: vec![Pcr::Pcr7],
            recovery_key_url: Some(
                Url::parse("http://example.com/invalid-recovery-key-http").unwrap(),
            ),
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            HostConfigurationStaticValidationError::InvalidEncryptionRecoveryKeyUrlScheme {
                url: "http://example.com/invalid-recovery-key-http".to_string(),
                scheme: "http".to_string(),
            }
        );
    }

    #[test]
    fn test_validate_encryption_fail_invalid_pcrs_empty() {
        let config = Encryption {
            pcrs: vec![],
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            HostConfigurationStaticValidationError::InvalidEncryptionPcrsEmpty
        );
    }

    #[test]
    fn test_validate_encryption_fail_invalid_pcrs_unsupported() {
        let config = Encryption {
            pcrs: vec![Pcr::Pcr0],
            ..Default::default()
        };
        assert_eq!(
            config.validate().unwrap_err(),
            HostConfigurationStaticValidationError::InvalidEncryptionPcrsUnsupported {
                pcrs: vec![Pcr::Pcr0],
            }
        );
    }
}
