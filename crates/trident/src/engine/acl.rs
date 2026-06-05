//! ACL (Azure Container Linux) UKI-specific constants and helpers.
//!
//! ACL uses fixed PARTUUIDs for USR A/B partitions and a verity addon that
//! places the root hash in the kernel command line as `usrhash=<hex>`.

use std::fs;

// ACL UKI disk layout defines fixed PARTUUIDs for the USR A/B data partitions.
// These are from acl-scripts disk_layout_uki.json.
pub const ACL_USR_A_PARTUUID: &str = "7130c94a-213a-4e5a-8e26-6cce9662f132";
pub const ACL_USR_B_PARTUUID: &str = "e03dd35c-7c2d-4a47-b3fe-27f15780a57c";

/// Reads the active USR verity root hash from `/proc/cmdline`.
///
/// ACL UKI images include a `usrhash=<hex>` parameter in the kernel command
/// line (contributed by the verity addon). Returns `None` if the parameter
/// is not present or `/proc/cmdline` cannot be read.
pub fn read_active_usr_roothash() -> Option<String> {
    let cmdline = fs::read_to_string("/proc/cmdline").ok()?;
    cmdline
        .split_whitespace()
        .find_map(|field| field.strip_prefix("usrhash="))
        .map(|hash| hash.to_owned())
}

/// Compares two verity root hashes for equality after trimming whitespace and
/// lowercasing.  Returns `false` if either hash is empty (an empty hash is not
/// a valid identity — `"" == ""` would incorrectly pass).
pub fn verity_hashes_match(a: &str, b: &str) -> bool {
    let a = a.trim().to_lowercase();
    let b = b.trim().to_lowercase();
    !a.is_empty() && !b.is_empty() && a == b
}

/// Returns a char-safe preview of a hash string for log messages.
/// Uses `chars().take(16)` instead of byte-index slicing to avoid panics
/// on non-ASCII input (which shouldn't happen for hex hashes, but defense
/// in depth).
pub fn hash_preview(hash: &str) -> String {
    hash.trim().to_lowercase().chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verity_hashes_match_identical() {
        assert!(verity_hashes_match("abc123", "abc123"));
    }

    #[test]
    fn verity_hashes_match_case_insensitive() {
        assert!(verity_hashes_match("ABC123", "abc123"));
        assert!(verity_hashes_match("abc123", "ABC123"));
    }

    #[test]
    fn verity_hashes_match_trims_whitespace() {
        assert!(verity_hashes_match("  abc123  ", "abc123"));
        assert!(verity_hashes_match("abc123", "  abc123\n"));
    }

    #[test]
    fn verity_hashes_match_rejects_empty() {
        assert!(!verity_hashes_match("", ""));
        assert!(!verity_hashes_match("abc123", ""));
        assert!(!verity_hashes_match("", "abc123"));
    }

    #[test]
    fn verity_hashes_match_rejects_whitespace_only() {
        assert!(!verity_hashes_match("   ", "   "));
        assert!(!verity_hashes_match("abc123", "   "));
    }

    #[test]
    fn verity_hashes_match_rejects_different() {
        assert!(!verity_hashes_match("abc123", "def456"));
    }

    #[test]
    fn hash_preview_truncates_to_16() {
        let long_hash = "0123456789abcdef0123456789abcdef";
        assert_eq!(hash_preview(long_hash), "0123456789abcdef");
    }

    #[test]
    fn hash_preview_short_hash_unchanged() {
        assert_eq!(hash_preview("abc"), "abc");
    }

    #[test]
    fn hash_preview_lowercases() {
        assert_eq!(hash_preview("ABCDEF"), "abcdef");
    }

    #[test]
    fn hash_preview_trims() {
        assert_eq!(hash_preview("  abc  "), "abc");
    }

    #[test]
    fn hash_preview_empty() {
        assert_eq!(hash_preview(""), "");
    }
}
