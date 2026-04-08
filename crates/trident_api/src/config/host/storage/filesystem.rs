use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    constants::{MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    BlockDeviceId,
};

use super::filesystem_types::{AdoptedFileSystemType, FileSystemType, NewFileSystemType};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(
    try_from = "fs_serde::FileSystemSerde",
    into = "fs_serde::FileSystemSerde"
)]
pub struct FileSystem {
    /// The ID of the block device on which to place this file system.
    pub device_id: Option<BlockDeviceId>,

    /// The source of the file system.
    ///
    /// If not specified, this field will default to image.
    pub source: FileSystemSource,

    /// The mount point of the file system.
    ///
    /// It can be provided as an object for more control over the mount options,
    /// or as a just a string when `defaults` is sufficient.
    pub mount_point: Option<MountPoint>,

    ///Whether this filesystem is the ESP.
    pub is_esp: bool,
}

pub mod fs_serde {
    use anyhow::{ensure, Context, Error};
    use serde::{Deserialize, Serialize};

    #[cfg(feature = "schemars")]
    use schemars::JsonSchema;

    use crate::{constants::ESP_MOUNT_POINT_PATH, is_default};

    #[cfg(feature = "schemars")]
    use crate::schema_helpers::block_device_id_schema;

    use super::{
        AdoptedFileSystemType, FileSystem, FileSystemSource, FileSystemType, MountPoint,
        NewFileSystemType,
    };

    const DEFAULT_ESP_MOUNT_PATH: &str = ESP_MOUNT_POINT_PATH;

    #[derive(Deserialize, Serialize, Default, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case", deny_unknown_fields)]
    #[cfg_attr(feature = "schemars", derive(JsonSchema))]
    enum FileSystemSourceSerde {
        /// # New
        ///
        /// Create a new file system.
        New,

        /// # Adopted
        ///
        /// Use an existing file system from an adopted partition.
        Adopted,

        /// # Image
        ///
        /// Use an existing file system from an image.
        #[default]
        Image,
    }

    #[derive(Deserialize, Serialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    #[cfg_attr(feature = "schemars", derive(JsonSchema))]
    pub(super) struct FileSystemSerde {
        /// The ID of the block device on which to place this file system.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
        device_id: Option<String>,

        /// The source of the file system.
        ///
        /// If not specified, this field will default to image.
        #[serde(default, skip_serializing_if = "is_default")]
        source: FileSystemSourceSerde,

        /// The type of the file system.
        ///
        /// File system type must *not* be specified if the source of the file
        /// system is `image`.
        #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
        fs_type: Option<FileSystemType>,

        /// The mount point of the file system.
        ///
        /// It can be provided as an object for more control over the mount options,
        /// or as a just a string when `defaults` is sufficient.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::primitives::shortcuts::opt_string_or_struct"
        )]
        #[cfg_attr(
            feature = "schemars",
            schemars(
                schema_with = "crate::primitives::shortcuts::opt_string_or_struct_schema::<MountPoint>"
            )
        )]
        mount_point: Option<MountPoint>,

        /// Options to change the default ESP mount point path.
        #[serde(default, skip_serializing_if = "is_default")]
        override_esp_mount: OverrideEspMount,
    }

    #[derive(Default, Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case", deny_unknown_fields)]
    #[cfg_attr(feature = "schemars", derive(JsonSchema))]
    enum OverrideEspMount {
        /// # Use Default Behavior
        ///
        /// Do not override the default ESP mount point path. This is the
        /// default behavior.
        #[default]
        UseDefault,

        /// # Override
        ///
        /// Override the default ESP mount point to be the path of this
        /// filesystem.
        Override,

        /// # Block
        ///
        /// This option should be used very rarely and in very specific
        /// non-standard scenarios.
        ///
        /// Used to indicate that this filesystem is NOT the ESP, even if it
        /// matches the default ESP mount point path. This is necessary in the
        /// case where a user has a non-ESP filesystem that they want to mount
        /// at the default ESP mount point path, and they want to ensure that
        /// Trident does not treat it as the ESP.
        Block,
    }

    impl TryFrom<FileSystemSerde> for FileSystem {
        type Error = Error;

        fn try_from(value: FileSystemSerde) -> Result<super::FileSystem, Self::Error> {
            let source = match value.source {
                FileSystemSourceSerde::New => FileSystemSource::New(match value.fs_type {
                    None => NewFileSystemType::default(),
                    Some(fs_type) => NewFileSystemType::try_from(fs_type)
                        .context("Invalid new filesystem type")?,
                }),
                FileSystemSourceSerde::Adopted => FileSystemSource::Adopted(match value.fs_type {
                    None => AdoptedFileSystemType::default(),
                    Some(fs_type) => AdoptedFileSystemType::try_from(fs_type)
                        .context("Invalid adopted filesystem type")?,
                }),
                FileSystemSourceSerde::Image => {
                    ensure!(
                        value.fs_type.is_none(),
                        "Filesystem type cannot be specified for image filesystems"
                    );
                    FileSystemSource::Image
                }
            };

            let is_esp = match value.override_esp_mount {
                OverrideEspMount::UseDefault => value
                    .mount_point
                    .as_ref()
                    .is_some_and(|mp| mp.path.eq(DEFAULT_ESP_MOUNT_PATH)),
                OverrideEspMount::Override => {
                    ensure!(
                        value.mount_point.is_some(),
                        "override_esp_mount cannot be set to Override when mount_point is not specified"
                    );
                    true
                }
                OverrideEspMount::Block => false,
            };

            Ok(FileSystem {
                device_id: value.device_id,
                source,
                mount_point: value.mount_point,
                is_esp,
            })
        }
    }

    impl From<FileSystem> for FileSystemSerde {
        fn from(value: FileSystem) -> Self {
            let override_esp_mount = if let Some(mp) = &value.mount_point {
                // There is a mount point, so the override_esp_mount field has
                // meaning. We determine its value based on whether the mount
                // point path matches the default ESP mount point path and
                // whether the is_esp field is set to true.

                match (mp.path.eq(DEFAULT_ESP_MOUNT_PATH), value.is_esp) {
                    // Mount point matches default ESP mount point path and
                    // is_esp is true, so we use the default behavior.
                    (true, true) => OverrideEspMount::UseDefault,

                    // Mount point matches default ESP mount point path but
                    // is_esp is false, so we block it from being treated as the
                    // ESP.
                    (true, false) => OverrideEspMount::Block,

                    // Mount point does not match default ESP mount point path
                    // but is_esp is true, so we override the default ESP mount
                    // point to be this mount point.
                    (false, true) => OverrideEspMount::Override,

                    // Mount point does not match default ESP mount point path and
                    // is_esp is false, so we use the default behavior.
                    (false, false) => OverrideEspMount::UseDefault,
                }
            } else {
                // If there is no mount point, then the override_esp_mount field
                // has no meaning, so we can just set it to UseDefault.
                OverrideEspMount::UseDefault
            };

            FileSystemSerde {
                device_id: value.device_id,
                source: match &value.source {
                    FileSystemSource::Image => FileSystemSourceSerde::Image,
                    FileSystemSource::New(_) => FileSystemSourceSerde::New,
                    FileSystemSource::Adopted(_) => FileSystemSourceSerde::Adopted,
                },
                fs_type: match &value.source {
                    FileSystemSource::New(fs_type) => Some((*fs_type).into()),
                    FileSystemSource::Adopted(fs_type) => Some((*fs_type).into()),
                    _ => None,
                },
                mount_point: value.mount_point,
                override_esp_mount,
            }
        }
    }

    #[cfg(feature = "schemars")]
    impl JsonSchema for super::FileSystem {
        fn schema_name() -> String {
            "FileSystem".to_string()
        }

        fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
            FileSystemSerde::json_schema(gen)
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FileSystemSource {
    /// # New
    ///
    /// Create a new file system.
    New(NewFileSystemType),

    /// # Adopted
    ///
    /// Use an existing file system from an adopted partition.
    Adopted(AdoptedFileSystemType),

    /// # Image
    ///
    /// Use an existing file system from an image.
    #[default]
    Image,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountPoint {
    /// The path to mount the file system.
    pub path: PathBuf,

    /// The mount options.
    #[serde(default)]
    pub options: MountOptions,
}

impl<T> From<T> for MountPoint
where
    T: Into<PathBuf>,
{
    fn from(value: T) -> Self {
        MountPoint {
            path: value.into(),
            options: MountOptions::defaults(),
        }
    }
}

impl FromStr for MountPoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(s.into())
    }
}

#[cfg(feature = "schemars")]
impl crate::primitives::shortcuts::StringOrStructMetadata for MountPoint {
    fn shorthand_format() -> &'static str {
        "path"
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(transparent)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountOptions(pub String);

impl MountOptions {
    pub fn new(options: impl Into<String>) -> Self {
        MountOptions(options.into())
    }

    pub fn defaults() -> Self {
        MountOptions("defaults".to_string())
    }

    pub fn empty() -> Self {
        MountOptions("".to_string())
    }

    pub fn contains(&self, option: impl AsRef<str>) -> bool {
        self.0.split(',').any(|o| o == option.as_ref())
    }

    pub fn str(&self) -> &str {
        &self.0
    }

    pub fn with(mut self, option: impl Into<String>) -> Self {
        self.append(option);
        self
    }

    pub fn append(&mut self, option: impl Into<String>) {
        if self.0.is_empty() {
            self.0 = option.into();
        } else {
            self.0.push(',');
            self.0.push_str(&option.into());
        }
    }

    pub fn to_str_vec(&self) -> Vec<&str> {
        self.0.split(',').filter(|s| !s.trim().is_empty()).collect()
    }

    pub fn to_string_vec(&self) -> Vec<String> {
        self.0
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

impl Default for MountOptions {
    fn default() -> Self {
        MountOptions::defaults()
    }
}

impl<T> From<T> for MountOptions
where
    T: Into<String>,
{
    fn from(options: T) -> Self {
        MountOptions::new(options)
    }
}

/// Helper struct to communicate information about a mount point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPointInfo<'a> {
    pub mount_point: &'a MountPoint,
    pub device_id: Option<&'a BlockDeviceId>,
}

impl FileSystem {
    /// Because filesystems don't have IDs that can uniquely identify them, this
    /// function can be used to create a description of the specific filesystem
    /// in lieu of an ID.
    pub fn description(&self) -> String {
        [
            (
                "src",
                Some(
                    match &self.source {
                        FileSystemSource::New(_) => "new",
                        FileSystemSource::Adopted(_) => "adopted",
                        FileSystemSource::Image => "image",
                    }
                    .to_owned(),
                ),
            ),
            (
                "type",
                match &self.source {
                    FileSystemSource::New(fs_type) => Some(fs_type.to_string()),
                    FileSystemSource::Adopted(fs_type) => Some(fs_type.to_string()),
                    FileSystemSource::Image => None,
                },
            ),
            ("dev", self.device_id.clone()),
            (
                "mnt",
                self.mount_point
                    .as_ref()
                    .map(|mp| mp.path.to_string_lossy().to_string()),
            ),
            ("is_esp", self.is_esp.then_some("true".to_owned())),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{k}:{v}")))
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// Returns whether the filesystem is the root filesystem, as determined by
    /// its mount point path.
    pub fn is_root(&self) -> bool {
        self.mount_point_path()
            .is_some_and(|mpp| mpp == Path::new(ROOT_MOUNT_POINT_PATH))
    }

    /// Returns whether the filesystem's mount options include the `ro` option.
    pub fn is_read_only(&self) -> bool {
        self.mount_point
            .as_ref()
            .is_some_and(|mp| mp.options.contains(MOUNT_OPTION_READ_ONLY))
    }

    /// Returns the path of the mount point, if it exists.
    pub fn mount_point_path(&self) -> Option<&Path> {
        self.mount_point.as_ref().map(|mp| mp.path.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::constants::ESP_MOUNT_POINT_PATH;

    #[test]
    fn test_mount_point_from_str() {
        let mount_point: MountPoint = "/mnt".into();
        assert_eq!(mount_point.path, PathBuf::from("/mnt"));
        assert_eq!(mount_point.options, MountOptions::defaults());
    }

    #[test]
    fn test_filesystem_mount_point_path() {
        let mut fs = FileSystem {
            device_id: Some("device_id".to_string()),
            source: Default::default(),
            mount_point: None,
            is_esp: false,
        };
        assert_eq!(fs.mount_point_path(), None);

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/mnt"),
            options: MountOptions::new("defaults"),
        });
        assert_eq!(fs.mount_point_path(), Some(Path::new("/mnt")));
        assert!(!fs.is_esp);
        assert!(!fs.is_root());
        assert!(!fs.is_read_only());

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/boot/efi"),
            options: MountOptions::new("defaults"),
        });
        fs.is_esp = true; // Manually set is_esp to true since we're not using the HostStorageConfig's esp_mount_path in this test.
        assert_eq!(fs.mount_point_path(), Some(Path::new(ESP_MOUNT_POINT_PATH)));
        assert!(fs.is_esp);
        assert!(!fs.is_root());
        assert!(!fs.is_read_only());

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/"),
            options: MountOptions::new("defaults"),
        });
        fs.is_esp = false; // Manually set is_esp to false since the mount point is now the root mount point, not the ESP mount point.
        assert_eq!(
            fs.mount_point_path(),
            Some(Path::new(ROOT_MOUNT_POINT_PATH))
        );
        assert!(!fs.is_esp);
        assert!(fs.is_root());
        assert!(!fs.is_read_only());

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/mnt"),
            options: MountOptions::new("ro"),
        });
        assert_eq!(fs.mount_point_path(), Some(Path::new("/mnt")));
        assert!(!fs.is_esp);
        assert!(!fs.is_root());
        assert!(fs.is_read_only());
    }

    #[test]
    fn test_deserialize_filesystem_image() {
        let root_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::Image,
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
            is_esp: false,
        };

        // Success: source unspecified
        let yaml = indoc::indoc! {r#"
            deviceId: root
            mountPoint: /
        "#};
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, root_fs);

        // Success: source specified
        let yaml = indoc::indoc! {r#"
            deviceId: root
            source: image
            mountPoint: /
        "#};
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, root_fs);

        // Failure: filesystem type specified
        let yaml = indoc::indoc! {r#"
            deviceId: root
            source: image
            type: ext4
            mountPoint: /
        "#};
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err
            .to_string()
            .contains("Filesystem type cannot be specified for image filesystems"));
    }

    #[test]
    fn test_deserialize_filesystem_new() {
        let new_fs = FileSystem {
            device_id: Some("trident".into()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
            is_esp: false,
        };

        // Success: filesystem type unspecified (default to Ext4)
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: new
            mountPoint: /
        "#};
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, new_fs);

        // Success: filesystem type specified
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: new
            type: ext4
            mountPoint: /
        "#};
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, new_fs);

        // Failure: invalid filesystem type specified (unknown)
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: new
            type: abcd
            mountPoint: /
        "#};
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown variant `abcd`"));

        // Failure: invalid filesystem type specified (adopted)
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: new
            type: iso9660
            mountPoint: /
        "#};
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("Invalid new filesystem type"));
    }

    #[test]
    fn test_deserialize_filesystem_adopted() {
        let adopted_fs = FileSystem {
            device_id: Some("trident".into()),
            source: FileSystemSource::Adopted(AdoptedFileSystemType::Auto),
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
            is_esp: false,
        };

        // Success: filesystem type unspecified (default to Auto)
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: adopted
            mountPoint: /
        "#};
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, adopted_fs);

        // Success: filesystem type specified
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: adopted
            type: auto
            mountPoint: /
        "#};
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, adopted_fs);

        // Failure: invalid filesystem type specified (unknown)
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: adopted
            type: abcd
            mountPoint: /
        "#};
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown variant `abcd`"));

        // Failure: invalid filesystem type specified (new)
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source: adopted
            type: tmpfs
            mountPoint: /
        "#};
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("Invalid adopted filesystem type"));

        // Failure: invalid nesting structure
        let yaml = indoc::indoc! {r#"
            deviceId: trident
            source:
              source: adopted
              type: auto
            mountPoint: /
        "#};
        serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
    }

    #[test]
    fn test_serialize_filesystem() {
        // Image
        let image_yaml = indoc::indoc! {r#"
            deviceId: root
            mountPoint:
              path: /
              options: defaults
        "#};
        let image_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::Image,
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
            is_esp: false,
        };
        assert_eq!(serde_yaml::to_string(&image_fs).unwrap(), image_yaml);

        // New
        let new_yaml = indoc::indoc! {r#"
            deviceId: root
            source: new
            type: ext4
            mountPoint:
              path: /
              options: defaults
        "#};
        let new_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
            is_esp: false,
        };
        assert_eq!(serde_yaml::to_string(&new_fs).unwrap(), new_yaml);

        // Adopted
        let adopted_yaml = indoc::indoc! {r#"
            deviceId: root
            source: adopted
            type: auto
            mountPoint:
              path: /
              options: defaults
        "#};
        let adopted_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::Adopted(AdoptedFileSystemType::Auto),
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
            is_esp: false,
        };
        assert_eq!(serde_yaml::to_string(&adopted_fs).unwrap(), adopted_yaml);
    }

    #[test]
    fn test_serde_roundtrip() {
        fn roundtrip(dev_id: Option<String>, source: FileSystemSource, mp: Option<MountPoint>) {
            let original = FileSystem {
                device_id: dev_id,
                source,
                mount_point: mp,
                is_esp: false,
            };

            let serialized_result = serde_yaml::to_string(&original).unwrap();
            let deserialized_result =
                serde_yaml::from_str::<FileSystem>(&serialized_result).unwrap();
            assert_eq!(original, deserialized_result);
        }

        roundtrip(
            Some("root".to_string()),
            FileSystemSource::Image,
            Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
        );
        roundtrip(
            Some("trident".to_string()),
            FileSystemSource::New(NewFileSystemType::default()),
            Some(MountPoint {
                path: "/mnt/my-tmp".into(),
                options: MountOptions::defaults(),
            }),
        );
        roundtrip(
            Some("trident".to_string()),
            FileSystemSource::New(NewFileSystemType::Tmpfs),
            Some(MountPoint {
                path: "/mnt/my-tmp".into(),
                options: MountOptions::empty(),
            }),
        );
        roundtrip(
            Some("trident".to_string()),
            FileSystemSource::Adopted(AdoptedFileSystemType::default()),
            None,
        );
        roundtrip(
            Some("trident".to_string()),
            FileSystemSource::Adopted(AdoptedFileSystemType::Iso9660),
            Some(MountPoint {
                path: "/mnt/custom".into(),
                options: MountOptions::defaults(),
            }),
        );
    }

    #[test]
    fn test_mount_options_to_vec_functions() {
        let mut mount_point = MountPoint {
            path: "/mnt/empty".into(),
            options: MountOptions::empty(),
        };
        assert_eq!(mount_point.options.to_str_vec(), Vec::<&str>::new());
        assert_eq!(mount_point.options.to_string_vec(), Vec::<String>::new());

        mount_point.options = MountOptions::new(", ,,");
        assert_eq!(mount_point.options.to_str_vec(), Vec::<&str>::new());
        assert_eq!(mount_point.options.to_string_vec(), Vec::<String>::new());

        mount_point.options = MountOptions::new("  ");
        assert_eq!(mount_point.options.to_str_vec(), Vec::<&str>::new());
        assert_eq!(mount_point.options.to_string_vec(), Vec::<String>::new());

        mount_point.options = MountOptions::default();
        assert_eq!(mount_point.options.to_str_vec(), vec!["defaults"]);
        assert_eq!(
            mount_point.options.to_string_vec(),
            vec!["defaults".to_string()]
        );

        mount_point.options = MountOptions::new("a,b,c,d");
        assert_eq!(mount_point.options.to_str_vec(), vec!["a", "b", "c", "d"]);
        assert_eq!(
            mount_point.options.to_string_vec(),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
    }
}
