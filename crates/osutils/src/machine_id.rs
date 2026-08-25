use std::{fs, path::Path};

use anyhow::{ensure, Context, Error};
use sha2::{Digest, Sha384};
use uuid::Uuid;

const MACHINE_ID_FILE: &str = "/etc/machine-id";
const BOOT_ID_FILE: &str = "/proc/sys/kernel/random/boot_id";

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
