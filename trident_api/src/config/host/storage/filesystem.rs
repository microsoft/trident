use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use serde::{Deserialize, Serialize};

use crate::{
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    BlockDeviceId,
};

use super::filesystem_types::{AdoptedFileSystemType, FileSystemType, NewFileSystemType};

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

pub mod fs_serde {
    #[cfg(feature = "schemars")]
    use schemars::JsonSchema;

    use serde::{Deserialize, Deserializer, Serialize};

    use crate::is_default;

    #[cfg(feature = "schemars")]
    use crate::schema_helpers::block_device_id_schema;

    use super::{AdoptedFileSystemType, FileSystemType, MountPoint, NewFileSystemType};

    #[derive(Deserialize, Serialize, Default, PartialEq, Eq)]
    #[serde(rename_all = "kebab-case", deny_unknown_fields)]
    #[cfg_attr(feature = "schemars", derive(JsonSchema))]
    enum FileSystemSource {
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
    struct FileSystem {
        /// The ID of the block device on which to place this file system.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
        device_id: Option<String>,

        /// The source of the file system.
        ///
        /// If not specified, this field will default to image.
        #[serde(default, skip_serializing_if = "is_default")]
        source: FileSystemSource,

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
    }

    #[cfg(feature = "schemars")]
    impl JsonSchema for super::FileSystem {
        fn schema_name() -> String {
            "FileSystem".to_string()
        }

        fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
            FileSystem::json_schema(gen)
        }
    }

    impl<'de> Deserialize<'de> for super::FileSystem {
        fn deserialize<D>(deserializer: D) -> Result<super::FileSystem, D::Error>
        where
            D: Deserializer<'de>,
        {
            let interim = FileSystem::deserialize(deserializer)?;
            let source = match interim.source {
                FileSystemSource::Adopted => {
                    super::FileSystemSource::Adopted(match interim.fs_type {
                        None => AdoptedFileSystemType::default(),
                        Some(fs_type) => AdoptedFileSystemType::try_from(fs_type).map_err(|e| {
                            serde::de::Error::custom(format!(
                                "Invalid adopted filesystem type: {e}"
                            ))
                        })?,
                    })
                }
                FileSystemSource::New => super::FileSystemSource::New(match interim.fs_type {
                    None => NewFileSystemType::default(),
                    Some(fs_type) => NewFileSystemType::try_from(fs_type).map_err(|e| {
                        serde::de::Error::custom(format!("Invalid new filesystem type: {e}"))
                    })?,
                }),
                FileSystemSource::Image => {
                    if interim.fs_type.is_some() {
                        return Err(serde::de::Error::custom(
                            "Filesystem type cannot be specified for image filesystems",
                        ));
                    }
                    super::FileSystemSource::Image
                }
            };
            Ok(super::FileSystem {
                device_id: interim.device_id,
                source,
                mount_point: interim.mount_point,
            })
        }
    }

    impl Serialize for super::FileSystem {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            let interim = FileSystem {
                device_id: self.device_id.clone(),
                source: match &self.source {
                    super::FileSystemSource::Image => FileSystemSource::Image,
                    super::FileSystemSource::New(_) => FileSystemSource::New,
                    super::FileSystemSource::Adopted(_) => FileSystemSource::Adopted,
                },
                mount_point: self.mount_point.clone(),
                fs_type: match &self.source {
                    super::FileSystemSource::New(fs_type) => Some((*fs_type).into()),
                    super::FileSystemSource::Adopted(fs_type) => Some((*fs_type).into()),
                    _ => None,
                },
            };
            interim.serialize(serializer)
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
            // ("type", Some(self.fs_type.to_string())),
            ("dev", self.device_id.clone()),
            (
                "mnt",
                self.mount_point
                    .as_ref()
                    .map(|mp| mp.path.to_string_lossy().to_string()),
            ),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{k}:{v}")))
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// Returns whether the filesystem is the EFI System Partition (ESP), as
    /// determined by its mount point path.
    pub fn is_esp(&self) -> bool {
        self.mount_point_path()
            .is_some_and(|mpp| mpp == Path::new(ESP_MOUNT_POINT_PATH))
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
        };
        assert_eq!(fs.mount_point_path(), None);

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/mnt"),
            options: MountOptions::new("defaults"),
        });
        assert_eq!(fs.mount_point_path(), Some(Path::new("/mnt")));
        assert!(!fs.is_esp());
        assert!(!fs.is_root());
        assert!(!fs.is_read_only());

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/boot/efi"),
            options: MountOptions::new("defaults"),
        });
        assert_eq!(fs.mount_point_path(), Some(Path::new(ESP_MOUNT_POINT_PATH)));
        assert!(fs.is_esp());
        assert!(!fs.is_root());
        assert!(!fs.is_read_only());

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/"),
            options: MountOptions::new("defaults"),
        });
        assert_eq!(
            fs.mount_point_path(),
            Some(Path::new(ROOT_MOUNT_POINT_PATH))
        );
        assert!(!fs.is_esp());
        assert!(fs.is_root());
        assert!(!fs.is_read_only());

        fs.mount_point = Some(MountPoint {
            path: PathBuf::from("/mnt"),
            options: MountOptions::new("ro"),
        });
        assert_eq!(fs.mount_point_path(), Some(Path::new("/mnt")));
        assert!(!fs.is_esp());
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
        };

        // Success: source unspecified
        let yaml = r#"
deviceId: root
mountPoint: /
"#;
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, root_fs);

        // Success: source specified
        let yaml = r#"
deviceId: root
source: image
mountPoint: /
"#;
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, root_fs);

        // Failure: filesystem type specified
        let yaml = r#"
deviceId: root
source: image
type: ext4
mountPoint: /
"#;
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
        };

        // Success: filesystem type unspecified (default to Ext4)
        let yaml = r#"
deviceId: trident
source: new
mountPoint: /
"#;
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, new_fs);

        // Success: filesystem type specified
        let yaml = r#"
deviceId: trident
source: new
type: ext4
mountPoint: /
"#;
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, new_fs);

        // Failure: invalid filesystem type specified (unknown)
        let yaml = r#"
deviceId: trident
source: new
type: abcd
mountPoint: /
"#;
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown variant `abcd`"));

        // Failure: invalid filesystem type specified (adopted)
        let yaml = r#"
deviceId: trident
source: new
type: iso9660
mountPoint: /
"#;
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
        };

        // Success: filesystem type unspecified (default to Auto)
        let yaml = r#"
deviceId: trident
source: adopted
mountPoint: /
"#;
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, adopted_fs);

        // Success: filesystem type specified
        let yaml = r#"
deviceId: trident
source: adopted
type: auto
mountPoint: /
"#;
        let filesystem: FileSystem = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(filesystem, adopted_fs);

        // Failure: invalid filesystem type specified (unknown)
        let yaml = r#"
deviceId: trident
source: adopted
type: abcd
mountPoint: /
"#;
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown variant `abcd`"));

        // Failure: invalid filesystem type specified (new)
        let yaml = r#"
deviceId: trident
source: adopted
type: tmpfs
mountPoint: /
"#;
        let err = serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
        assert!(err.to_string().contains("Invalid adopted filesystem type"));

        // Failure: invalid nesting structure
        let yaml = r#"
deviceId: trident
source:
  source: adopted
  type: auto
mountPoint: /
"#;
        serde_yaml::from_str::<FileSystem>(yaml).unwrap_err();
    }

    #[test]
    fn test_serialize_filesystem() {
        // Image
        let image_yaml = r#"deviceId: root
mountPoint:
  path: /
  options: defaults
"#;
        let image_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::Image,
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
        };
        assert_eq!(serde_yaml::to_string(&image_fs).unwrap(), image_yaml);

        // New
        let new_yaml = r#"deviceId: root
source: new
type: ext4
mountPoint:
  path: /
  options: defaults
"#;
        let new_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
        };
        assert_eq!(serde_yaml::to_string(&new_fs).unwrap(), new_yaml);

        // Adopted
        let adopted_yaml = r#"deviceId: root
source: adopted
type: auto
mountPoint:
  path: /
  options: defaults
"#;
        let adopted_fs = FileSystem {
            device_id: Some("root".into()),
            source: FileSystemSource::Adopted(AdoptedFileSystemType::Auto),
            mount_point: Some(MountPoint {
                path: "/".into(),
                options: MountOptions::defaults(),
            }),
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
