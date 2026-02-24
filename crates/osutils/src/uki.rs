use std::path::{Path, PathBuf};

/// UKI Addon directory suffix.
pub const UKI_ADDON_DIR_SUFFIX: &str = ".extra.d";
/// UKI Addon file suffix.
pub const UKI_ADDON_FILE_SUFFIX: &str = ".addon.efi";

/// Returns the path to the addon directory associated with the given UKI file,
/// which is expected to be named `<UKI_filename>.extra.d/`. For example, if the
/// UKI file is `vmlinuz-1-azla1.efi`, the associated addon directory would be
/// `vmlinuz-1-azla1.efi.extra.d/` in the same directory as the UKI file.
pub fn uki_addon_dir(uki_path: &Path) -> PathBuf {
    let mut addon_dir = uki_path.to_path_buf().into_os_string();
    addon_dir.push(UKI_ADDON_DIR_SUFFIX);
    PathBuf::from(addon_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validates that `uki_addon_dir` appends the `.extra.d` suffix to the
    /// UKI file path to form the addon directory path.
    #[test]
    fn test_uki_addon_dir() {
        let uki_path = PathBuf::from("/some/path/vmlinuz-1-azla1.efi");
        let expected_addon_dir = PathBuf::from("/some/path/vmlinuz-1-azla1.efi.extra.d");
        assert_eq!(uki_addon_dir(&uki_path), expected_addon_dir);
    }
}
