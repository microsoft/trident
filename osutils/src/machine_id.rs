use std::path::Path;

use anyhow::{Context, Error};
use sha2::{Digest, Sha384};
use uuid::Uuid;

const MACHINE_ID_FILE: &str = "/etc/machine-id";

#[derive(Debug, Clone, Copy)]
pub struct MachineId(u128);

impl MachineId {
    pub fn read() -> Result<Self, Error> {
        Self::read_inner(MACHINE_ID_FILE)
    }

    fn read_inner(path: impl AsRef<Path>) -> Result<Self, Error> {
        let id = std::fs::read_to_string(MACHINE_ID_FILE).with_context(|| {
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
