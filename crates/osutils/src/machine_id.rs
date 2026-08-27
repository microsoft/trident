use std::{fs, path::Path};

use anyhow::{ensure, Context, Error};
use sha2::{Digest, Sha384};
use uuid::Uuid;

const MACHINE_ID_FILE: &str = "/etc/machine-id";
const BOOT_ID_FILE: &str = "/proc/sys/kernel/random/boot_id";
const PROC_STAT_FILE: &str = "/proc/stat";

#[derive(Debug, Clone, Copy)]
pub struct MachineId(u128);

impl MachineId {
    pub fn read() -> Result<Self, Error> {
        Self::read_inner(MACHINE_ID_FILE)
    }

    fn read_inner(path: impl AsRef<Path>) -> Result<Self, Error> {
        let id = fs::read_to_string(path.as_ref()).with_context(|| {
            format!(
                "Failed to read machine ID from '{}'",
                path.as_ref().display()
            )
        })?;
        let trimmed = id.trim();
        Ok(Self(u128::from_str_radix(trimmed, 16).with_context(
            || {
                format!(
                    "Failed to parse machine '{}' ID read from '{}'. It should be a 32-character \
                    lowercase hexadecimal string.",
                    trimmed,
                    path.as_ref().display()
                )
            },
        )?))
    }

    pub fn boot_id() -> Result<String, Error> {
        let raw = fs::read_to_string(BOOT_ID_FILE)
            .with_context(|| format!("Failed to read boot ID from '{BOOT_ID_FILE}'"))?;
        let boot_id = raw.trim();
        ensure!(!boot_id.is_empty(), "Boot ID in '{BOOT_ID_FILE}' was empty");
        Ok(boot_id.to_string())
    }

    pub fn as_bytes(&self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    pub fn as_u128(&self) -> u128 {
        self.0
    }

    pub fn as_uuid(&self) -> Uuid {
        Uuid::from_u128(self.0)
    }

    pub fn as_string(&self) -> String {
        format!("{:032x}", self.0)
    }

    pub fn hashed(&self) -> [u8; 16] {
        let bytes: [u8; 48] = Sha384::digest(self.as_bytes()).into();
        let mut result = [0; 16];
        result.copy_from_slice(&bytes[0..16]);
        result
    }

    pub fn hashed_uuid(&self) -> Uuid {
        Uuid::from_bytes(self.hashed())
    }
}

pub fn boot_id() -> Result<String, Error> {
    MachineId::boot_id()
}

/// Returns the current boot's start time as Unix epoch seconds, read from
/// `/proc/stat`'s `btime` line - the same absolute wall-clock boot
/// timestamp `systemctl show -p KernelTimestamp` exposes, but read directly
/// with no subprocess/systemd dependency, matching this module's existing
/// style of reading `/proc` files directly (see `boot_id` above). Useful
/// for determining whether a reboot has happened since some earlier
/// wall-clock timestamp, without needing any local state persisted across
/// that reboot: unlike `boot_id`, which only distinguishes "this boot" from
/// "some other boot" (and needs a previously-recorded boot ID to compare
/// against), this can be compared directly against an absolute timestamp
/// recorded anywhere - including one that only survives in an external
/// system (e.g. a Kubernetes annotation), not on local disk.
pub fn boot_time() -> Result<i64, Error> {
    boot_time_inner(PROC_STAT_FILE)
}

fn boot_time_inner(path: impl AsRef<Path>) -> Result<i64, Error> {
    let path = path.as_ref();
    let contents = fs::read_to_string(path)
        .with_context(|| format!("Failed to read boot time from '{}'", path.display()))?;
    let line = contents
        .lines()
        .find(|line| line.starts_with("btime "))
        .with_context(|| format!("No 'btime' line found in '{}'", path.display()))?;
    let raw = line
        .split_whitespace()
        .nth(1)
        .with_context(|| format!("Malformed 'btime' line in '{}': {line:?}", path.display()))?;
    raw.parse::<i64>().with_context(|| {
        format!(
            "Failed to parse boot time {raw:?} from '{}'",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_time_inner_parses_btime_line() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("stat");
        std::fs::write(&path, "cpu  1 2 3 4\nbtime 1700000000\nprocesses 100\n")
            .expect("failed to write test file");

        assert_eq!(boot_time_inner(&path).expect("should parse"), 1_700_000_000);
    }

    #[test]
    fn boot_time_inner_errors_when_btime_missing() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("stat");
        std::fs::write(&path, "cpu  1 2 3 4\nprocesses 100\n").expect("failed to write test file");

        assert!(boot_time_inner(&path).is_err());
    }

    #[test]
    fn boot_time_inner_errors_for_missing_file() {
        assert!(boot_time_inner("/nonexistent/proc-stat-for-test").is_err());
    }
}
