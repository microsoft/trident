use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Module {
    /// Name of the module.
    pub name: String,

    /// Load mode of the kernel module.
    ///
    /// The load mode setting for kernel modules dictates how and when these modules are
    /// loaded or disabled in the system.
    #[serde(default)]
    pub load_mode: LoadMode,

    /// Kernel options.
    ///
    /// Kernel options for modules can specify how these modules interact with the system,
    /// and adjust performance or security settings specific to each module.
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// Load mode of the kernel module.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum LoadMode {
    /// # Always
    ///
    /// Set kernel modules to be loaded automatically at boot time.
    Always,

    /// # Auto
    ///
    /// Used for modules that are automatically loaded by the kernel as needed, without
    /// explicit configuration to load them at boot.
    Auto,

    /// # Disable
    ///
    /// Configures kernel modules to be explicitly disabled, preventing them from loading
    /// automatically.
    Disable,

    /// # Inherit
    ///
    /// Configures kernel modules to inherit the loading behavior set in the base image.
    /// Only applying new options where they are explicitly provided and applicable.
    #[default]
    Inherit,
}
