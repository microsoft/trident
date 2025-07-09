use serde::{de::Error, Deserialize, Deserializer};
use strum_macros::{EnumIter, IntoStaticStr};

/// System architecture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, IntoStaticStr, EnumIter)]
pub enum SystemArchitecture {
    /// 64-bit x86
    #[strum(serialize = "amd64")]
    Amd64,

    /// 64-bit ARM
    #[strum(serialize = "arm64")]
    Aarch64,
}

impl SystemArchitecture {
    /// Get the current system architecture
    pub const fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            SystemArchitecture::Amd64
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
            "x64" | "amd64" | "x86_64" => SystemArchitecture::Amd64,
            "arm64" | "aarch64" => SystemArchitecture::Aarch64,
            arch => {
                return Err(D::Error::custom(format!(
                    "unknown system architecture '{arch}'",
                )))
            }
        })
    }
}

/// System architecture
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum PackageArchitecture {
    /// NoArch
    #[serde(alias = "noarch")]
    #[serde(alias = "(none)")]
    NoArch,

    #[serde(untagged)]
    Specific(SystemArchitecture),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_system_architecture_amd64_variants() {
        let variants = vec!["amd64", "x64", "x86_64", "AMD64", "X64", "X86_64"];
        for arch in variants {
            let deser: SystemArchitecture = serde_json::from_str(&format!("\"{arch}\"")).unwrap();
            assert_eq!(deser, SystemArchitecture::Amd64);
        }
    }

    #[test]
    fn test_deserialize_system_architecture_aarch64_variants() {
        let variants = vec!["arm64", "aarch64", "ARM64", "AARCH64"];
        for arch in variants {
            let deser: SystemArchitecture = serde_json::from_str(&format!("\"{arch}\"")).unwrap();
            assert_eq!(deser, SystemArchitecture::Aarch64);
        }
    }

    #[test]
    fn test_deserialize_system_architecture_invalid() {
        let result: Result<SystemArchitecture, _> = serde_json::from_str("\"foobar\"");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown system architecture"));
    }

    #[test]
    fn test_deserialize_package_architecture_noarch() {
        let variants = vec!["noarch", "(none)", "NoArch"];
        for arch in variants {
            let deser: PackageArchitecture = serde_json::from_str(&format!("\"{arch}\"")).unwrap();
            assert_eq!(deser, PackageArchitecture::NoArch);
        }
    }

    #[test]
    fn test_deserialize_package_architecture_specific() {
        let deser: PackageArchitecture = serde_json::from_str("\"amd64\"").unwrap();
        assert_eq!(
            deser,
            PackageArchitecture::Specific(SystemArchitecture::Amd64)
        );

        let deser: PackageArchitecture = serde_json::from_str("\"arm64\"").unwrap();
        assert_eq!(
            deser,
            PackageArchitecture::Specific(SystemArchitecture::Aarch64)
        );
    }
}
