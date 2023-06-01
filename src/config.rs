use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct ConfigFile {
    pub listen_port: Option<u16>,
}
