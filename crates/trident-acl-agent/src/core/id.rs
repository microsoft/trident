use std::fmt::Display;

use osutils::{hostname, machine_id::MachineId};

use crate::core::error::AgentError;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum IdSource {
    MachineIdHashed,
    MachineIdRaw,
    Hostname,
}

impl Display for IdSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdSource::MachineIdHashed => write!(f, "machine-id-hashed"),
            IdSource::MachineIdRaw => write!(f, "machine-id-raw"),
            IdSource::Hostname => write!(f, "hostname"),
        }
    }
}

impl IdSource {
    pub(crate) fn produce_id(&self) -> Result<String, AgentError> {
        Ok(match self {
            IdSource::MachineIdHashed => MachineId::read()
                .map_err(|err| AgentError::MachineIdRead(err.to_string()))?
                .hashed_uuid()
                .to_string(),
            IdSource::MachineIdRaw => MachineId::read()
                .map_err(|err| AgentError::MachineIdRead(err.to_string()))?
                .as_string(),
            IdSource::Hostname => {
                hostname::read().map_err(|err| AgentError::HostnameRead(err.to_string()))?
            }
        })
    }
}
