use serde::{de::value::Error, forward_to_deserialize_any, Deserialize, Deserializer};
use strum_macros::{EnumIs, IntoStaticStr};

/// Superset of all filesystem types recognized by the kernel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, EnumIs)]
#[serde(untagged)]
pub enum KernelFilesystemType {
    Real(RealFilesystemType),
    Nodev(NodevFilesystemType),
    #[serde(untagged)]
    Other(String),
}

impl From<RealFilesystemType> for KernelFilesystemType {
    fn from(fs: RealFilesystemType) -> Self {
        KernelFilesystemType::Real(fs)
    }
}

impl From<NodevFilesystemType> for KernelFilesystemType {
    fn from(fs: NodevFilesystemType) -> Self {
        KernelFilesystemType::Nodev(fs)
    }
}

impl From<&str> for KernelFilesystemType {
    fn from(fs: &str) -> Self {
        Self::deserialize(&mut EnumDeserializer(fs))
            .unwrap_or_else(|_| KernelFilesystemType::Other(fs.to_string()))
    }
}

/// List of all known real or physical filesystem types. These are types that
/// require a block device.
///
/// Essentially, things you might see in `/proc/filesystems` without the `nodev`
/// attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, IntoStaticStr)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum RealFilesystemType {
    Btrfs,
    Cramfs,
    Exfat,
    Ext2,
    Ext3,
    Ext4,
    Fuseblk,
    Iso9660,
    Msdos,
    Ntfs,
    Squashfs,
    Udf,
    Vfat,
    Xfs,
}

impl RealFilesystemType {
    pub fn as_kernel(self) -> KernelFilesystemType {
        self.into()
    }
}

/// List of all known nodev filesystem types. These are types that do NOT use a
/// block device.
///
/// Essentially, things you might see in `/proc/filesystems` WITH the `nodev`
/// attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, IntoStaticStr)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum NodevFilesystemType {
    Autofs,
    Bdev,
    Bpf,
    Cgroup,
    Cgroup2,
    Configfs,
    Cpuset,
    Debugfs,
    Devpts,
    Devtmpfs,
    Efivarfs,
    Fuse,
    Fusectl,
    Hugetlbfs,
    Mqueue,
    Overlay,
    Pipefs,
    Proc,
    Pstore,
    Ramfs,
    Securityfs,
    Selinuxfs,
    Sockfs,
    Sysfs,
    Tmpfs,
    Tracefs,
}

impl NodevFilesystemType {
    pub fn as_kernel(self) -> KernelFilesystemType {
        self.into()
    }
}

/// Simple deserializer to convert a &str into an enum using serde.
struct EnumDeserializer<'de>(&'de str);
impl<'de> Deserializer<'de> for &mut EnumDeserializer<'de> {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_str(self.0)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kernel_filesystem_type() {
        let json = r#""ext4""#;
        let fs: KernelFilesystemType = serde_json::from_str(json).unwrap();
        assert_eq!(fs, KernelFilesystemType::Real(RealFilesystemType::Ext4));
        assert!(fs.is_real());
        assert!(!fs.is_nodev());
        assert!(!fs.is_other());

        let json = r#""overlay""#;
        let fs: KernelFilesystemType = serde_json::from_str(json).unwrap();
        assert_eq!(
            fs,
            KernelFilesystemType::Nodev(NodevFilesystemType::Overlay)
        );
        assert!(!fs.is_real());
        assert!(fs.is_nodev());
        assert!(!fs.is_other());

        let json = r#""some-other-thing""#;
        let fs: KernelFilesystemType = serde_json::from_str(json).unwrap();
        assert_eq!(
            fs,
            KernelFilesystemType::Other("some-other-thing".to_string())
        );
        assert!(!fs.is_real());
        assert!(!fs.is_nodev());
        assert!(fs.is_other());

        // Test From<X> for KernelFilesystemType implementations

        fn test(thing: impl Into<KernelFilesystemType>, expected: KernelFilesystemType) {
            let fs: KernelFilesystemType = thing.into();
            assert_eq!(fs, expected);
        }

        test(
            RealFilesystemType::Ext4,
            KernelFilesystemType::Real(RealFilesystemType::Ext4),
        );

        test(
            NodevFilesystemType::Overlay,
            KernelFilesystemType::Nodev(NodevFilesystemType::Overlay),
        );

        // Test From<&str> for KernelFilesystemType implementations

        test("ext4", KernelFilesystemType::Real(RealFilesystemType::Ext4));
        test(
            "overlay",
            KernelFilesystemType::Nodev(NodevFilesystemType::Overlay),
        );

        test(
            "some-other-thing",
            KernelFilesystemType::Other("some-other-thing".to_string()),
        );

        // Test as_kernel() methods

        let fs = RealFilesystemType::Ext4;
        let kfs = fs.as_kernel();
        assert_eq!(kfs, KernelFilesystemType::Real(RealFilesystemType::Ext4));

        let fs = NodevFilesystemType::Overlay;
        let kfs = fs.as_kernel();
        assert_eq!(
            kfs,
            KernelFilesystemType::Nodev(NodevFilesystemType::Overlay)
        );
    }
}
