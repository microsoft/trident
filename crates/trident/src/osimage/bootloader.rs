#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BootloaderMetadata {
    pub(crate) bootloader_type: BootloaderMetadataType,
    pub(crate) systemd_boot: Option<SystemdBootMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BootloaderMetadataType {
    SystemdBoot,
    Grub,
    EfitreeloaderMshvSystemdBoot,
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemdBootMetadata {
    pub(crate) entries: Vec<SystemdBootEntryMetadata>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SystemdBootEntryMetadata {
    pub(crate) entry_type: SystemdBootEntryMetadataType,
    pub(crate) kernel: String,
    pub(crate) path: String,
    pub(crate) cmdline: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SystemdBootEntryMetadataType {
    UkiStandalone,
    UkiConfig,
    Config,
    Unknown(String),
}
