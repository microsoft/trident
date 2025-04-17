use serde::{de::Error, Deserialize, Deserializer};
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
}

impl SystemArchitecture {
    /// Get the current system architecture
    pub fn current() -> Self {
        #[cfg(target_arch = "x86")]
        {
            SystemArchitecture::X86
        }

        #[cfg(target_arch = "x86_64")]
        {
            SystemArchitecture::Amd64
        }

        #[cfg(target_arch = "arm")]
        {
            SystemArchitecture::Arm
        }

        #[cfg(target_arch = "aarch64")]
        {
            SystemArchitecture::Aarch64
        }
    }
}

impl<'de> Deserialize<'de> for SystemArchitecture {
    fn deserialize<D>(deserializer: D) -> Result<SystemArchitecture, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match &*String::deserialize(deserializer)?.to_lowercase() {
            "x86" | "i386" => SystemArchitecture::X86,
            "x64" | "amd64" | "x86_64" => SystemArchitecture::Amd64,
            "arm" | "aarch32" => SystemArchitecture::Arm,
            "arm64" | "aarch64" => SystemArchitecture::Aarch64,
            arch => {
                return Err(D::Error::custom(format!(
                    "unknown system architecture '{arch}'",
                )))
            }
        })
    }
}
