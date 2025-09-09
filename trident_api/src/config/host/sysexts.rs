use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Sysexts {
    pub add: Vec<Sysext>,
    pub remove: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Sysext {
    pub url: Url,
}
