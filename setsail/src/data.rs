use std::collections::HashMap;

use crate::{commands as cmd, sections::script::Script};

/// Struct to hold all meaningful data parsed from a kickstart file
#[derive(Debug, Default)]
pub struct ParsedData {
    pub scripts: Vec<Script>,
    pub partitions: Vec<cmd::partition::Partition>,
    pub users: HashMap<String, cmd::user::User>,
    pub root: Option<cmd::rootpw::Rootpw>,
    pub netdevs: HashMap<cmd::network::UniqueDeviceReference, cmd::network::Network>,
    pub hostname: Option<cmd::network::Hostname>,
}
