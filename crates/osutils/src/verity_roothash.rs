//! Verity root hash utilities.
//!
//! Provides a [`VerityRootHash`] newtype for safe handling of dm-verity root
//! hashes, including case-insensitive comparison, preview formatting, and
//! extraction from `/proc/cmdline`.

use std::fmt;
use std::fs;

/// A dm-verity root hash, normalized to lowercase with whitespace trimmed.
///
/// An empty string is not a valid hash — comparisons against or with an empty
/// `VerityRootHash` always return `false`.
#[derive(Debug, Clone, Eq)]
pub struct VerityRootHash(String);

impl VerityRootHash {
    /// Creates a new `VerityRootHash`, normalizing to lowercase and trimming
    /// whitespace. Returns `None` if the input is empty after trimming.
    pub fn new(hash: &str) -> Option<Self> {
        let normalized = hash.trim().to_lowercase();
        if normalized.is_empty() {
            None
        } else {
            Some(Self(normalized))
        }
    }

    /// Returns a truncated preview of the hash for log messages (first 16 chars).
    pub fn preview(&self) -> &str {
        let end = self
            .0
            .char_indices()
            .nth(16)
            .map(|(i, _)| i)
            .unwrap_or(self.0.len());
        &self.0[..end]
    }

    /// Reads the active USR verity root hash from `/proc/cmdline`.
    ///
    /// Looks for a `usrhash=<hex>` parameter in the kernel command line.
    /// Returns `None` if the parameter is not present or `/proc/cmdline`
    /// cannot be read.
    pub fn from_proc_cmdline() -> Option<Self> {
        let cmdline = fs::read_to_string("/proc/cmdline").ok()?;
        cmdline
            .split_whitespace()
            .find_map(|field| field.strip_prefix("usrhash="))
            .and_then(Self::new)
    }
}

impl PartialEq for VerityRootHash {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl AsRef<str> for VerityRootHash {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VerityRootHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_normalizes_case_and_whitespace() {
        let h = VerityRootHash::new("  ABC123  ").unwrap();
        assert_eq!(h.as_ref(), "abc123");
    }

    #[test]
    fn new_rejects_empty() {
        assert!(VerityRootHash::new("").is_none());
        assert!(VerityRootHash::new("   ").is_none());
    }

    #[test]
    fn equality_is_case_insensitive() {
        let a = VerityRootHash::new("ABC123").unwrap();
        let b = VerityRootHash::new("abc123").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn inequality_on_different_hashes() {
        let a = VerityRootHash::new("abc123").unwrap();
        let b = VerityRootHash::new("def456").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn preview_truncates_to_16() {
        let h = VerityRootHash::new("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(h.preview(), "0123456789abcdef");
    }

    #[test]
    fn preview_short_hash_unchanged() {
        let h = VerityRootHash::new("abc").unwrap();
        assert_eq!(h.preview(), "abc");
    }

    #[test]
    fn as_ref_returns_normalized() {
        let h = VerityRootHash::new("ABCDEF").unwrap();
        assert_eq!(h.as_ref(), "abcdef");
    }

    #[test]
    fn display_shows_full_hash() {
        let h = VerityRootHash::new("abc123").unwrap();
        assert_eq!(format!("{h}"), "abc123");
    }
}
