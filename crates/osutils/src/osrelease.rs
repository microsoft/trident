use std::path::Path;

use anyhow::{Context, Error};
use const_format::formatcp;
use log::trace;
use serde::{Deserialize, Deserializer};

use crate::path;

/// Absolute path to the /etc/os-release file.
pub const OS_RELEASE_PATH: &str = "/etc/os-release";

// Distro consts

/// Azure Linux distro ID
pub const AZURE_LINUX_DISTRO_ID: &str = "azurelinux";
/// CBL-Mariner distro ID
pub const CBL_MARINER_DISTRO_ID: &str = "mariner";
/// Ubuntu distro ID
pub const UBUNTU_DISTRO_ID: &str = "ubuntu";
/// Azure Container Linux variant ID
pub const AZURE_CONTAINER_LINUX_VARIANT_ID: &str = "azurecontainerlinux";

/// Returns whether the host is running Azure Linux 2.
pub fn is_azl2() -> Result<bool, Error> {
    Ok(OsRelease::read()?.get_distro().is_azl2())
}

/// Returns whether the host is running Azure Linux 3.
pub fn is_azl3() -> Result<bool, Error> {
    Ok(OsRelease::read()?.get_distro().is_azl3())
}

/// Represents the contents of the /etc/os-release file.
///
/// See <https://www.freedesktop.org/software/systemd/man/latest/os-release.html>
/// for the full specification.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct OsRelease {
    // General information identifying the operating system.
    pub name: Option<String>,
    pub id: Option<String>,
    pub id_like: Option<String>,
    pub pretty_name: Option<String>,
    pub cpe_name: Option<String>,
    pub variant: Option<String>,
    pub variant_id: Option<String>,

    // Information about the version of the operating system.
    pub version: Option<String>,
    pub version_id: Option<String>,
    pub version_codename: Option<String>,
    pub build_id: Option<String>,
    pub image_id: Option<String>,
    pub image_version: Option<String>,
    pub release_type: Option<String>,

    // Presentation information and links.
    pub home_url: Option<String>,
    pub documentation_url: Option<String>,
    pub support_url: Option<String>,
    pub bug_report_url: Option<String>,
    pub privacy_policy_url: Option<String>,
    pub support_end: Option<String>,
    pub logo: Option<String>,
    pub ansi_color: Option<String>,
    pub ansi_color_reverse: Option<String>,
    pub vendor_name: Option<String>,
    pub vendor_url: Option<String>,
    pub experiment: Option<String>,
    pub experiment_url: Option<String>,

    // Distribution-level defaults and metadata.
    pub default_hostname: Option<String>,
    pub architecture: Option<String>,
    pub sysext_level: Option<String>,
    pub confext_level: Option<String>,
}

impl OsRelease {
    /// Represents an empty OsRelease, where all fields are set to None.
    pub const EMPTY: Self = Self {
        name: None,
        id: None,
        id_like: None,
        pretty_name: None,
        cpe_name: None,
        variant: None,
        variant_id: None,
        version: None,
        version_id: None,
        version_codename: None,
        build_id: None,
        image_id: None,
        image_version: None,
        release_type: None,
        home_url: None,
        documentation_url: None,
        support_url: None,
        bug_report_url: None,
        privacy_policy_url: None,
        support_end: None,
        logo: None,
        ansi_color: None,
        ansi_color_reverse: None,
        vendor_name: None,
        vendor_url: None,
        experiment: None,
        experiment_url: None,
        default_hostname: None,
        architecture: None,
        sysext_level: None,
        confext_level: None,
    };

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

    /// Returns the distribution represented by this `OsRelease` instance.
    pub fn get_distro(&self) -> Distro {
        match self.id.as_deref() {
            Some(AZURE_LINUX_DISTRO_ID)
                if self.variant_id.as_deref() == Some(AZURE_CONTAINER_LINUX_VARIANT_ID) =>
            {
                Distro::AzureContainerLinux
            }
            Some(CBL_MARINER_DISTRO_ID) | Some(AZURE_LINUX_DISTRO_ID) => Distro::AzureLinux(
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
            Some(UBUNTU_DISTRO_ID) => Distro::Ubuntu,
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
                "NAME" => os_release.name = value(),
                "ID" => os_release.id = value(),
                "ID_LIKE" => os_release.id_like = value(),
                "PRETTY_NAME" => os_release.pretty_name = value(),
                "CPE_NAME" => os_release.cpe_name = value(),
                "VARIANT" => os_release.variant = value(),
                "VARIANT_ID" => os_release.variant_id = value(),
                "VERSION" => os_release.version = value(),
                "VERSION_ID" => os_release.version_id = value(),
                "VERSION_CODENAME" => os_release.version_codename = value(),
                "BUILD_ID" => os_release.build_id = value(),
                "IMAGE_ID" => os_release.image_id = value(),
                "IMAGE_VERSION" => os_release.image_version = value(),
                "RELEASE_TYPE" => os_release.release_type = value(),
                "HOME_URL" => os_release.home_url = value(),
                "DOCUMENTATION_URL" => os_release.documentation_url = value(),
                "SUPPORT_URL" => os_release.support_url = value(),
                "BUG_REPORT_URL" => os_release.bug_report_url = value(),
                "PRIVACY_POLICY_URL" => os_release.privacy_policy_url = value(),
                "SUPPORT_END" => os_release.support_end = value(),
                "LOGO" => os_release.logo = value(),
                "ANSI_COLOR" => os_release.ansi_color = value(),
                "ANSI_COLOR_REVERSE" => os_release.ansi_color_reverse = value(),
                "VENDOR_NAME" => os_release.vendor_name = value(),
                "VENDOR_URL" => os_release.vendor_url = value(),
                "EXPERIMENT" => os_release.experiment = value(),
                "EXPERIMENT_URL" => os_release.experiment_url = value(),
                "DEFAULT_HOSTNAME" => os_release.default_hostname = value(),
                "ARCHITECTURE" => os_release.architecture = value(),
                "SYSEXT_LEVEL" => os_release.sysext_level = value(),
                "CONFEXT_LEVEL" => os_release.confext_level = value(),
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

impl Default for &OsRelease {
    fn default() -> Self {
        &OsRelease::EMPTY
    }
}

/// Represents the contents of the extension-release file of a sysext or
/// confext.
///
/// See <https://www.freedesktop.org/software/systemd/man/latest/os-release.html>
/// for the full specification.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ExtensionRelease {
    pub sysext_id: Option<String>,
    pub confext_id: Option<String>,
    pub sysext_scope: Option<String>,
    pub confext_scope: Option<String>,
    pub portable_prefixes: Option<String>,
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
        let mut sysext_scope = None;
        let mut confext_scope = None;
        let mut portable_prefixes = None;

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
                "SYSEXT_SCOPE" => sysext_scope = value(),
                "CONFEXT_SCOPE" => confext_scope = value(),
                "PORTABLE_PREFIXES" => portable_prefixes = value(),
                _ => {}
            }
        }

        Self {
            sysext_id,
            confext_id,
            sysext_scope,
            confext_scope,
            portable_prefixes,
            os_release: OsRelease::parse(data),
        }
    }
}

/// Represents the distribution of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Distro {
    AzureLinux(AzureLinuxRelease),
    AzureContainerLinux,
    Ubuntu,
    Other,
}

impl Distro {
    pub fn is_azl2(&self) -> bool {
        self == &Distro::AzureLinux(AzureLinuxRelease::AzL2)
    }

    pub fn is_azl3(&self) -> bool {
        self == &Distro::AzureLinux(AzureLinuxRelease::AzL3)
    }

    pub fn is_acl(&self) -> bool {
        self == &Distro::AzureContainerLinux
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

    #[test]
    fn test_parse_all_fields() {
        let data = indoc::indoc! {
            r#"
            NAME="Microsoft Azure Linux"
            ID=azurelinux
            ID_LIKE="fedora"
            PRETTY_NAME="Microsoft Azure Linux 3.0 (Workstation Edition)"
            CPE_NAME="cpe:/o:microsoft:azurelinux:3"
            VARIANT="Workstation Edition"
            VARIANT_ID=workstation
            VERSION="3.0.20240609 (Workstation Edition)"
            VERSION_ID="3.0"
            VERSION_CODENAME=azl3
            BUILD_ID="2024-06-09.1"
            IMAGE_ID=azurelinux-workstation
            IMAGE_VERSION=3.0.1
            RELEASE_TYPE=stable
            HOME_URL="https://aka.ms/azurelinux"
            DOCUMENTATION_URL="https://learn.microsoft.com/azure/azure-linux/"
            SUPPORT_URL="https://aka.ms/azurelinux"
            BUG_REPORT_URL="https://aka.ms/azurelinux"
            PRIVACY_POLICY_URL="https://privacy.microsoft.com/"
            SUPPORT_END=2027-06-09
            LOGO=azurelinux-logo
            ANSI_COLOR="1;34"
            ANSI_COLOR_REVERSE="0;48;2;0;120;212"
            VENDOR_NAME="Microsoft"
            VENDOR_URL="https://microsoft.com/"
            EXPERIMENT="Switch to DNF5"
            EXPERIMENT_URL="https://aka.ms/azurelinux/dnf5"
            DEFAULT_HOSTNAME=azurelinux
            ARCHITECTURE=x86-64
            SYSEXT_LEVEL=2
            CONFEXT_LEVEL=3
            "#,
        };

        let os_release = OsRelease::parse(data);

        assert_eq!(os_release.name, Some("Microsoft Azure Linux".to_string()));
        assert_eq!(os_release.id, Some("azurelinux".to_string()));
        assert_eq!(os_release.id_like, Some("fedora".to_string()));
        assert_eq!(
            os_release.pretty_name,
            Some("Microsoft Azure Linux 3.0 (Workstation Edition)".to_string())
        );
        assert_eq!(
            os_release.cpe_name,
            Some("cpe:/o:microsoft:azurelinux:3".to_string())
        );
        assert_eq!(os_release.variant, Some("Workstation Edition".to_string()));
        assert_eq!(os_release.variant_id, Some("workstation".to_string()));
        assert_eq!(
            os_release.version,
            Some("3.0.20240609 (Workstation Edition)".to_string())
        );
        assert_eq!(os_release.version_id, Some("3.0".to_string()));
        assert_eq!(os_release.version_codename, Some("azl3".to_string()));
        assert_eq!(os_release.build_id, Some("2024-06-09.1".to_string()));
        assert_eq!(
            os_release.image_id,
            Some("azurelinux-workstation".to_string())
        );
        assert_eq!(os_release.image_version, Some("3.0.1".to_string()));
        assert_eq!(os_release.release_type, Some("stable".to_string()));
        assert_eq!(
            os_release.home_url,
            Some("https://aka.ms/azurelinux".to_string())
        );
        assert_eq!(
            os_release.documentation_url,
            Some("https://learn.microsoft.com/azure/azure-linux/".to_string())
        );
        assert_eq!(
            os_release.support_url,
            Some("https://aka.ms/azurelinux".to_string())
        );
        assert_eq!(
            os_release.bug_report_url,
            Some("https://aka.ms/azurelinux".to_string())
        );
        assert_eq!(
            os_release.privacy_policy_url,
            Some("https://privacy.microsoft.com/".to_string())
        );
        assert_eq!(os_release.support_end, Some("2027-06-09".to_string()));
        assert_eq!(os_release.logo, Some("azurelinux-logo".to_string()));
        assert_eq!(os_release.ansi_color, Some("1;34".to_string()));
        assert_eq!(
            os_release.ansi_color_reverse,
            Some("0;48;2;0;120;212".to_string())
        );
        assert_eq!(os_release.vendor_name, Some("Microsoft".to_string()));
        assert_eq!(
            os_release.vendor_url,
            Some("https://microsoft.com/".to_string())
        );
        assert_eq!(os_release.experiment, Some("Switch to DNF5".to_string()));
        assert_eq!(
            os_release.experiment_url,
            Some("https://aka.ms/azurelinux/dnf5".to_string())
        );
        assert_eq!(os_release.default_hostname, Some("azurelinux".to_string()));
        assert_eq!(os_release.architecture, Some("x86-64".to_string()));
        assert_eq!(os_release.sysext_level, Some("2".to_string()));
        assert_eq!(os_release.confext_level, Some("3".to_string()));

        assert_eq!(
            os_release.get_distro(),
            Distro::AzureLinux(AzureLinuxRelease::AzL3)
        );
    }

    #[test]
    fn test_get_distro_azure_container_linux() {
        let data = indoc::indoc! {
            r#"
            NAME="Microsoft Azure Linux"
            VERSION="3.0.20240609"
            ID=azurelinux
            VERSION_ID="3.0"
            VARIANT_ID=azurecontainerlinux
            PRETTY_NAME="Microsoft Azure Linux 3.0"
            "#,
        };

        let os_release = OsRelease::parse(data);
        assert_eq!(os_release.get_distro(), Distro::AzureContainerLinux);
    }

    #[test]
    fn test_get_distro_ubuntu() {
        let data = indoc::indoc! {
            r#"
            NAME="Ubuntu"
            VERSION="22.04.3 LTS (Jammy Jellyfish)"
            ID=ubuntu
            VERSION_ID="22.04"
            PRETTY_NAME="Ubuntu 22.04.3 LTS"
            "#,
        };

        let os_release = OsRelease::parse(data);
        assert_eq!(os_release.get_distro(), Distro::Ubuntu);
    }

    #[test]
    fn test_get_distro_unknown() {
        let data = indoc::indoc! {
            r#"
            NAME="Fedora Linux"
            ID=fedora
            VERSION_ID="39"
            "#,
        };

        let os_release = OsRelease::parse(data);
        assert_eq!(os_release.get_distro(), Distro::Other);
    }
}
