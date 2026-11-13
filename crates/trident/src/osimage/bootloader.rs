use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Bootloader {
    bootloader_type: BootloaderType,
    systemd_boot: Option<SystemdBoot>,
}

impl Bootloader {
    pub(super) fn new(bootloader_type: BootloaderType, systemd_boot: Option<SystemdBoot>) -> Self {
        Self {
            bootloader_type,
            systemd_boot,
        }
    }

    pub(crate) fn bootloader_type(&self) -> &BootloaderType {
        &self.bootloader_type
    }

    pub(crate) fn systemd_boot(&self) -> Option<&SystemdBoot> {
        self.systemd_boot.as_ref()
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
    entries: Vec<SystemdBootEntry>,
}

impl SystemdBoot {
    pub(super) fn new(entries: Vec<SystemdBootEntry>) -> Self {
        Self { entries }
    }

    pub(crate) fn entries(&self) -> &[SystemdBootEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemdBootEntry {
    entry_type: SystemdBootEntryType,
    kernel: String,
    path: PathBuf,
    cmdline: String,
}

impl SystemdBootEntry {
    pub(super) fn new(
        entry_type: SystemdBootEntryType,
        kernel: String,
        path: PathBuf,
        cmdline: String,
    ) -> Self {
        Self {
            entry_type,
            kernel,
            path,
            cmdline,
        }
    }

    pub(crate) fn entry_type(&self) -> &SystemdBootEntryType {
        &self.entry_type
    }

    pub(crate) fn kernel(&self) -> &str {
        &self.kernel
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn cmdline(&self) -> &str {
        &self.cmdline
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SystemdBootEntryType {
    UkiStandalone,
    UkiConfig,
    Config,
    Unknown(String),
}
