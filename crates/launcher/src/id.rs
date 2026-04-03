use std::fmt::Display;

use osutils::{hostname, machine_id::MachineId};

use crate::error::HarpoonError;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum IdSource {
    MachineIdHashed,
    MachineIdRaw,
    Hostname,
}

impl Display for IdSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdSource::MachineIdHashed => write!(f, "machin-id-hashed"),
            IdSource::MachineIdRaw => write!(f, "machine-id-raw"),
            IdSource::Hostname => write!(f, "hostname"),
        }
    }
}

impl IdSource {
    pub(super) fn produce_id(&self) -> Result<String, HarpoonError> {
        Ok(match self {
            IdSource::MachineIdHashed => MachineId::read()
                .map_err(|err| HarpoonError::MachineIdRead(err.to_string()))?
                .hashed_uuid()
                .to_string(),
            IdSource::MachineIdRaw => MachineId::read()
                .map_err(|err| HarpoonError::MachineIdRead(err.to_string()))?
                .as_string(),
            IdSource::Hostname => {
                hostname::read().map_err(|err| HarpoonError::HostnameRead(err.to_string()))?
            }
        })
    }
}
