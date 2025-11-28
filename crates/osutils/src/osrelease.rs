use std::path::Path;

use anyhow::{Context, Error};
use const_format::formatcp;
use log::trace;
use serde::{Deserialize, Deserializer};

use crate::path;

/// Absolute path to the /etc/os-release file.
pub const OS_RELEASE_PATH: &str = "/etc/os-release";

/// Returns whether the host is running Azure Linux 2.
pub fn is_azl2() -> Result<bool, Error> {
    Ok(OsRelease::read()?.get_distro().is_azl2())
}

/// Returns whether the host is running Azure Linux 3.
pub fn is_azl3() -> Result<bool, Error> {
    Ok(OsRelease::read()?.get_distro().is_azl3())
}

/// Represents the contents of the /etc/os-release file.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct OsRelease {
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub version_id: Option<String>,
    pub pretty_name: Option<String>,
}

impl OsRelease {
    /// Reads the contents of /etc/os-release and parses it into an OsRelease struct.
    pub fn read() -> Result<Self, Error> {
        Ok(Self::parse(
            &std::fs::read_to_string(OS_RELEASE_PATH)
                .context(formatcp!("Failed to read '{OS_RELEASE_PATH}'"))?,
        ))
    }

    /// Reads the contents of /\<root\>/etc/os-release and parses it into an OsRelease struct.
    pub fn read_root(root: impl AsRef<Path>) -> Result<Self, Error> {
        let osrelease_path = path::join_relative(root, OS_RELEASE_PATH);
        Ok(Self::parse(
            &std::fs::read_to_string(&osrelease_path)
                .with_context(|| format!("Failed to read '{}'", osrelease_path.display()))?,
        ))
    }

    /// Returns the distribution of the host.
    pub fn get_distro(&self) -> Distro {
        match self.id.as_deref() {
            Some("mariner") | Some("azurelinux") => Distro::AzureLinux(
                self.version_id
                    .as_deref()
                    .map(|v| {
                        if v.starts_with("2.") {
                            AzureLinuxRelease::AzL2
                        } else if v.starts_with("3.") {
                            AzureLinuxRelease::AzL3
                        } else {
                            trace!("Unknown Azure Linux release: {v}");
                            AzureLinuxRelease::Other
                        }
                    })
                    .unwrap_or_default(),
            ),
            _ => Distro::Other,
        }
    }

    /// Parses the input string into an OsRelease struct.
    fn parse(data: &str) -> Self {
        let mut os_release = OsRelease::default();
        for line in data.lines() {
            if line.is_empty() || line.trim_start().starts_with('#') {
                continue;
            }

            let Some((key, raw_value)) = line.trim().split_once('=') else {
                continue;
            };

            // Fn to trim whitespace and quotes from value, and return as
            // Option<String>
            let value = || {
                Some(
                    raw_value
                        .trim()
                        .trim_matches('\"')
                        .trim_matches('\'')
                        .to_string(),
                )
            };

            match key {
                "ID" => os_release.id = value(),
                "NAME" => os_release.name = value(),
                "VERSION" => os_release.version = value(),
                "VERSION_ID" => os_release.version_id = value(),
                "PRETTY_NAME" => os_release.pretty_name = value(),
                _ => {}
            }
        }

        os_release
    }
}

impl<'de> Deserialize<'de> for OsRelease {
    fn deserialize<D>(deserializer: D) -> Result<OsRelease, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(OsRelease::parse(&String::deserialize(deserializer)?))
    }
}

/// Represents the contents of the extension-release file of a sysext or
/// confext.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ExtensionRelease {
    pub sysext_id: Option<String>,
    pub confext_id: Option<String>,
    pub os_release: OsRelease,
}

impl ExtensionRelease {
    /// Reads the contents of the provided file and parses it into an ExtensionRelease struct.
    pub fn read_file(file: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self::parse(&std::fs::read_to_string(&file).with_context(
            || format!("Failed to read '{}'", file.as_ref().display()),
        )?))
    }

    /// Parses the input string into an ExtensionRelease struct.
    fn parse(data: &str) -> Self {
        let mut sysext_id = None;
        let mut confext_id = None;

        for line in data.lines() {
            if line.is_empty() || line.trim_start().starts_with('#') {
                continue;
            }

            let Some((key, raw_value)) = line.trim().split_once('=') else {
                continue;
            };

            // Fn to trim whitespace and quotes from value, and return as
            // Option<String>
            let value = || {
                Some(
                    raw_value
                        .trim()
                        .trim_matches('\"')
                        .trim_matches('\'')
                        .to_string(),
                )
            };

            match key {
                "SYSEXT_ID" => sysext_id = value(),
                "CONFEXT_ID" => confext_id = value(),
                _ => {}
            }
        }

        Self {
            sysext_id,
            confext_id,
            os_release: OsRelease::parse(data),
        }
    }
}

/// Represents the distribution of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Distro {
    AzureLinux(AzureLinuxRelease),
    Other,
}

impl Distro {
    pub fn is_azl2(&self) -> bool {
        self == &Distro::AzureLinux(AzureLinuxRelease::AzL2)
    }

    pub fn is_azl3(&self) -> bool {
        self == &Distro::AzureLinux(AzureLinuxRelease::AzL3)
    }
}

/// Represents the Azure Linux release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum AzureLinuxRelease {
    #[default]
    Other,
    AzL2,
    AzL3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_azl2() {
        let data = indoc::indoc! {
            r#"
            NAME="Common Base Linux Mariner"
            VERSION="2.0.20240609"
            ID=mariner
            VERSION_ID="2.0"
            PRETTY_NAME="CBL-Mariner/Linux"
            ANSI_COLOR="1;34"
            HOME_URL="https://aka.ms/cbl-mariner"
            BUG_REPORT_URL="https://aka.ms/cbl-mariner"
            SUPPORT_URL="https://aka.ms/cbl-mariner"
            "#,
        };

        let os_release = OsRelease::parse(data);

        assert_eq!(os_release.id, Some("mariner".to_string()));
        assert_eq!(
            os_release.name,
            Some("Common Base Linux Mariner".to_string())
        );
        assert_eq!(os_release.version, Some("2.0.20240609".to_string()));
        assert_eq!(os_release.version_id, Some("2.0".to_string()));
        assert_eq!(
            os_release.pretty_name,
            Some("CBL-Mariner/Linux".to_string())
        );

        assert_eq!(
            os_release.get_distro(),
            Distro::AzureLinux(AzureLinuxRelease::AzL2)
        );
    }

    #[test]
    fn test_parse_azl3() {
        let data = indoc::indoc! {
            r#"
            NAME="Microsoft Azure Linux"
            VERSION="3.0.20240609"
            ID=azurelinux
            VERSION_ID="3.0"
            PRETTY_NAME="Microsoft Azure Linux 3.0"
            ANSI_COLOR="1;34"
            HOME_URL="https://aka.ms/azurelinux"
            BUG_REPORT_URL="https://aka.ms/azurelinux"
            SUPPORT_URL="https://aka.ms/azurelinux"
            "#,
        };

        let os_release = OsRelease::parse(data);

        assert_eq!(os_release.id, Some("azurelinux".to_string()));
        assert_eq!(os_release.name, Some("Microsoft Azure Linux".to_string()));
        assert_eq!(os_release.version, Some("3.0.20240609".to_string()));
        assert_eq!(os_release.version_id, Some("3.0".to_string()));
        assert_eq!(
            os_release.pretty_name,
            Some("Microsoft Azure Linux 3.0".to_string())
        );

        assert_eq!(
            os_release.get_distro(),
            Distro::AzureLinux(AzureLinuxRelease::AzL3)
        );
    }

    #[test]
    fn test_parse_extension_release() {
        let data = indoc::indoc! {
            r#"
            ID=_any
            SYSEXT_ID=docker
            SYSEXT_VERSION_ID=28.0.4
            ARCHITECTURE=x86-64
            "#,
        };

        let extension_release = ExtensionRelease::parse(data);

        assert_eq!(extension_release.sysext_id, Some("docker".to_string()));
        assert_eq!(extension_release.confext_id, None);
        assert_eq!(extension_release.os_release.id, Some("_any".to_string()));
    }
}
