use anyhow::{Context, Error};

use crate::dependencies::Dependency;

// Grab the kernel version using the `uname` command
pub fn kernel_release() -> Result<String, Error> {
    Dependency::Uname
        .cmd()
        .arg("-r")
        .output_and_check()
        .context("Failed to run uname -r")
}

/// Parsed kernel version with major and minor components.
///
/// Implements `Ord` so callers can compare against feature thresholds
/// (e.g., `kv >= KernelVersion { major: 6, minor: 7 }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct KernelVersion {
    pub major: u32,
    pub minor: u32,
}

impl KernelVersion {
    /// Parse a kernel version from a `uname -r` string.
    ///
    /// Extracts the leading `major.minor` from strings like:
    /// - `6.6.78.2-1.cm2`
    /// - `6.7.0-1.cm2`
    /// - `7.0.0`
    ///
    /// Returns `None` if the string cannot be parsed.
    pub fn parse(release: &str) -> Option<Self> {
        // Strip everything after the first '-' (e.g. "-1.cm2"), then split on '.'.
        let numeric_part = release.split('-').next()?;
        let mut parts = numeric_part.split('.');
        let major = parts.next()?.parse::<u32>().ok()?;
        let minor = parts.next()?.parse::<u32>().ok()?;
        Some(KernelVersion { major, minor })
    }

    /// Returns the kernel version of the running system.
    ///
    /// Returns `Err` if the `uname` command fails to execute, or `Ok(None)`
    /// if the output cannot be parsed into a major.minor version.
    pub fn running() -> Result<Option<Self>, Error> {
        let release = kernel_release()?;
        Ok(Self::parse(&release))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_release() {
        kernel_release().unwrap();
    }

    #[test]
    fn test_parse_azl_kernel() {
        let v = KernelVersion::parse("6.6.78.2-1.cm2").unwrap();
        assert_eq!(v, KernelVersion { major: 6, minor: 6 });
        assert!(v < KernelVersion { major: 6, minor: 7 });
    }

    #[test]
    fn test_parse_67_kernel() {
        let v = KernelVersion::parse("6.7.0-1.cm2").unwrap();
        assert_eq!(v, KernelVersion { major: 6, minor: 7 });
        assert!(v >= KernelVersion { major: 6, minor: 7 });
    }

    #[test]
    fn test_parse_major_7() {
        let v = KernelVersion::parse("7.0.0").unwrap();
        assert_eq!(v, KernelVersion { major: 7, minor: 0 });
        assert!(v >= KernelVersion { major: 6, minor: 7 });
    }

    #[test]
    fn test_parse_simple() {
        let v = KernelVersion::parse("5.15").unwrap();
        assert_eq!(
            v,
            KernelVersion {
                major: 5,
                minor: 15
            }
        );
        assert!(v < KernelVersion { major: 6, minor: 7 });
    }

    #[test]
    fn test_parse_garbage() {
        assert!(KernelVersion::parse("not-a-version").is_none());
        assert!(KernelVersion::parse("").is_none());
        assert!(KernelVersion::parse("6").is_none());
    }
}
