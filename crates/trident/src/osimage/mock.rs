use std::{
    collections::BTreeMap,
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
    pub gpt: GptDisk<Cursor<Vec<u8>>>,
}

impl MockPartitioningInfo {
    /// Creates a new `MockPartitioningInfo` with a protective MBR and GPT
    /// header with no partitions.
    pub fn new_protective_mbr_and_gpt() -> Result<Self, Error> {
        let fake_disk_size = 10u64 * 1024 * 1024 * 1024; // 10 GiB
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
            .create_from_device(Cursor::new(mock_gpt_area), None)?;

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

/// An arbitrary data containers that pretends to be bigger than it actually is.
pub struct SparseCursor {
    size: u64,
    // Holds sparse data in NON-overlapping chunks. The key is the offset of the
    // chunk, and the value is the data.
    data: BTreeMap<u64, Vec<u8>>,
    position: u64,
}

impl SparseCursor {
    pub fn new(size: u64) -> Self {
        Self {
            size,
            data: BTreeMap::new(),
            position: 0,
        }
    }

    /// Returns the number of bytes remaining until the end of the cursor.
    fn remaining(&self) -> u64 {
        self.size.saturating_sub(self.position)
    }

    /// Returns the number of bytes that can be written, which is the minimum of
    /// the remaining bytes and the provided size.
    fn writable_size(&self, size: u64) -> u64 {
        self.remaining().min(size)
    }

    
    fn chunk_end(start: u64, data: &[u8]) -> u64 {
        start + data.len() as u64
    }

    fn copy_overlap(
        dst: &mut [u8],
        dst_start: u64,
        dst_end: u64,
        chunk_start: u64,
        chunk_data: &[u8],
    ) {
        let chunk_end = Self::chunk_end(chunk_start, chunk_data);
        let overlap_start = chunk_start.max(dst_start);
        let overlap_end = chunk_end.min(dst_end);

        if overlap_start >= overlap_end {
            return;
        }

        let src_offset = (overlap_start - chunk_start) as usize;
        let dst_offset = (overlap_start - dst_start) as usize;
        let len = (overlap_end - overlap_start) as usize;

        dst[dst_offset..dst_offset + len]
            .copy_from_slice(&chunk_data[src_offset..src_offset + len]);
    }
}

impl Write for SparseCursor {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let to_write = self.writable_size(buf.len() as u64);
        if to_write == 0 {
            return Ok(0);
        }

        let write_start = self.position;
        let write_end = write_start + to_write;

        // Fast path: write fits entirely within an existing chunk.
        if let Some((chunk_start, chunk)) = self.data.range_mut(..=write_start).next_back() {
            let chunk_end = Self::chunk_end(*chunk_start, chunk);
            if write_end <= chunk_end {
                let offset = (write_start - *chunk_start) as usize;
                chunk[offset..offset + to_write as usize]
                    .copy_from_slice(&buf[..to_write as usize]);
                self.position = write_end;
                return Ok(to_write as usize);
            }
        }

        let mut merge_start = write_start;
        let mut merge_end = write_end;
        let mut keys_to_remove: Vec<u64> = Vec::new();

        // Check previous chunk for overlap or adjacency.
        if let Some((chunk_start, chunk)) = self.data.range(..=write_start).next_back() {
            let chunk_end = Self::chunk_end(*chunk_start, chunk);
            if chunk_end >= write_start {
                merge_start = merge_start.min(*chunk_start);
                merge_end = merge_end.max(chunk_end);
                keys_to_remove.push(*chunk_start);
            }
        }

        // Check following chunks that overlap or touch the merge range.
        let mut search_start = write_start;
        loop {
            let next = self
                .data
                .range(search_start..)
                .next()
                .map(|(s, d)| (*s, d.len() as u64));

            let Some((chunk_start, len)) = next else {
                break;
            };

            if chunk_start > merge_end {
                break;
            }

            let chunk_end = chunk_start + len;
            merge_start = merge_start.min(chunk_start);
            merge_end = merge_end.max(chunk_end);
            keys_to_remove.push(chunk_start);

            search_start = chunk_start + 1;
        }

        keys_to_remove.sort_unstable();
        keys_to_remove.dedup();

        let merged_len = (merge_end - merge_start) as usize;
        let mut merged = vec![0u8; merged_len];

        for key in keys_to_remove {
            if let Some(chunk) = self.data.remove(&key) {
                let offset = (key - merge_start) as usize;
                merged[offset..offset + chunk.len()].copy_from_slice(&chunk);
            }
        }

        let write_offset = (write_start - merge_start) as usize;
        merged[write_offset..write_offset + to_write as usize]
            .copy_from_slice(&buf[..to_write as usize]);

        self.data.insert(merge_start, merged);

        self.position = write_end;
        Ok(to_write as usize)
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

impl Read for SparseCursor {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let to_read = self.remaining().min(buf.len() as u64) as usize;
        if to_read == 0 {
            return Ok(0);
        }

        let read_start = self.position;
        let read_end = read_start + to_read as u64;

        buf[..to_read].fill(0);

        // Check previous chunk for overlap.
        if let Some((chunk_start, chunk)) = self.data.range(..=read_start).next_back() {
            let chunk_end = Self::chunk_end(*chunk_start, chunk);
            if chunk_end > read_start {
                Self::copy_overlap(
                    &mut buf[..to_read],
                    read_start,
                    read_end,
                    *chunk_start,
                    chunk,
                );
            }
        }

        // Iterate over chunks starting at or after read_start.
        for (chunk_start, chunk) in self.data.range(read_start..) {
            if *chunk_start >= read_end {
                break;
            }
            Self::copy_overlap(
                &mut buf[..to_read],
                read_start,
                read_end,
                *chunk_start,
                chunk,
            );
        }

        self.position = read_end;
        Ok(to_read)
    }
}

impl Seek for SparseCursor {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        let size = self.size as i128;
        let current = self.position as i128;
        let next = match pos {
            SeekFrom::Start(offset) => offset as i128,
            SeekFrom::End(offset) => size + offset as i128,
            SeekFrom::Current(offset) => current + offset as i128,
        };

        if next < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid seek to a negative position",
            ));
        }

        self.position = next as u64;
        Ok(self.position)
    }
}
