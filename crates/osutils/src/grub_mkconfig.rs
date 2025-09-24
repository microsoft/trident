use std::{
    ffi::OsString,
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{debug, trace};

use trident_api::constants::ROOT_MOUNT_POINT_PATH;

use crate::{files, path};

/// Common location for all grub-mkconfig scripts.
pub const GRUB_MKCONFIG_SCRIPT_CONF_DIR: &str = "/etc/default/grub.d";

/// A new grub-mkconfig script.
#[derive(Debug, Default)]
pub struct GrubMkConfigScript {
    /// The script name.
    name: String,

    /// Path to the root directory where the script should be written.
    root: PathBuf,

    /// New kernel command line parameters that should be appended to the
    /// GRUB_CMDLINE_LINUX variable.
    new_params: Vec<(OsString, Option<OsString>)>,

    /// Boot device to use for the GRUB_DEVICE variable.
    boot_device: Option<OsString>,
}

impl GrubMkConfigScript {
    /// Creates a new grub-mkconfig script at GRUB_MKCONFIG_SCRIPT_CONF_DIR
    /// where `name` will be the name of the script without the file extension.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::path::Path;
    ///
    /// use osutils::grub_mkconfig::GrubMkConfigScript;
    ///
    /// let mut script = GrubMkConfigScript::new("50_my_script");
    ///
    /// assert_eq!(script.file_path(), Path::new("/etc/default/grub.d/50_my_script.cfg"));
    /// ```
    ///
    /// This will create a new grub-mkconfig script at
    /// `/etc/default/grub.d/50_my_script.cfg`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            root: PathBuf::from(ROOT_MOUNT_POINT_PATH),
            ..Default::default()
        }
    }

    /// Sets the root directory where the grub-mkconfig script should be
    /// written.
    ///
    /// # Example
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use osutils::grub_mkconfig::GrubMkConfigScript;
    ///
    /// let script = GrubMkConfigScript::new("50_my_script").with_root("/my/path");
    ///
    /// assert_eq!(script.file_path(), Path::new("/my/path/etc/default/grub.d/50_my_script.cfg"));
    /// ```
    pub fn with_root(self, root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ..self
        }
    }

    /// Appends a new simple parameter to the GRUB_CMDLINE_LINUX variable.
    pub fn add_simple_param(&mut self, key: impl Into<OsString>) {
        self.new_params.push((key.into(), None));
    }

    /// Appends a new key-value parameter to the GRUB_CMDLINE_LINUX variable.
    pub fn add_kv_param(&mut self, key: impl Into<OsString>, value: impl Into<OsString>) {
        self.new_params.push((key.into(), Some(value.into())));
    }

    /// Sets the boot device to use for the GRUB_DEVICE variable.
    pub fn set_boot_device(&mut self, boot_device: impl Into<OsString>) {
        self.boot_device = Some(boot_device.into());
    }

    /// Writes the grub-mkconfig script to the filesystem.
    pub fn write(&self) -> Result<(), Error> {
        self.write_inner(&self.file_path())
    }

    /// Internal implementation of `write` that allows specifying the path.
    fn write_inner(&self, path: &Path) -> Result<(), Error> {
        debug!("Writing grub-mkconfig script to '{}'", path.display());

        let content = self.render();
        trace!(
            "Grub-mkconfig script content:\n{}",
            content.to_string_lossy()
        );

        let mut file = files::create_file(path).with_context(|| {
            format!(
                "Failed to create grub-mkconfig script at '{}'",
                path.display()
            )
        })?;

        file.write_all(content.as_bytes()).with_context(|| {
            format!(
                "Failed to write grub-mkconfig script to '{}'",
                path.display()
            )
        })
    }

    /// Returns the path to the grub-mkconfig script.
    pub fn file_path(&self) -> PathBuf {
        path::join_relative(
            &self.root,
            Path::new(GRUB_MKCONFIG_SCRIPT_CONF_DIR)
                .join(&self.name)
                .with_extension("cfg"),
        )
    }

    /// Renders the grub-mkconfig script content.
    fn render(&self) -> OsString {
        let mut conf = OsString::new();

        // Append the new parameters to the GRUB_CMDLINE_LINUX variable.
        if !self.new_params.is_empty() {
            conf.push("GRUB_CMDLINE_LINUX=\"$GRUB_CMDLINE_LINUX ");
            let mut params = self.new_params.iter().peekable();
            while let Some((key, value)) = params.next() {
                conf.push(key);
                if let Some(value) = value {
                    conf.push("=");
                    if value.as_bytes().contains(&b' ') {
                        conf.push(r#"\""#);
                        conf.push(value);
                        conf.push(r#"\""#);
                    } else {
                        conf.push(value);
                    }
                }

                // Add a space between parameters.
                if params.peek().is_some() {
                    conf.push(" ");
                }
            }

            conf.push("\"\n");
        }

        // Set the boot device.
        if let Some(boot_device) = &self.boot_device {
            conf.push("GRUB_DEVICE=\"");
            conf.push(boot_device);
            conf.push("\"\n");
        }

        conf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grub_mkconfig_script() {
        let mut script = GrubMkConfigScript::new("50_my_script");
        script.add_simple_param("my_param");
        script.add_kv_param("my_key", "my_value");
        script.set_boot_device("/dev/sda");

        let content = script.render();
        assert_eq!(
            content,
            OsString::from(indoc::indoc! {
                r#"
                GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX my_param my_key=my_value"
                GRUB_DEVICE="/dev/sda"
                "#
            })
        );
    }

    #[test]
    fn test_only_boot_device() {
        let mut script = GrubMkConfigScript::new("50_my_script");
        script.set_boot_device("/dev/sda");

        let content = script.render();
        assert_eq!(
            content,
            OsString::from(indoc::indoc! {
                r#"
                GRUB_DEVICE="/dev/sda"
                "#
            })
        );
    }

    #[test]
    fn test_only_kernel_params() {
        let mut script = GrubMkConfigScript::new("50_my_script");
        script.add_simple_param("my_param");
        script.add_kv_param("my_key", "my_value");

        let content = script.render();
        assert_eq!(
            content,
            OsString::from(indoc::indoc! {
                r#"
                GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX my_param my_key=my_value"
                "#
            })
        );
    }

    #[test]
    fn test_no_params() {
        let script = GrubMkConfigScript::new("50_my_script");

        let content = script.render();
        assert_eq!(content, OsString::new());
    }

    #[test]
    fn test_value_with_space() {
        let mut script = GrubMkConfigScript::new("50_my_script");
        script.add_kv_param("my_key", "my value");

        let content = script.render();
        assert_eq!(
            content,
            OsString::from(indoc::indoc! {
                r#"
                GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX my_key=\"my value\""
                "#
            })
        );
    }

    #[test]
    fn test_name() {
        let script = GrubMkConfigScript::new("50_my_script");
        assert_eq!(
            script.file_path(),
            Path::new(GRUB_MKCONFIG_SCRIPT_CONF_DIR).join("50_my_script.cfg")
        );
    }

    #[test]
    fn test_write() {
        let mut script = GrubMkConfigScript::new("50_my_script");
        script.add_simple_param("my_param");

        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_file = tmp_dir.path().join("50_my_script.cfg");

        assert!(!tmp_file.exists());

        script.write_inner(&tmp_file).unwrap();

        assert!(tmp_file.exists());

        let content = std::fs::read_to_string(&tmp_file).unwrap();

        assert_eq!(
            content,
            indoc::indoc! {
                r#"
                GRUB_CMDLINE_LINUX="$GRUB_CMDLINE_LINUX my_param"
                "#
            }
        );
    }
}
