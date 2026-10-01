//! Built-in defaults for the Image Customizer (IC) toolchain — the single, obvious source of truth
//! for *which Image Customizer tailor runs by default*.
//!
//! Change the constants below to update the default IC image/version everywhere it matters:
//! - [`DEFAULT_IC_CONTAINER`] — the image used in standalone mode (no `tailor.yaml`).
//! - [`DEFAULT_IC_TAG`] — the tag pulled when a toolchain pins neither a `tag` nor a `version`
//!   (see [`ToolchainEntry::effective_tag`](crate::ToolchainEntry::effective_tag)).

use crate::schema::{PullPolicy, ToolConfig, ToolchainEntry, Toolchains};

/// The default Image Customizer container image, used in standalone mode when there is no
/// `tailor.yaml` pinning a toolchain.
pub const DEFAULT_IC_CONTAINER: &str = "mcr.microsoft.com/azurelinux/imagecustomizer";

/// The registry tag pulled when a toolchain pins neither an explicit `tag` nor a `version`.
/// MCR publishes unprefixed tags (e.g. `1.3.0`, `latest`).
pub const DEFAULT_IC_TAG: &str = "latest";

/// The default ownership-janitor image (`meta/docs/2026-06-22-design.md` §7.7) — a minimal OS image with
/// `/bin/chown` and `/bin/rm`, used to normalize IC's root-owned outputs sudo-free when the manifest
/// sets no `runtime.janitorImage`.
pub const DEFAULT_JANITOR_CONTAINER: &str = "mcr.microsoft.com/azurelinux/base/core";

/// The tag pulled for [`DEFAULT_JANITOR_CONTAINER`].
pub const DEFAULT_JANITOR_TAG: &str = "3.0";

/// The directory (relative to the workspace root) where registry base images are cached when the
/// manifest sets no `runtime.imageCacheDir`. IC requires a cache dir for `oci`/`azureLinux` bases.
pub const DEFAULT_IMAGE_CACHE_DIR: &str = ".tailor/cache";

/// The name of the built-in toolchain entry created in standalone mode.
const DEFAULT_TOOLCHAIN_ID: &str = "ic";

/// The toolchain configuration used in standalone mode (no `tailor.yaml`): a single entry pointing
/// at [`DEFAULT_IC_CONTAINER`] with no pinned tag/version, so it resolves to [`DEFAULT_IC_TAG`].
#[must_use]
pub fn default_tool_config() -> ToolConfig {
    let entries = vec![ToolchainEntry {
        name: DEFAULT_TOOLCHAIN_ID.to_owned(),
        container: DEFAULT_IC_CONTAINER.to_owned(),
        version: None,
        tag: None,
        pull: PullPolicy::Missing,
    }];
    ToolConfig {
        schema_version: 1,
        preview_features: Vec::new(),
        toolchains: Toolchains {
            default: DEFAULT_TOOLCHAIN_ID.to_owned(),
            entries,
        },
        tools_dir_sources: Vec::new(),
        runtime: None,
        defaults: None,
        signing: None,
        images: None,
        base_images: None,
        export: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_default_is_latest_image_customizer() {
        let tc = default_tool_config();
        let entry = tc.toolchains.get(&tc.toolchains.default).unwrap();
        assert_eq!(entry.container, DEFAULT_IC_CONTAINER);
        assert_eq!(entry.effective_tag(), DEFAULT_IC_TAG);
        assert_eq!(entry.effective_tag(), "latest");
    }
}
