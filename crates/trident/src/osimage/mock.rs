use std::{
    io::{Cursor, Read, Result as IoResult, Seek, SeekFrom, Write},
    ops::ControlFlow,
    path::{Path, PathBuf},
};

use anyhow::Error;
use gpt::GptDisk;
use gpt::{disk::LogicalBlockSize, mbr::ProtectiveMBR, GptConfig};
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use osutils::osrelease::OsRelease;
use sysdefs::{
    arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
};
use trident_api::{error::TridentError, primitives::hash::Sha384Hash};

use super::{OsImageFile, OsImageFileSystem, OsImageFileSystemType, OsImageVerityHash};

/// Content returned by the reader of a mock OS image file.
pub const MOCK_OS_IMAGE_CONTENT: &str = "mock-os-image-content-lorem-ipsum";

/// This is a generic abstraction of what an OS image is, which can be used to
/// mock an OS image for testing purposes. It should not be tied to the
/// specifics of any one OS image implementation. Currently does not include
/// verity.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MockOsImage {
    pub source: Url,

    pub os_arch: SystemArchitecture,

    pub os_release: OsRelease,

    pub images: Vec<MockImage>,

    pub is_uki: bool,

    #[serde(skip)]
    pub partitioning_info: Option<MockPartitioningInfo>,
}

#[derive(Debug, Clone)]
pub struct MockPartitioningInfo {
    pub lba0: [u8; 512],
    pub gpt: GptDisk<MockLargeFile>,
}

impl MockPartitioningInfo {
    /// Creates a new `MockPartitioningInfo` with a protective MBR and GPT
    /// header with no partitions.
    pub fn new_protective_mbr_and_gpt(fake_disk_size: u64) -> Result<Self, Error> {
        let lba_size = 512;

        // Protective MBR bytes.
        let protective_mbr =
            ProtectiveMBR::with_lb_size((fake_disk_size / lba_size - 1) as u32).to_bytes();

        // A mini 100KiB disk that should be enough to hold the primary and
        // backup gpt.
        let mut mock_gpt_area = vec![0; lba_size as usize * 100];

        // Set the first 512 bytes to the protective MBR.
        mock_gpt_area[..lba_size as usize].copy_from_slice(&protective_mbr);

        let disk = GptConfig::new()
            .change_partition_count(true)
            .writable(true)
            .logical_block_size(LogicalBlockSize::Lb512)
            .readonly_backup(true)
            .create_from_device(MockLargeFile::new(fake_disk_size, mock_gpt_area), None)?;

        Ok(Self {
            lba0: protective_mbr,
            gpt: disk,
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MockImage {
    pub mount_point: PathBuf,

    pub fs_type: OsImageFileSystemType,

    pub fs_uuid: OsUuid,

    pub part_type: DiscoverablePartitionType,

    pub verity: Option<MockVerity>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MockVerity {
    pub roothash: String,
}

fn mock_os_image_file() -> OsImageFile {
    OsImageFile {
        compressed_size: 0,
        sha384: Sha384Hash::from("mock-sha384"),
        uncompressed_size: 0,
        path: "/img.raw.zstd".into(),
    }
}

impl MockOsImage {
    /// Returns a new mock OS image with dummy data.
    pub fn new() -> Self {
        Self {
            source: Url::parse("mock://").unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            is_uki: false,
            images: vec![],
            partitioning_info: None,
        }
    }

    /// Adds an image to the mock OS image.
    pub fn with_image(mut self, image: MockImage) -> Self {
        self.images.push(image);
        self
    }

    /// Adds an image to the mock OS image.
    pub fn with_images(mut self, images: impl IntoIterator<Item = MockImage>) -> Self {
        self.images.extend(images);
        self
    }

    pub fn with_partitioning_info(mut self, info: MockPartitioningInfo) -> Self {
        self.partitioning_info = Some(info);
        self
    }

    /// Returns an iterator of available mount points in the COSI file.
    pub(super) fn available_mount_points(&self) -> impl Iterator<Item = &Path> {
        self.images
            .iter()
            .filter(|fs| fs.part_type != DiscoverablePartitionType::Esp)
            .map(|image| image.mount_point.as_path())
    }

    /// Returns the ESP filesystem image.
    pub fn esp_filesystem(&self) -> Result<OsImageFileSystem, Error> {
        if let Some(esp_img) = self
            .images
            .iter()
            .find(|fs| fs.part_type == DiscoverablePartitionType::Esp)
        {
            Ok(OsImageFileSystem {
                mount_point: esp_img.mount_point.clone(),
                fs_type: esp_img.fs_type,
                fs_uuid: esp_img.fs_uuid.clone(),
                part_type: esp_img.part_type,
                image_file: mock_os_image_file(),
                verity: esp_img.verity.as_ref().map(|verity| OsImageVerityHash {
                    roothash: verity.roothash.clone(),
                    hash_image_file: mock_os_image_file(),
                }),
            })
        } else {
            Err(Error::msg("No ESP filesystem found"))
        }
    }

    /// Returns non-ESP filesystems.
    pub fn filesystems(&self) -> impl Iterator<Item = OsImageFileSystem> {
        self.images
            .iter()
            .filter(|fs| fs.part_type != DiscoverablePartitionType::Esp)
            .map(|image| OsImageFileSystem {
                mount_point: image.mount_point.clone(),
                fs_type: image.fs_type,
                fs_uuid: image.fs_uuid.clone(),
                part_type: image.part_type,
                image_file: mock_os_image_file(),
                verity: image.verity.as_ref().map(|verity| OsImageVerityHash {
                    roothash: verity.roothash.clone(),
                    hash_image_file: mock_os_image_file(),
                }),
            })
    }

    /// Returns the OS architecture of the image.
    pub fn architecture(&self) -> SystemArchitecture {
        self.os_arch
    }

    pub fn metadata_sha384(&self) -> Sha384Hash {
        Sha384Hash::from("0".repeat(96))
    }

    pub(super) fn read_images<F>(&self, mut f: F) -> Result<(), TridentError>
    where
        F: FnMut(&Path, Box<dyn Read>) -> ControlFlow<Result<(), TridentError>>,
    {
        match f(
            Path::new("/img.raw.zstd"),
            Box::new(MOCK_OS_IMAGE_CONTENT.as_bytes()),
        ) {
            ControlFlow::Continue(()) => Ok(()),
            ControlFlow::Break(b) => b,
        }
    }
}

impl MockImage {
    /// Returns a new mock image with dummy data.
    pub fn new(
        mount_point: impl AsRef<Path>,
        fs_type: OsImageFileSystemType,
        part_type: DiscoverablePartitionType,
        roothash: Option<impl AsRef<str>>,
    ) -> Self {
        Self {
            mount_point: mount_point.as_ref().to_owned(),
            fs_type,
            part_type,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            verity: roothash.map(|roothash| MockVerity {
                roothash: roothash.as_ref().to_owned(),
            }),
        }
    }
}

/// A ReadWriteSeek object that pretends to be much larger.
///
/// The gpt library is generally designed to work with disk-like objects, and it
/// uses seek to figure out the size of the disk. For mocking purposes, we want
/// to be able to create a GPT disk that appears to be much larger than the
/// actual data we have, so that we can test how our code handles large disks
/// without needing to allocate a huge amount of memory.
///
/// This struct assumes that no one is doing reads or writes outside of what
/// inner data contains.
#[derive(Debug, Clone)]
pub struct MockLargeFile {
    size: u64,
    cursor: Cursor<Vec<u8>>,
}

impl MockLargeFile {
    pub fn new(size: u64, data: Vec<u8>) -> Self {
        Self {
            size,
            cursor: Cursor::new(data),
        }
    }
}

impl Read for MockLargeFile {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.cursor.read(buf)
    }
}

impl Write for MockLargeFile {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        self.cursor.write(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        self.cursor.flush()
    }
}

impl Seek for MockLargeFile {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        match pos {
            SeekFrom::Start(offset) => {
                self.cursor.set_position(offset);
                Ok(offset)
            }
            SeekFrom::End(offset) => {
                let new_pos = (self.size as i64 + offset) as u64;
                self.cursor.set_position(new_pos);
                Ok(new_pos)
            }
            SeekFrom::Current(offset) => {
                let new_pos = (self.cursor.position() as i64 + offset) as u64;
                self.cursor.set_position(new_pos);
                Ok(new_pos)
            }
        }
    }
}
