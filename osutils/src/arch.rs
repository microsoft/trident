use serde::{Deserialize, Deserializer};
use strum_macros::IntoStaticStr;

/// System architecture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr)]
pub enum SystemArchitecture {
    /// 32-bit x86
    #[strum(serialize = "x86")]
    X86,

    /// 64-bit x86
    #[strum(serialize = "amd64")]
    Amd64,

    /// 32-bit ARM
    #[strum(serialize = "arm")]
    Arm,

    /// 64-bit ARM
    #[strum(serialize = "arm64")]
    Aarch64,

    /// Other
    #[strum(serialize = "other")]
    Other,
}

impl SystemArchitecture {
    /// Get the current system architecture
    pub fn current() -> Self {
        if cfg!(target_arch = "x86") {
            SystemArchitecture::X86
        } else if cfg!(target_arch = "x86_64") {
            SystemArchitecture::Amd64
        } else if cfg!(target_arch = "arm") {
            SystemArchitecture::Arm
        } else if cfg!(target_arch = "aarch64") {
            SystemArchitecture::Aarch64
        } else {
            SystemArchitecture::Other
        }
    }
}

impl From<&str> for SystemArchitecture {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "x86" | "i386" => SystemArchitecture::X86,
            "x64" | "amd64" | "x86_64" => SystemArchitecture::Amd64,
            "arm" | "aarch32" => SystemArchitecture::Arm,
            "arm64" | "aarch64" => SystemArchitecture::Aarch64,
            _ => SystemArchitecture::Other,
        }
    }
}

impl<'de> Deserialize<'de> for SystemArchitecture {
    fn deserialize<D>(deserializer: D) -> Result<SystemArchitecture, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(SystemArchitecture::from(
            String::deserialize(deserializer)?.as_str(),
        ))
    }
}
