use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

pub(super) mod error;
pub(super) mod os;
pub(super) mod scripts;
pub(super) mod storage;
pub(super) mod trident;

use os::Os;
use scripts::Scripts;
use storage::Storage;
use trident::Trident;

use error::InvalidHostConfigurationError;

use self::os::ManagementOs;

/// HostConfiguration is the configuration for a host. Trident agent will use this to configure the host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct HostConfiguration {
    /// The Trident Management configuration controls the installation of the
    /// Trident agent onto the runtime OS.
    #[serde(default)]
    pub trident: Trident,

    /// Describes the storage configuration of the host.
    #[serde(default)]
    pub storage: Storage,

    /// Optional scripts to be run after different Trident stages have completed.
    #[serde(default, skip_serializing_if = "is_default")]
    pub scripts: Scripts,

    /// OS Configuration for the runtime OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub os: Os,

    /// OS Configuration for the management OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub management_os: ManagementOs,
}

impl HostConfiguration {
    pub fn validate(&self) -> Result<(), InvalidHostConfigurationError> {
        let require_root_mount_point = self.trident != Trident::default()
            || self.scripts != Scripts::default()
            || self.os != Os::default()
            || self.os.network.is_some();
        self.storage.validate(require_root_mount_point)?;
        self.os.validate()?;
        self.scripts.validate()?;
        self.management_os.validate()?;

        Ok(())
    }

    #[cfg(feature = "schemars")]
    pub fn generate_schema() -> schemars::schema::RootSchema {
        use schemars::schema::Schema;
        let mut schema =
            crate::schema_helpers::schema_generator().into_root_schema_for::<HostConfiguration>();

        // Because netplan-types currently does not support schemars, we have to
        // manually provide a placeholder schema for the netplan fields using
        // `schemars(with = "...")`. These are Option<> fields, but overriding
        // schematization using `with` removes this behavior. (is_option is a
        // "private" function in the JsonSchema trait) This means we have to
        // manually edit the schema to remove these two fields from the required
        // list.
        let remove_network = |schema: &mut schemars::schema::RootSchema, key: &str| {
            if let Some(Schema::Object(obj)) = schema.definitions.get_mut(key) {
                obj.object().required.remove("network");
            } else {
                panic!(
                    "Failed to remove 'network' from required fields from definition '{}'. Perhaps the API has changed?",
                    key
                );
            }
        };

        remove_network(&mut schema, "Os");
        remove_network(&mut schema, "ManagementOs");

        schema
    }
}
