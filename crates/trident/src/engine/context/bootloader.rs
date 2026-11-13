use crate::osimage::{
    bootloader::{
        BootloaderMetadata, BootloaderMetadataType, SystemdBootEntryMetadata,
        SystemdBootEntryMetadataType, SystemdBootMetadata,
    },
    OsImage,
};

/// Engine-facing bootloader settings copied from the OS image metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Bootloader {
    pub(crate) bootloader_type: BootloaderType,
    pub(crate) systemd_boot: Option<SystemdBoot>,
}

impl Bootloader {
    pub(super) fn from_image(image: &OsImage) -> Option<Self> {
        image.bootloader().map(Self::from)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BootloaderType {
    SystemdBoot,
    Grub,
    EfitreeloaderMshvSystemdBoot,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemdBoot {
    pub(crate) entries: Vec<SystemdBootEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemdBootEntry {
    pub(crate) entry_type: SystemdBootEntryType,
    pub(crate) kernel: String,
    pub(crate) path: PathBuf,
    pub(crate) cmdline: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SystemdBootEntryType {
    UkiStandalone,
    UkiConfig,
    Config,
    Unknown(String),
}

impl From<BootloaderMetadata> for Bootloader {
    fn from(bootloader: BootloaderMetadata) -> Self {
        Self {
            bootloader_type: BootloaderType::from(bootloader.bootloader_type),
            systemd_boot: bootloader.systemd_boot.map(SystemdBoot::from),
        }
    }
}

impl From<BootloaderMetadataType> for BootloaderType {
    fn from(bootloader_type: BootloaderMetadataType) -> Self {
        match bootloader_type {
            BootloaderMetadataType::SystemdBoot => Self::SystemdBoot,
            BootloaderMetadataType::Grub => Self::Grub,
            BootloaderMetadataType::EfitreeloaderMshvSystemdBoot => {
                Self::EfitreeloaderMshvSystemdBoot
            }
            BootloaderMetadataType::Unknown(value) => Self::Unknown(value),
        }
    }
}

impl From<SystemdBootMetadata> for SystemdBoot {
    fn from(systemd_boot: SystemdBootMetadata) -> Self {
        Self {
            entries: systemd_boot
                .entries
                .into_iter()
                .map(SystemdBootEntry::from)
                .collect(),
        }
    }
}

impl From<SystemdBootEntryMetadata> for SystemdBootEntry {
    fn from(entry: SystemdBootEntryMetadata) -> Self {
        Self {
            entry_type: SystemdBootEntryType::from(entry.entry_type),
            kernel: entry.kernel,
            path: entry.path,
            cmdline: entry.cmdline,
        }
    }
}

impl From<SystemdBootEntryMetadataType> for SystemdBootEntryType {
    fn from(entry_type: SystemdBootEntryMetadataType) -> Self {
        match entry_type {
            SystemdBootEntryMetadataType::UkiStandalone => Self::UkiStandalone,
            SystemdBootEntryMetadataType::UkiConfig => Self::UkiConfig,
            SystemdBootEntryMetadataType::Config => Self::Config,
            SystemdBootEntryMetadataType::Unknown(value) => Self::Unknown(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::osimage::mock::MockOsImage;

    #[test]
    fn test_from_image() {
        assert_eq!(
            Bootloader::from_image(&OsImage::mock(MockOsImage::new())),
            None
        );

        let image = OsImage::mock(MockOsImage::new().with_bootloader(BootloaderMetadata {
            bootloader_type: BootloaderMetadataType::EfitreeloaderMshvSystemdBoot,
            systemd_boot: Some(SystemdBootMetadata {
                entries: vec![SystemdBootEntryMetadata {
                    entry_type: SystemdBootEntryMetadataType::UkiStandalone,
                    kernel: "vmlinuz".to_string(),
                    path: PathBuf::from("EFI/Linux/image.efi"),
                    cmdline: "quiet".to_string(),
                }],
            }),
        }));

        assert_eq!(
            Bootloader::from_image(&image),
            Some(Bootloader {
                bootloader_type: BootloaderType::EfitreeloaderMshvSystemdBoot,
                systemd_boot: Some(SystemdBoot {
                    entries: vec![SystemdBootEntry {
                        entry_type: SystemdBootEntryType::UkiStandalone,
                        kernel: "vmlinuz".to_string(),
                        path: PathBuf::from("EFI/Linux/image.efi"),
                        cmdline: "quiet".to_string(),
                    }],
                }),
            })
        );
    }

    #[test]
    fn test_unknown_values_are_preserved() {
        let image = OsImage::mock(MockOsImage::new().with_bootloader(BootloaderMetadata {
            bootloader_type: BootloaderMetadataType::Unknown("future-bootloader".to_string()),
            systemd_boot: Some(SystemdBootMetadata {
                entries: vec![SystemdBootEntryMetadata {
                    entry_type: SystemdBootEntryMetadataType::Unknown("future-entry".to_string()),
                    kernel: "vmlinuz".to_string(),
                    path: PathBuf::from("loader/entries/future.conf"),
                    cmdline: "quiet".to_string(),
                }],
            }),
        }));

        assert_eq!(
            Bootloader::from_image(&image),
            Some(Bootloader {
                bootloader_type: BootloaderType::Unknown("future-bootloader".to_string()),
                systemd_boot: Some(SystemdBoot {
                    entries: vec![SystemdBootEntry {
                        entry_type: SystemdBootEntryType::Unknown("future-entry".to_string()),
                        kernel: "vmlinuz".to_string(),
                        path: PathBuf::from("loader/entries/future.conf"),
                        cmdline: "quiet".to_string(),
                    }],
                }),
            })
        );
    }
}
use std::path::PathBuf;
