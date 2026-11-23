use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Bootloader {
    bootloader_type: BootloaderType,
    systemd_boot: Option<SystemdBoot>,
}

impl Bootloader {
    pub(crate) fn new(bootloader_type: BootloaderType, systemd_boot: Option<SystemdBoot>) -> Self {
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

    pub(crate) fn uses_mshv(&self) -> bool {
        self.bootloader_type == BootloaderType::EfitreeloaderMshvSystemdBoot
    }

    /// Includes both direct systemd-boot and the efitreeloader/MSHV chain.
    pub(crate) fn uses_systemd_boot(&self) -> bool {
        matches!(
            &self.bootloader_type,
            BootloaderType::SystemdBoot | BootloaderType::EfitreeloaderMshvSystemdBoot
        )
    }

    pub(crate) fn uses_grub(&self) -> bool {
        self.bootloader_type == BootloaderType::Grub
    }

    pub(crate) fn uses_efitreeloader(&self) -> bool {
        self.bootloader_type == BootloaderType::EfitreeloaderMshvSystemdBoot
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bootloader_components() {
        let systemd_boot = Bootloader::new(BootloaderType::SystemdBoot, None);
        assert!(systemd_boot.uses_systemd_boot());
        assert!(!systemd_boot.uses_mshv());
        assert!(!systemd_boot.uses_grub());
        assert!(!systemd_boot.uses_efitreeloader());

        let grub = Bootloader::new(BootloaderType::Grub, None);
        assert!(grub.uses_grub());
        assert!(!grub.uses_mshv());
        assert!(!grub.uses_systemd_boot());
        assert!(!grub.uses_efitreeloader());

        let mshv = Bootloader::new(BootloaderType::EfitreeloaderMshvSystemdBoot, None);
        assert!(mshv.uses_mshv());
        assert!(mshv.uses_systemd_boot());
        assert!(mshv.uses_efitreeloader());
        assert!(!mshv.uses_grub());

        let unknown = Bootloader::new(BootloaderType::Unknown("future".to_string()), None);
        assert!(!unknown.uses_mshv());
        assert!(!unknown.uses_systemd_boot());
        assert!(!unknown.uses_grub());
        assert!(!unknown.uses_efitreeloader());
    }
}
